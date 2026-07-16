use std::{sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use etcd_client::{EventType as EtcdEventType, WatchOptions};
use nacos_sdk::api::config::{ConfigChangeListener, ConfigResponse};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zookeeper_client::{EventType as ZookeeperEventType, SessionState, WatchedEvent};

use super::{
    RegistryError, ResourceAddress, SubscriptionId, WatchChangeKind, WatchEvent, WatchRequest,
    WatchStatusState,
    adapters::{NacosSession, RegistrySession, read_nacos_authoritative},
};

const INITIAL_RETRY: Duration = Duration::from_millis(250);
const MAX_RETRY: Duration = Duration::from_secs(5);
const LISTENER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const NACOS_HEALTH_INTERVAL: Duration = Duration::from_secs(5);

pub(super) async fn run(
    session: RegistrySession,
    subscription_id: SubscriptionId,
    request: WatchRequest,
    token: CancellationToken,
    events: mpsc::Sender<WatchEvent>,
) -> Result<(), RegistryError> {
    match session {
        RegistrySession::Etcd(client) => {
            watch_etcd(*client, subscription_id, request, token, events).await
        }
        RegistrySession::Zookeeper(client) => {
            watch_zookeeper(client, subscription_id, request, token, events).await
        }
        RegistrySession::Nacos(session) => {
            watch_nacos(session, subscription_id, request, token, events).await
        }
    }
}

async fn watch_etcd(
    mut client: etcd_client::Client,
    subscription_id: SubscriptionId,
    request: WatchRequest,
    token: CancellationToken,
    events: mpsc::Sender<WatchEvent>,
) -> Result<(), RegistryError> {
    let (key, prefix) = match &request.address {
        ResourceAddress::Etcd { key_base64 } => (decode(key_base64, "etcd key")?, false),
        ResourceAddress::EtcdPrefix { prefix_base64 } => {
            (decode(prefix_base64, "etcd prefix")?, true)
        }
        _ => {
            return Err(RegistryError::validation(
                "etcd watch requires an etcd key or prefix",
            ));
        }
    };
    let mut next_revision = request
        .start_version
        .as_deref()
        .map(str::trim)
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|_| RegistryError::validation("invalid etcd start revision"))?
        .map(|revision| revision.saturating_add(1));
    let mut retry = INITIAL_RETRY;

    loop {
        let requested_revision = next_revision;
        let mut options = WatchOptions::new().with_progress_notify();
        if prefix {
            options = options.with_prefix();
        }
        if let Some(revision) = next_revision {
            options = options.with_start_revision(revision);
        }
        let stream = tokio::select! {
            () = token.cancelled() => return Ok(()),
            result = client.watch(key.clone(), Some(options)) => result,
        };
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(error) if retryable_etcd_watch_error(&error) => {
                if !reconnecting(
                    &events,
                    &subscription_id,
                    "etcd watch connection lost",
                    retry,
                )
                .await
                {
                    return Ok(());
                }
                wait_retry(&token, retry).await?;
                retry = next_retry(retry);
                continue;
            }
            Err(error) => return Err(etcd_watch_error(error)),
        };
        let mut confirmed = false;

        loop {
            let response = tokio::select! {
                () = token.cancelled() => return Ok(()),
                response = stream.message() => response,
            };
            let response = match response {
                Ok(Some(response)) => response,
                Ok(None) => {
                    if !reconnecting(
                        &events,
                        &subscription_id,
                        "etcd watch stream interrupted",
                        retry,
                    )
                    .await
                    {
                        return Ok(());
                    }
                    break;
                }
                Err(error) if retryable_etcd_watch_error(&error) => {
                    if !reconnecting(
                        &events,
                        &subscription_id,
                        "etcd watch stream interrupted",
                        retry,
                    )
                    .await
                    {
                        return Ok(());
                    }
                    break;
                }
                Err(error) => return Err(etcd_watch_error(error)),
            };
            if response.compact_revision() > 0 {
                let revision = response.compact_revision();
                if !status(
                    &events,
                    &subscription_id,
                    WatchStatusState::Compacted,
                    Some(format!(
                        "etcd events before revision {revision} were compacted; refresh before restarting the watch"
                    )),
                    None,
                )
                .await
                {
                    return Ok(());
                }
                token.cancelled().await;
                return Ok(());
            }
            if response.canceled() {
                return Err(RegistryError::network(format!(
                    "etcd watch was cancelled by the server: {}",
                    response.cancel_reason()
                )));
            }
            if response.created() && !confirmed {
                if requested_revision.is_none()
                    && let Some(header) = response.header()
                {
                    next_revision = Some(header.revision().saturating_add(1));
                }
                if !status(
                    &events,
                    &subscription_id,
                    WatchStatusState::Live,
                    None,
                    None,
                )
                .await
                {
                    return Ok(());
                }
                confirmed = true;
                retry = INITIAL_RETRY;
            }
            if !response.created()
                && response.events().is_empty()
                && let Some(header) = response.header()
            {
                next_revision = Some(
                    next_revision
                        .unwrap_or_default()
                        .max(header.revision().saturating_add(1)),
                );
            }
            for event in response.events() {
                if !confirmed {
                    if !status(
                        &events,
                        &subscription_id,
                        WatchStatusState::Live,
                        None,
                        None,
                    )
                    .await
                    {
                        return Ok(());
                    }
                    confirmed = true;
                    retry = INITIAL_RETRY;
                }
                let Some(key_value) = event.kv() else {
                    continue;
                };
                next_revision = Some(
                    next_revision
                        .unwrap_or_default()
                        .max(key_value.mod_revision().saturating_add(1)),
                );
                let change = match event.event_type() {
                    EtcdEventType::Put if key_value.version() == 1 => WatchChangeKind::Created,
                    EtcdEventType::Put => WatchChangeKind::Updated,
                    EtcdEventType::Delete => WatchChangeKind::Deleted,
                };
                if events
                    .send(WatchEvent::Change {
                        subscription_id: subscription_id.as_str().to_owned(),
                        change,
                        address: ResourceAddress::Etcd {
                            key_base64: STANDARD.encode(key_value.key()),
                        },
                        version: Some(key_value.mod_revision().to_string()),
                    })
                    .await
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
        wait_retry(&token, retry).await?;
        retry = next_retry(retry);
    }
}

async fn watch_zookeeper(
    client: zookeeper_client::Client,
    subscription_id: SubscriptionId,
    request: WatchRequest,
    token: CancellationToken,
    events: mpsc::Sender<WatchEvent>,
) -> Result<(), RegistryError> {
    let path = match request.address {
        ResourceAddress::Zookeeper { path } => path,
        _ => {
            return Err(RegistryError::validation(
                "ZooKeeper watch requires a ZooKeeper path",
            ));
        }
    };
    let address = ResourceAddress::Zookeeper { path: path.clone() };
    let mut state_watcher = client.state_watcher();
    let mut retry = INITIAL_RETRY;

    loop {
        let armed = tokio::select! {
            () = token.cancelled() => return Ok(()),
            result = client.check_and_watch_stat(&path) => result,
        };
        let (_stat, watcher) = match armed {
            Ok(armed) => armed,
            Err(zookeeper_client::Error::SessionExpired) => {
                session_expired(&events, &subscription_id).await;
                token.cancelled().await;
                return Ok(());
            }
            Err(zookeeper_client::Error::NoAuth | zookeeper_client::Error::AuthFailed) => {
                return Err(RegistryError::permission_denied(
                    "ZooKeeper watch authorization failed",
                ));
            }
            Err(zookeeper_client::Error::ConnectionLoss | zookeeper_client::Error::Timeout) => {
                if !reconnecting(
                    &events,
                    &subscription_id,
                    "ZooKeeper session disconnected",
                    retry,
                )
                .await
                {
                    return Ok(());
                }
                wait_retry(&token, retry).await?;
                retry = next_retry(retry);
                continue;
            }
            Err(error) => {
                return Err(RegistryError::network(format!(
                    "ZooKeeper watch failed: {error}"
                )));
            }
        };
        retry = INITIAL_RETRY;
        if !status(
            &events,
            &subscription_id,
            WatchStatusState::Live,
            None,
            None,
        )
        .await
        {
            return Ok(());
        }

        let changed = watcher.changed();
        tokio::pin!(changed);
        loop {
            tokio::select! {
                () = token.cancelled() => return Ok(()),
                event = &mut changed => {
                    if event.event_type == ZookeeperEventType::Session {
                        if event.session_state == SessionState::Expired {
                            session_expired(&events, &subscription_id).await;
                            token.cancelled().await;
                            return Ok(());
                        }
                        return Err(RegistryError::network(format!(
                            "ZooKeeper watch session ended: {}",
                            event.session_state
                        )));
                    }
                    if let Some(change) = zookeeper_change(&event)
                        && events.send(WatchEvent::Change {
                            subscription_id: subscription_id.as_str().to_owned(),
                            change,
                            address: address.clone(),
                            version: (event.zxid != WatchedEvent::NO_ZXID)
                                .then(|| event.zxid.to_string()),
                        }).await.is_err() {
                        return Ok(());
                    }
                    break;
                }
                state = state_watcher.changed() => match state {
                    SessionState::Disconnected => {
                        if !reconnecting(
                            &events,
                            &subscription_id,
                            "ZooKeeper session disconnected; one-shot watch will be restored",
                            retry,
                        ).await {
                            return Ok(());
                        }
                    }
                    SessionState::SyncConnected | SessionState::ConnectedReadOnly => {
                        if !status(
                            &events,
                            &subscription_id,
                            WatchStatusState::Live,
                            None,
                            None,
                        ).await {
                            return Ok(());
                        }
                    }
                    SessionState::Expired => {
                        session_expired(&events, &subscription_id).await;
                        token.cancelled().await;
                        return Ok(());
                    }
                    SessionState::AuthFailed => {
                        return Err(RegistryError::permission_denied(
                            "ZooKeeper watch authentication failed",
                        ));
                    }
                    SessionState::Closed => {
                        return Err(RegistryError::network("ZooKeeper client closed"));
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct NacosChange {
    address: ResourceAddress,
    version: Option<String>,
}

struct LatestNacosListener {
    address: ResourceAddress,
    latest: watch::Sender<Option<NacosChange>>,
}

impl ConfigChangeListener for LatestNacosListener {
    fn notify(&self, response: ConfigResponse) {
        self.latest.send_replace(Some(NacosChange {
            address: self.address.clone(),
            version: Some(response.md5().clone()),
        }));
    }
}

async fn watch_nacos(
    session: NacosSession,
    subscription_id: SubscriptionId,
    request: WatchRequest,
    token: CancellationToken,
    events: mpsc::Sender<WatchEvent>,
) -> Result<(), RegistryError> {
    let (group, data_id) = match &request.address {
        ResourceAddress::NacosConfig { group, data_id } => (group.clone(), data_id.clone()),
        _ => {
            return Err(RegistryError::validation(
                "Nacos watch requires an exact config address",
            ));
        }
    };
    let address = request.address;
    let (latest, mut changes) = watch::channel(None);
    let listener: Arc<dyn ConfigChangeListener> = Arc::new(LatestNacosListener {
        address: address.clone(),
        latest,
    });
    let registration = session
        .config
        .add_listener(data_id.clone(), group.clone(), listener.clone())
        .await;
    if token.is_cancelled() {
        if registration.is_ok() {
            cleanup_nacos_listener(session, data_id, group, listener).await?;
        }
        return Ok(());
    }
    registration.map_err(|error| {
        RegistryError::network(format!("Nacos listener registration failed: {error}"))
    })?;
    let initial_probe = tokio::select! {
        () = token.cancelled() => {
            return cleanup_nacos_listener(session, data_id, group, listener).await;
        }
        result = nacos_watch_version(&session, &data_id, &group) => result,
    };
    let mut last_version = None;
    let mut remote_live = match initial_probe {
        Ok(version) => {
            last_version = version;
            if !status(
                &events,
                &subscription_id,
                WatchStatusState::Live,
                None,
                None,
            )
            .await
            {
                return cleanup_nacos_listener(session, data_id, group, listener).await;
            }
            true
        }
        Err(error) if error.retryable => {
            if !reconnecting(
                &events,
                &subscription_id,
                "Nacos connection check failed; the SDK listener will keep reconnecting",
                NACOS_HEALTH_INTERVAL,
            )
            .await
            {
                return cleanup_nacos_listener(session, data_id, group, listener).await;
            }
            false
        }
        Err(error) => {
            cleanup_nacos_listener(session, data_id, group, listener).await?;
            return Err(error);
        }
    };
    let mut heartbeat = tokio::time::interval(NACOS_HEALTH_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    let mut terminal_error = None;

    loop {
        tokio::select! {
            () = token.cancelled() => break,
            _ = heartbeat.tick() => {
                let probe = tokio::select! {
                    () = token.cancelled() => break,
                    result = nacos_watch_version(&session, &data_id, &group) => result,
                };
                match probe {
                    Ok(version) => {
                        if !remote_live {
                            remote_live = true;
                            if !status(
                                &events,
                                &subscription_id,
                                WatchStatusState::Live,
                                None,
                                None,
                            ).await {
                                break;
                            }
                        }
                        if let Some(change) = nacos_reconciled_change(
                            last_version.as_deref(),
                            version.as_deref(),
                        ) {
                            last_version = version.clone();
                            if events.send(WatchEvent::Change {
                                subscription_id: subscription_id.as_str().to_owned(),
                                change,
                                address: address.clone(),
                                version,
                            }).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(error) if error.retryable => {
                        remote_live = false;
                        if !reconnecting(
                            &events,
                            &subscription_id,
                            "Nacos connection check failed; the SDK listener will keep reconnecting",
                            NACOS_HEALTH_INTERVAL,
                        ).await {
                            break;
                        }
                    }
                    Err(error) => {
                        terminal_error = Some(error);
                        break;
                    }
                }
            }
            result = changes.changed() => {
                if result.is_err() {
                    break;
                }
                let change = changes.borrow_and_update().clone();
                if !remote_live {
                    remote_live = true;
                    if !status(
                        &events,
                        &subscription_id,
                        WatchStatusState::Live,
                        None,
                        None,
                    ).await {
                        break;
                    }
                }
                if let Some(change) = change
                    && nacos_reconciled_change(
                        last_version.as_deref(),
                        change.version.as_deref(),
                    ).is_some()
                {
                    last_version = change.version.clone();
                    if events.send(WatchEvent::Change {
                        subscription_id: subscription_id.as_str().to_owned(),
                        change: WatchChangeKind::Updated,
                        address: change.address,
                        version: change.version,
                    }).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
    cleanup_nacos_listener(session, data_id, group, listener).await?;
    terminal_error.map_or(Ok(()), Err)
}

async fn nacos_watch_version(
    session: &NacosSession,
    data_id: &str,
    group: &str,
) -> Result<Option<String>, RegistryError> {
    match read_nacos_authoritative(session, data_id, group).await {
        Ok(response) => Ok(Some(response.md5().clone())),
        Err(error) if error.code == super::RegistryErrorCode::NotFound => Ok(None),
        Err(mut error) => {
            error.message = format!(
                "Nacos authoritative watch reconciliation failed: {}",
                error.message
            );
            Err(error)
        }
    }
}

fn nacos_reconciled_change(
    previous: Option<&str>,
    current: Option<&str>,
) -> Option<WatchChangeKind> {
    if previous == current {
        None
    } else if current.is_none() {
        Some(WatchChangeKind::Deleted)
    } else {
        Some(WatchChangeKind::Updated)
    }
}

async fn cleanup_nacos_listener(
    session: NacosSession,
    data_id: String,
    group: String,
    listener: Arc<dyn ConfigChangeListener>,
) -> Result<(), RegistryError> {
    tokio::time::timeout(
        LISTENER_CLEANUP_TIMEOUT,
        session.config.remove_listener(data_id, group, listener),
    )
    .await
    .map_err(|_| RegistryError::network("Nacos listener cleanup timed out"))?
    .map_err(|error| RegistryError::network(format!("Nacos listener cleanup failed: {error}")))
}

fn zookeeper_change(event: &WatchedEvent) -> Option<WatchChangeKind> {
    match event.event_type {
        ZookeeperEventType::NodeCreated => Some(WatchChangeKind::Created),
        ZookeeperEventType::NodeDeleted => Some(WatchChangeKind::Deleted),
        ZookeeperEventType::NodeDataChanged => Some(WatchChangeKind::Updated),
        ZookeeperEventType::NodeChildrenChanged => Some(WatchChangeKind::ChildrenChanged),
        ZookeeperEventType::Session => None,
    }
}

async fn status(
    events: &mpsc::Sender<WatchEvent>,
    subscription_id: &SubscriptionId,
    state: WatchStatusState,
    message: Option<String>,
    retry_in_ms: Option<u64>,
) -> bool {
    events
        .send(WatchEvent::status(
            subscription_id,
            state,
            message,
            retry_in_ms,
        ))
        .await
        .is_ok()
}

async fn reconnecting(
    events: &mpsc::Sender<WatchEvent>,
    subscription_id: &SubscriptionId,
    message: &str,
    retry: Duration,
) -> bool {
    status(
        events,
        subscription_id,
        WatchStatusState::Reconnecting,
        Some(message.to_owned()),
        Some(retry.as_millis() as u64),
    )
    .await
}

async fn session_expired(events: &mpsc::Sender<WatchEvent>, subscription_id: &SubscriptionId) {
    let _ = status(
        events,
        subscription_id,
        WatchStatusState::SessionExpired,
        Some("ZooKeeper session expired; refresh and reopen the connection".to_owned()),
        None,
    )
    .await;
}

async fn wait_retry(token: &CancellationToken, retry: Duration) -> Result<(), RegistryError> {
    tokio::select! {
        () = token.cancelled() => Ok(()),
        () = tokio::time::sleep(retry) => Ok(()),
    }
}

fn next_retry(current: Duration) -> Duration {
    current.saturating_mul(2).min(MAX_RETRY)
}

fn retryable_etcd_watch_error(error: &etcd_client::Error) -> bool {
    match error {
        etcd_client::Error::IoError(_)
        | etcd_client::Error::TransportError(_)
        | etcd_client::Error::WatchError(_)
        | etcd_client::Error::Internal(_) => true,
        etcd_client::Error::GRpcStatus(status) => matches!(
            status.code(),
            tonic::Code::Aborted
                | tonic::Code::Cancelled
                | tonic::Code::DeadlineExceeded
                | tonic::Code::Internal
                | tonic::Code::ResourceExhausted
                | tonic::Code::Unavailable
                | tonic::Code::Unknown
        ),
        _ => false,
    }
}

fn etcd_watch_error(error: etcd_client::Error) -> RegistryError {
    match &error {
        etcd_client::Error::GRpcStatus(status)
            if matches!(
                status.code(),
                tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
            ) =>
        {
            RegistryError::permission_denied(format!("etcd watch authorization failed: {status}"))
        }
        etcd_client::Error::GRpcStatus(status)
            if status.code() == tonic::Code::ResourceExhausted =>
        {
            RegistryError::resource_exhausted(format!("etcd watch resource exhausted: {status}"))
        }
        etcd_client::Error::InvalidArgs(_)
        | etcd_client::Error::InvalidUri(_)
        | etcd_client::Error::InvalidMetadataValue(_)
        | etcd_client::Error::EndpointsNotManaged => {
            RegistryError::validation(format!("etcd watch configuration is invalid: {error}"))
        }
        _ => RegistryError::network(format!("etcd watch failed: {error}")),
    }
}

fn decode(value: &str, label: &str) -> Result<Vec<u8>, RegistryError> {
    STANDARD
        .decode(value)
        .map_err(|_| RegistryError::validation(format!("{label} is not valid base64")))
}

#[cfg(test)]
mod tests {
    use nacos_sdk::api::config::{ConfigChangeListener, ConfigResponse};
    use tokio::sync::watch;
    use zookeeper_client::{EventType, WatchedEvent};

    use super::{
        LatestNacosListener, WatchChangeKind, etcd_watch_error, nacos_reconciled_change,
        retryable_etcd_watch_error, zookeeper_change,
    };
    use crate::registry::{RegistryErrorCode, ResourceAddress};

    #[test]
    fn zookeeper_events_map_without_reading_node_data() {
        let changed = WatchedEvent::new(EventType::NodeDataChanged, "/app/config");
        let children = WatchedEvent::new(EventType::NodeChildrenChanged, "/app");

        assert_eq!(zookeeper_change(&changed), Some(WatchChangeKind::Updated));
        assert_eq!(
            zookeeper_change(&children),
            Some(WatchChangeKind::ChildrenChanged)
        );
    }

    #[test]
    fn nacos_listener_retains_only_address_and_md5() {
        let (latest, mut changes) = watch::channel(None);
        let listener = LatestNacosListener {
            address: ResourceAddress::NacosConfig {
                group: "DEFAULT_GROUP".to_owned(),
                data_id: "application.yaml".to_owned(),
            },
            latest,
        };
        listener.notify(ConfigResponse::new(
            "application.yaml".to_owned(),
            "DEFAULT_GROUP".to_owned(),
            "public".to_owned(),
            "password: must-not-cross-the-watch-boundary".to_owned(),
            "yaml".to_owned(),
            "safe-md5".to_owned(),
        ));

        let change = changes.borrow_and_update().clone().unwrap();
        assert_eq!(change.version.as_deref(), Some("safe-md5"));
        let debug = serde_json::to_string(&change.address).unwrap();
        assert!(!debug.contains("must-not-cross"));
    }

    #[test]
    fn nacos_reconciliation_deduplicates_versions_and_detects_deletion() {
        assert_eq!(
            nacos_reconciled_change(Some("old"), Some("new")),
            Some(WatchChangeKind::Updated)
        );
        assert_eq!(nacos_reconciled_change(Some("same"), Some("same")), None);
        assert_eq!(
            nacos_reconciled_change(Some("old"), None),
            Some(WatchChangeKind::Deleted)
        );
        assert_eq!(nacos_reconciled_change(None, None), None);
    }

    #[test]
    fn etcd_watch_retries_transport_failures_but_stops_on_authentication_failure() {
        let unavailable = etcd_client::Error::GRpcStatus(tonic::Status::unavailable("offline"));
        assert!(retryable_etcd_watch_error(&unavailable));

        let unauthenticated =
            etcd_client::Error::GRpcStatus(tonic::Status::unauthenticated("expired token"));
        assert!(!retryable_etcd_watch_error(&unauthenticated));
        assert_eq!(
            etcd_watch_error(unauthenticated).code,
            RegistryErrorCode::PermissionDenied
        );
    }
}
