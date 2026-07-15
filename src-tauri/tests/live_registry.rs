use atlas_registry_lib::{
    credentials::ConnectionSecret,
    registry::{
        AdapterId, AuthenticationMode, ConnectionAuth, ConnectionProfile, MutationValue,
        NacosApiVersion, RegistryService, ResourceAddress, ResourceMutation, SubscriptionId,
        TlsProfile, ValueEncoding, WatchEvent, WatchRequest, WatchStatusState,
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn profile(adapter: AdapterId, endpoint: String) -> ConnectionProfile {
    ConnectionProfile {
        id: format!("live-{adapter:?}").to_lowercase(),
        name: format!("Live {adapter:?}"),
        adapter,
        endpoint,
        namespace: String::new(),
        nacos_api_version: NacosApiVersion::V2,
        environment: Default::default(),
        auth: Default::default(),
        tls: Default::default(),
    }
}

fn secured_profile(
    adapter: AdapterId,
    endpoint: String,
) -> (ConnectionProfile, Option<ConnectionSecret>) {
    let mut profile = profile(adapter, endpoint);
    let prefix = match adapter {
        AdapterId::Etcd => "ATLAS_TEST_ETCD",
        AdapterId::Zookeeper => "ATLAS_TEST_ZOOKEEPER",
        AdapterId::Nacos => "ATLAS_TEST_NACOS",
    };
    let username = std::env::var(format!("{prefix}_USERNAME")).ok();
    let secret = username.map(|username| {
        profile.auth = ConnectionAuth {
            mode: if adapter == AdapterId::Zookeeper {
                AuthenticationMode::Digest
            } else {
                AuthenticationMode::UsernamePassword
            },
            username,
            custom_key: String::new(),
        };
        ConnectionSecret::new(
            std::env::var(format!("{prefix}_PASSWORD"))
                .unwrap_or_else(|_| panic!("set {prefix}_PASSWORD with {prefix}_USERNAME")),
        )
    });
    if std::env::var(format!("{prefix}_TLS")).as_deref() == Ok("1") {
        profile.tls = TlsProfile {
            enabled: true,
            ca_certificate_path: std::env::var(format!("{prefix}_TLS_CA")).unwrap_or_default(),
            client_certificate_path: std::env::var(format!("{prefix}_TLS_CERT"))
                .unwrap_or_default(),
            client_key_path: std::env::var(format!("{prefix}_TLS_KEY")).unwrap_or_default(),
            server_name: std::env::var(format!("{prefix}_TLS_SERVER_NAME")).unwrap_or_default(),
        };
    }
    (profile, secret)
}

fn mutations_enabled() -> bool {
    std::env::var("ATLAS_TEST_ENABLE_MUTATIONS").as_deref() == Ok("1")
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    )
}

fn text_value(content: &str) -> MutationValue {
    MutationValue {
        content: content.to_owned(),
        encoding: ValueEncoding::Utf8,
    }
}

#[test]
#[ignore = "requires ATLAS_TEST_ETCD_ENDPOINT"]
fn etcd_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ETCD_ENDPOINT")
        .expect("set ATLAS_TEST_ETCD_ENDPOINT before running ignored tests");
    let service = RegistryService::default();
    let (connection, secret) = secured_profile(AdapterId::Etcd, endpoint);

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("etcd session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("etcd root should be listable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same etcd session should be reusable");
        if let Ok(key) = std::env::var("ATLAS_TEST_ETCD_KEY") {
            let document = service
                .read(
                    &session.id,
                    ResourceAddress::Etcd {
                        key_base64: STANDARD.encode(key),
                    },
                )
                .await
                .expect("configured etcd fixture should be readable");
            assert!(document.metadata.contains_key("modRevision"));
        }
        if mutations_enabled() {
            let prefix = std::env::var("ATLAS_TEST_ETCD_MUTATION_PREFIX")
                .unwrap_or_else(|_| "/atlas-registry-live-test".to_owned());
            let address = ResourceAddress::Etcd {
                key_base64: STANDARD.encode(format!("{prefix}/{}", unique_suffix())),
            };
            assert_create_update_delete(&service, &session.id, address).await;
        }
    });
}

#[test]
#[ignore = "requires ATLAS_TEST_ZOOKEEPER_ENDPOINT"]
fn zookeeper_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ZOOKEEPER_ENDPOINT")
        .expect("set ATLAS_TEST_ZOOKEEPER_ENDPOINT before running ignored tests");
    let service = RegistryService::default();
    let (connection, secret) = secured_profile(AdapterId::Zookeeper, endpoint);

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("ZooKeeper session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("ZooKeeper root should be listable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same ZooKeeper session should be reusable");
        if let Ok(path) = std::env::var("ATLAS_TEST_ZOOKEEPER_PATH") {
            let document = service
                .read(&session.id, ResourceAddress::Zookeeper { path })
                .await
                .expect("configured ZooKeeper fixture should be readable");
            assert!(document.metadata.contains_key("modifiedZxid"));
        }
        if mutations_enabled() {
            let parent = std::env::var("ATLAS_TEST_ZOOKEEPER_MUTATION_PARENT")
                .unwrap_or_else(|_| "/".to_owned());
            let address = ResourceAddress::Zookeeper {
                path: format!(
                    "{}/atlas-registry-live-test-{}",
                    parent.trim_end_matches('/'),
                    unique_suffix()
                ),
            };
            assert_create_update_delete(&service, &session.id, address).await;
        }
    });
}

#[test]
#[ignore = "requires ATLAS_TEST_NACOS_ENDPOINT"]
fn nacos_live_session_can_browse_the_config_list() {
    let endpoint = std::env::var("ATLAS_TEST_NACOS_ENDPOINT")
        .expect("set ATLAS_TEST_NACOS_ENDPOINT before running ignored tests");
    let version = match std::env::var("ATLAS_TEST_NACOS_VERSION").as_deref() {
        Ok("3") | Ok("v3") => NacosApiVersion::V3,
        _ => NacosApiVersion::V2,
    };
    let service = RegistryService::default();
    let (mut connection, secret) = secured_profile(AdapterId::Nacos, endpoint);
    connection.nacos_api_version = version;
    connection.namespace = std::env::var("ATLAS_TEST_NACOS_NAMESPACE").unwrap_or_default();

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("Nacos session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("Nacos config list should be browsable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same Nacos session should be reusable");
        if let (Ok(group), Ok(data_id)) = (
            std::env::var("ATLAS_TEST_NACOS_GROUP"),
            std::env::var("ATLAS_TEST_NACOS_DATA_ID"),
        ) {
            let document = service
                .read(&session.id, ResourceAddress::NacosConfig { group, data_id })
                .await
                .expect("configured Nacos fixture should be readable");
            assert!(document.metadata.contains_key("md5"));
        }
        if mutations_enabled() {
            let group = std::env::var("ATLAS_TEST_NACOS_MUTATION_GROUP")
                .unwrap_or_else(|_| "ATLAS_REGISTRY_TEST".to_owned());
            let address = ResourceAddress::NacosConfig {
                group,
                data_id: format!("atlas-registry-live-test-{}", unique_suffix()),
            };
            assert_create_update_delete(&service, &session.id, address).await;
        }
    });
}

async fn assert_create_update_delete(
    service: &RegistryService,
    connection_id: &str,
    address: ResourceAddress,
) {
    let created = service
        .mutate(
            connection_id,
            ResourceMutation::Create {
                address: address.clone(),
                value: text_value("atlas-live-create"),
                content_type: Some("text".to_owned()),
            },
        )
        .await
        .expect("test resource should be created");
    let created_version = created
        .current
        .and_then(|snapshot| snapshot.version)
        .expect("create should return a version");

    let subscription_id = SubscriptionId::new(format!("live-watch-{}", unique_suffix())).unwrap();
    let mut watch_events = service
        .start_watch(
            subscription_id.clone(),
            connection_id.to_owned(),
            WatchRequest {
                address: address.clone(),
                start_version: matches!(&address, ResourceAddress::Etcd { .. })
                    .then(|| created_version.clone()),
            },
        )
        .await
        .expect("live resource watch should start");
    wait_for_live_watch(&mut watch_events).await;
    drain_initial_watch_events(&mut watch_events).await;

    let stale_version = match &address {
        ResourceAddress::NacosConfig { .. } => "stale-md5".to_owned(),
        _ => (created_version
            .parse::<i64>()
            .expect("numeric protocol version")
            + 1)
        .to_string(),
    };
    let conflict = service
        .mutate(
            connection_id,
            ResourceMutation::Update {
                address: address.clone(),
                value: text_value("must-not-overwrite"),
                content_type: Some("text".to_owned()),
                expected_version: stale_version,
            },
        )
        .await
        .expect_err("stale update must be rejected");
    assert_eq!(
        conflict.code,
        atlas_registry_lib::registry::RegistryErrorCode::Conflict
    );

    let updated = service
        .mutate(
            connection_id,
            ResourceMutation::Update {
                address: address.clone(),
                value: text_value("atlas-live-update"),
                content_type: Some("text".to_owned()),
                expected_version: created_version,
            },
        )
        .await
        .expect("test resource should be conditionally updated");
    wait_for_change(&mut watch_events).await;
    let updated_version = updated
        .current
        .and_then(|snapshot| snapshot.version)
        .expect("update should return a version");
    let document = service
        .read(connection_id, address.clone())
        .await
        .expect("updated resource should be readable");
    assert_eq!(document.value.content, "atlas-live-update");

    service
        .mutate(
            connection_id,
            ResourceMutation::Delete {
                address: address.clone(),
                expected_version: updated_version,
            },
        )
        .await
        .expect("test resource should be conditionally deleted");
    assert!(service.stop_watch(&subscription_id).await);
    let missing = service
        .read(connection_id, address)
        .await
        .expect_err("deleted resource should not be readable");
    assert_eq!(
        missing.code,
        atlas_registry_lib::registry::RegistryErrorCode::NotFound
    );
}

async fn wait_for_live_watch(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events
                .recv()
                .await
                .expect("watch event channel should remain open")
            {
                WatchEvent::Status {
                    state: WatchStatusState::Live,
                    ..
                } => return,
                WatchEvent::Status {
                    state: WatchStatusState::Failed,
                    message,
                    ..
                } => panic!("live watch failed: {message:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("watch should become live within 10 seconds");
}

async fn drain_initial_watch_events(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    while matches!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv()).await,
        Ok(Some(_))
    ) {}
}

async fn wait_for_change(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events
                .recv()
                .await
                .expect("watch event channel should remain open")
            {
                WatchEvent::Change { .. } => return,
                WatchEvent::Status {
                    state: WatchStatusState::Failed,
                    message,
                    ..
                } => panic!("live watch failed: {message:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("updated resource should emit a watch event within 10 seconds");
}
