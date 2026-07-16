use std::collections::{BTreeMap, HashMap};

use nacos_sdk::api::naming::ServiceInstance;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    MutationConsistency, MutationPhase, NacosApiVersion, NacosInstance, NacosNamespace,
    NacosNativeAction, NacosNativeActionResult, NacosService, NacosServicePage, RegistryError,
    RegistryErrorCode, adapters::NacosSession,
};

const MAX_NAMESPACES: usize = 1_000;
const MAX_INSTANCES: usize = 5_000;
const MAX_READ_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const CONFIRM_ATTEMPTS: usize = 20;
const CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    message: Option<String>,
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceWire {
    #[serde(alias = "namespaceId")]
    namespace: String,
    #[serde(default, alias = "namespaceName")]
    namespace_show_name: String,
    #[serde(default)]
    namespace_desc: String,
    #[serde(default)]
    config_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2ServiceList {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    services: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V3ServicePage {
    #[serde(default)]
    page_number: usize,
    #[serde(default)]
    pages_available: usize,
    #[serde(default)]
    page_items: Vec<ServiceSummaryWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceSummaryWire {
    name: String,
    #[serde(default = "default_group")]
    group_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceWire {
    #[serde(default, alias = "namespace")]
    namespace_id: String,
    #[serde(default = "default_group")]
    group_name: String,
    service_name: String,
    #[serde(default)]
    protect_threshold: f64,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2InstanceList {
    #[serde(default)]
    hosts: Vec<InstanceWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceWire {
    #[serde(default)]
    service_name: String,
    #[serde(default = "default_cluster")]
    cluster_name: String,
    ip: String,
    port: u16,
    #[serde(default = "default_weight")]
    weight: f64,
    #[serde(default)]
    healthy: bool,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

pub(super) async fn list_namespaces(
    session: &NacosSession,
) -> Result<Vec<NacosNamespace>, RegistryError> {
    let path = match session.api_version() {
        NacosApiVersion::V2 => "/nacos/v1/console/namespaces",
        NacosApiVersion::V3 => "/nacos/v3/admin/core/namespace/list",
    };
    let wires: Vec<NamespaceWire> =
        send_read(session.native_request(Method::GET, path), "namespace list").await?;
    if wires.len() > MAX_NAMESPACES {
        return Err(RegistryError::invalid_response(format!(
            "Nacos namespace response exceeds the {MAX_NAMESPACES}-item safety limit"
        )));
    }
    Ok(wires.into_iter().map(namespace_from_wire).collect())
}

pub(super) async fn list_services(
    session: &NacosSession,
    group: String,
    cursor: Option<String>,
    limit: usize,
) -> Result<NacosServicePage, RegistryError> {
    let page = parse_page(cursor)?;
    let namespace_id = session.namespace_id().to_owned();
    match session.api_version() {
        NacosApiVersion::V2 => {
            let data: V2ServiceList = send_read(
                session
                    .native_request(Method::GET, "/nacos/v2/ns/service/list")
                    .query(&[
                        ("namespaceId", namespace_id.clone()),
                        ("groupName", group.clone()),
                        ("pageNo", page.to_string()),
                        ("pageSize", limit.to_string()),
                    ]),
                "service list",
            )
            .await?;
            let consumed = page.saturating_mul(limit);
            let next_cursor = (consumed < data.count).then(|| (page + 1).to_string());
            let items = data
                .services
                .into_iter()
                .map(|name| service_summary(&namespace_id, &group, name))
                .collect();
            Ok(NacosServicePage { items, next_cursor })
        }
        NacosApiVersion::V3 => {
            let data: V3ServicePage = send_read(
                session
                    .native_request(Method::GET, "/nacos/v3/admin/ns/service/list")
                    .query(&[
                        ("namespaceId", namespace_id.clone()),
                        ("groupNameParam", group),
                        ("pageNo", page.to_string()),
                        ("pageSize", limit.to_string()),
                        ("withInstances", "false".to_owned()),
                    ]),
                "service list",
            )
            .await?;
            let next_cursor = (data.page_number < data.pages_available)
                .then(|| (data.page_number + 1).to_string());
            let items = data
                .page_items
                .into_iter()
                .map(|wire| service_summary(&namespace_id, &wire.group_name, wire.name))
                .collect();
            Ok(NacosServicePage { items, next_cursor })
        }
    }
}

pub(super) async fn read_service(
    session: &NacosSession,
    group: &str,
    service_name: &str,
) -> Result<NacosService, RegistryError> {
    let path = match session.api_version() {
        NacosApiVersion::V2 => "/nacos/v2/ns/service",
        NacosApiVersion::V3 => "/nacos/v3/admin/ns/service",
    };
    let wire: ServiceWire = send_read(
        session.native_request(Method::GET, path).query(&[
            ("namespaceId", session.namespace_id()),
            ("groupName", group),
            ("serviceName", service_name),
        ]),
        "service detail",
    )
    .await?;
    Ok(service_from_wire(session.namespace_id(), group, wire))
}

pub(super) async fn list_instances(
    session: &NacosSession,
    group: &str,
    service_name: &str,
) -> Result<Vec<NacosInstance>, RegistryError> {
    let common = [
        ("namespaceId", session.namespace_id()),
        ("groupName", group),
        ("serviceName", service_name),
    ];
    let mut wires = match session.api_version() {
        NacosApiVersion::V2 => {
            let data: V2InstanceList = send_read(
                session
                    .native_request(Method::GET, "/nacos/v2/ns/instance/list")
                    .query(&common),
                "instance list",
            )
            .await?;
            data.hosts
        }
        NacosApiVersion::V3 => {
            send_read(
                session
                    .native_request(Method::GET, "/nacos/v3/admin/ns/instance/list")
                    .query(&common),
                "instance list",
            )
            .await?
        }
    };
    if session.api_version() == NacosApiVersion::V2 {
        let service_ephemeral = read_service(session, group, service_name).await?.ephemeral;
        for wire in &mut wires {
            wire.ephemeral = service_ephemeral;
        }
    }
    if wires.len() > MAX_INSTANCES {
        return Err(RegistryError::invalid_response(format!(
            "Nacos instance response exceeds the {MAX_INSTANCES}-item safety limit"
        )));
    }
    Ok(wires
        .into_iter()
        .map(|wire| instance_from_wire(session.namespace_id(), group, service_name, wire))
        .collect())
}

pub(super) async fn execute_action(
    session: &NacosSession,
    mut action: NacosNativeAction,
    phase: &MutationPhase,
) -> Result<NacosNativeActionResult, RegistryError> {
    action.validate()?;
    preflight(session, &action).await?;
    let operation = action.operation();
    let target = action_target(&action);
    if execute_ephemeral_instance_action(session, &action, phase, &target).await? {
        confirm_eventually(session, &action)
            .await
            .map_err(|error| {
                RegistryError::mutation_outcome_unknown(format!(
                    "Nacos accepted {target}, but the result could not be confirmed: {}",
                    error.message
                ))
            })?;
        return Ok(NacosNativeActionResult {
            operation,
            target,
            consistency: MutationConsistency::CheckedBeforeMutation,
        });
    }
    let (method, path, form, query) = action_request(session, &action)?;
    phase.mark_dispatched();
    let mut request = session.native_request(method, path);
    if !form.is_empty() {
        request = request.form(&form);
    }
    if !query.is_empty() {
        request = request.query(&query);
    }
    send_write(request, &target).await?;
    confirm_eventually(session, &action)
        .await
        .map_err(|error| {
            RegistryError::mutation_outcome_unknown(format!(
                "Nacos accepted {target}, but the result could not be confirmed: {}",
                error.message
            ))
        })?;
    Ok(NacosNativeActionResult {
        operation,
        target,
        consistency: MutationConsistency::CheckedBeforeMutation,
    })
}

async fn confirm_eventually(
    session: &NacosSession,
    action: &NacosNativeAction,
) -> Result<(), RegistryError> {
    for attempt in 0..CONFIRM_ATTEMPTS {
        match confirm(session, action).await {
            Ok(()) => return Ok(()),
            Err(error) if confirmation_retryable(&error) && attempt + 1 < CONFIRM_ATTEMPTS => {
                tokio::time::sleep(CONFIRM_DELAY).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the confirmation loop always returns on its final attempt")
}

fn confirmation_retryable(error: &RegistryError) -> bool {
    matches!(
        error.code,
        RegistryErrorCode::NotFound | RegistryErrorCode::Conflict
    )
}

async fn preflight(
    session: &NacosSession,
    action: &NacosNativeAction,
) -> Result<(), RegistryError> {
    match action {
        NacosNativeAction::CreateNamespace { namespace_id, .. } => {
            if list_namespaces(session)
                .await?
                .iter()
                .any(|item| item.id == *namespace_id)
            {
                return Err(RegistryError::conflict("Nacos namespace already exists"));
            }
        }
        NacosNativeAction::UpdateNamespace {
            namespace_id,
            expected_fingerprint,
            ..
        }
        | NacosNativeAction::DeleteNamespace {
            namespace_id,
            expected_fingerprint,
        } => {
            let current = list_namespaces(session)
                .await?
                .into_iter()
                .find(|item| item.id == *namespace_id)
                .ok_or_else(|| RegistryError::not_found("Nacos namespace does not exist"))?;
            ensure_fingerprint(expected_fingerprint, &current.fingerprint, "namespace")?;
        }
        NacosNativeAction::CreateService {
            group,
            service_name,
            ..
        } => match read_service(session, group, service_name).await {
            Ok(_) => return Err(RegistryError::conflict("Nacos service already exists")),
            Err(error) if error.code == RegistryErrorCode::NotFound => {}
            Err(error) => return Err(error),
        },
        NacosNativeAction::UpdateService {
            group,
            service_name,
            expected_fingerprint,
            ..
        }
        | NacosNativeAction::DeleteService {
            group,
            service_name,
            expected_fingerprint,
        } => {
            let current = read_service(session, group, service_name).await?;
            ensure_fingerprint(expected_fingerprint, &current.fingerprint, "service")?;
        }
        NacosNativeAction::RegisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ephemeral,
            ..
        } => {
            let service = read_service(session, group, service_name).await?;
            ensure_service_instance_lifetime(&service, *ephemeral)?;
            if find_instance(session, group, service_name, cluster, ip, *port)
                .await?
                .is_some()
            {
                return Err(RegistryError::conflict("Nacos instance already exists"));
            }
        }
        NacosNativeAction::UpdateInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ephemeral,
            expected_fingerprint,
            ..
        } => {
            let current = find_instance(session, group, service_name, cluster, ip, *port)
                .await?
                .ok_or_else(|| RegistryError::not_found("Nacos instance does not exist"))?;
            ensure_fingerprint(expected_fingerprint, &current.fingerprint, "instance")?;
            ensure_instance_lifetime(ephemeral, &current)?;
            let service = read_service(session, group, service_name).await?;
            ensure_service_instance_lifetime(&service, *ephemeral)?;
        }
        NacosNativeAction::DeregisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ephemeral,
            expected_fingerprint,
            ..
        } => {
            let current = find_instance(session, group, service_name, cluster, ip, *port)
                .await?
                .ok_or_else(|| RegistryError::not_found("Nacos instance does not exist"))?;
            ensure_fingerprint(expected_fingerprint, &current.fingerprint, "instance")?;
            ensure_instance_lifetime(ephemeral, &current)?;
            let service = read_service(session, group, service_name).await?;
            ensure_service_instance_lifetime(&service, *ephemeral)?;
        }
    }
    Ok(())
}

fn ensure_instance_lifetime(
    requested_ephemeral: &bool,
    current: &NacosInstance,
) -> Result<(), RegistryError> {
    if current.ephemeral != *requested_ephemeral {
        Err(RegistryError::conflict(
            "Nacos instance lifetime changed after it was loaded; refresh before continuing",
        ))
    } else {
        Ok(())
    }
}

fn ensure_service_instance_lifetime(
    service: &NacosService,
    instance_ephemeral: bool,
) -> Result<(), RegistryError> {
    if service.ephemeral != instance_ephemeral {
        Err(RegistryError::conflict(
            "Nacos service and instance lifetimes must match; refresh the service or choose the matching instance type",
        ))
    } else {
        Ok(())
    }
}

async fn execute_ephemeral_instance_action(
    session: &NacosSession,
    action: &NacosNativeAction,
    phase: &MutationPhase,
    target: &str,
) -> Result<bool, RegistryError> {
    let request = match action {
        NacosNativeAction::RegisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral: true,
            metadata,
        }
        | NacosNativeAction::UpdateInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral: true,
            metadata,
            ..
        } => Some((
            false,
            group.clone(),
            service_name.clone(),
            ServiceInstance {
                instance_id: None,
                ip: ip.clone(),
                port: i32::from(*port),
                weight: *weight,
                healthy: true,
                enabled: *enabled,
                ephemeral: true,
                cluster_name: Some(cluster.clone()),
                service_name: Some(service_name.clone()),
                metadata: metadata
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<HashMap<_, _>>(),
            },
        )),
        NacosNativeAction::DeregisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ephemeral: true,
            ..
        } => Some((
            true,
            group.clone(),
            service_name.clone(),
            ServiceInstance {
                instance_id: None,
                ip: ip.clone(),
                port: i32::from(*port),
                weight: 1.0,
                healthy: true,
                enabled: true,
                ephemeral: true,
                cluster_name: Some(cluster.clone()),
                service_name: Some(service_name.clone()),
                metadata: HashMap::new(),
            },
        )),
        _ => None,
    };
    let Some((deregister, group, service_name, instance)) = request else {
        return Ok(false);
    };

    phase.mark_dispatched();
    let result = if deregister {
        session
            .naming
            .deregister_instance(service_name, Some(group), instance)
            .await
    } else {
        session
            .naming
            .register_instance(service_name, Some(group), instance)
            .await
    };
    result.map_err(|error| {
        RegistryError::mutation_outcome_unknown(format!(
            "Nacos Naming SDK could not confirm {target}: {error}"
        ))
    })?;
    Ok(true)
}

async fn confirm(session: &NacosSession, action: &NacosNativeAction) -> Result<(), RegistryError> {
    match action {
        NacosNativeAction::CreateNamespace {
            namespace_id,
            name,
            description,
        }
        | NacosNativeAction::UpdateNamespace {
            namespace_id,
            name,
            description,
            ..
        } => {
            let current = list_namespaces(session)
                .await?
                .into_iter()
                .find(|item| item.id == *namespace_id)
                .ok_or_else(|| RegistryError::not_found("Nacos namespace result is not visible"))?;
            if current.name == *name && current.description == *description {
                Ok(())
            } else {
                Err(RegistryError::conflict(
                    "Nacos namespace result does not match the requested state",
                ))
            }
        }
        NacosNativeAction::DeleteNamespace { namespace_id, .. } => {
            if list_namespaces(session)
                .await?
                .iter()
                .any(|item| item.id == *namespace_id)
            {
                Err(RegistryError::conflict("Nacos namespace still exists"))
            } else {
                Ok(())
            }
        }
        NacosNativeAction::CreateService {
            group,
            service_name,
            protect_threshold,
            ephemeral,
            metadata,
        }
        | NacosNativeAction::UpdateService {
            group,
            service_name,
            protect_threshold,
            ephemeral,
            metadata,
            ..
        } => {
            let current = read_service(session, group, service_name).await?;
            if service_matches(&current, *protect_threshold, *ephemeral, metadata) {
                Ok(())
            } else {
                Err(RegistryError::conflict(
                    "Nacos service result does not match the requested state",
                ))
            }
        }
        NacosNativeAction::DeleteService {
            group,
            service_name,
            ..
        } => match read_service(session, group, service_name).await {
            Err(error) if error.code == RegistryErrorCode::NotFound => Ok(()),
            Ok(_) => Err(RegistryError::conflict("Nacos service still exists")),
            Err(error) => Err(error),
        },
        NacosNativeAction::RegisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral,
            metadata,
        }
        | NacosNativeAction::UpdateInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral,
            metadata,
            ..
        } => {
            let current = find_instance(session, group, service_name, cluster, ip, *port)
                .await?
                .ok_or_else(|| RegistryError::not_found("Nacos instance result is not visible"))?;
            if instance_matches(&current, *weight, *enabled, *ephemeral, metadata) {
                Ok(())
            } else {
                Err(RegistryError::conflict(format!(
                    "Nacos instance result does not match the requested state (weight: {}, enabled: {}, lifetime: {}, metadata: {})",
                    (current.weight - weight).abs() < f64::EPSILON,
                    current.enabled == *enabled,
                    current.ephemeral == *ephemeral,
                    current.metadata == *metadata,
                )))
            }
        }
        NacosNativeAction::DeregisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ..
        } => {
            if find_instance(session, group, service_name, cluster, ip, *port)
                .await?
                .is_some()
            {
                Err(RegistryError::conflict("Nacos instance still exists"))
            } else {
                Ok(())
            }
        }
    }
}

fn service_matches(
    current: &NacosService,
    protect_threshold: f64,
    ephemeral: bool,
    metadata: &BTreeMap<String, String>,
) -> bool {
    (current.protect_threshold - protect_threshold).abs() <= f64::EPSILON
        && current.ephemeral == ephemeral
        && current.metadata == *metadata
}

fn instance_matches(
    current: &NacosInstance,
    weight: f64,
    enabled: bool,
    ephemeral: bool,
    metadata: &BTreeMap<String, String>,
) -> bool {
    (current.weight - weight).abs() <= f64::EPSILON
        && current.enabled == enabled
        && current.ephemeral == ephemeral
        && current.metadata == *metadata
}

type ActionRequest = (
    Method,
    &'static str,
    Vec<(String, String)>,
    Vec<(String, String)>,
);

fn action_request(
    session: &NacosSession,
    action: &NacosNativeAction,
) -> Result<ActionRequest, RegistryError> {
    let namespace = session.namespace_id().to_owned();
    let v3 = session.api_version() == NacosApiVersion::V3;
    let request = match action {
        NacosNativeAction::CreateNamespace {
            namespace_id,
            name,
            description,
        } => (
            Method::POST,
            if v3 {
                "/nacos/v3/admin/core/namespace"
            } else {
                "/nacos/v1/console/namespaces"
            },
            vec![
                (
                    if v3 {
                        "namespaceId"
                    } else {
                        "customNamespaceId"
                    }
                    .to_owned(),
                    namespace_id.clone(),
                ),
                ("namespaceName".to_owned(), name.clone()),
                ("namespaceDesc".to_owned(), description.clone()),
            ],
            vec![],
        ),
        NacosNativeAction::UpdateNamespace {
            namespace_id,
            name,
            description,
            ..
        } => (
            Method::PUT,
            if v3 {
                "/nacos/v3/admin/core/namespace"
            } else {
                "/nacos/v1/console/namespaces"
            },
            vec![
                ("namespaceId".to_owned(), namespace_id.clone()),
                ("namespaceName".to_owned(), name.clone()),
                ("namespaceDesc".to_owned(), description.clone()),
            ],
            vec![],
        ),
        NacosNativeAction::DeleteNamespace { namespace_id, .. } => (
            Method::DELETE,
            if v3 {
                "/nacos/v3/admin/core/namespace"
            } else {
                "/nacos/v1/console/namespaces"
            },
            vec![],
            vec![("namespaceId".to_owned(), namespace_id.clone())],
        ),
        NacosNativeAction::CreateService {
            group,
            service_name,
            protect_threshold,
            ephemeral,
            metadata,
        }
        | NacosNativeAction::UpdateService {
            group,
            service_name,
            protect_threshold,
            ephemeral,
            metadata,
            ..
        } => (
            if matches!(action, NacosNativeAction::CreateService { .. }) {
                Method::POST
            } else {
                Method::PUT
            },
            if v3 {
                "/nacos/v3/admin/ns/service"
            } else {
                "/nacos/v2/ns/service"
            },
            vec![
                ("namespaceId".to_owned(), namespace),
                ("groupName".to_owned(), group.clone()),
                ("serviceName".to_owned(), service_name.clone()),
                ("protectThreshold".to_owned(), protect_threshold.to_string()),
                ("ephemeral".to_owned(), ephemeral.to_string()),
                ("metadata".to_owned(), metadata_json(metadata)?),
            ],
            vec![],
        ),
        NacosNativeAction::DeleteService {
            group,
            service_name,
            ..
        } => (
            Method::DELETE,
            if v3 {
                "/nacos/v3/admin/ns/service"
            } else {
                "/nacos/v2/ns/service"
            },
            vec![],
            vec![
                ("namespaceId".to_owned(), namespace),
                ("groupName".to_owned(), group.clone()),
                ("serviceName".to_owned(), service_name.clone()),
            ],
        ),
        NacosNativeAction::RegisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral,
            metadata,
        }
        | NacosNativeAction::UpdateInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            weight,
            enabled,
            ephemeral,
            metadata,
            ..
        } => (
            if matches!(action, NacosNativeAction::RegisterInstance { .. }) {
                Method::POST
            } else {
                Method::PUT
            },
            if v3 {
                "/nacos/v3/admin/ns/instance"
            } else {
                "/nacos/v2/ns/instance"
            },
            vec![
                ("namespaceId".to_owned(), namespace),
                ("groupName".to_owned(), group.clone()),
                ("serviceName".to_owned(), service_name.clone()),
                ("clusterName".to_owned(), cluster.clone()),
                ("ip".to_owned(), ip.clone()),
                ("port".to_owned(), port.to_string()),
                ("weight".to_owned(), weight.to_string()),
                ("enabled".to_owned(), enabled.to_string()),
                ("ephemeral".to_owned(), ephemeral.to_string()),
                ("metadata".to_owned(), metadata_json(metadata)?),
            ],
            vec![],
        ),
        NacosNativeAction::DeregisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ephemeral,
            ..
        } => (
            Method::DELETE,
            if v3 {
                "/nacos/v3/admin/ns/instance"
            } else {
                "/nacos/v2/ns/instance"
            },
            vec![],
            vec![
                ("namespaceId".to_owned(), namespace),
                ("groupName".to_owned(), group.clone()),
                ("serviceName".to_owned(), service_name.clone()),
                ("clusterName".to_owned(), cluster.clone()),
                ("ip".to_owned(), ip.clone()),
                ("port".to_owned(), port.to_string()),
                ("ephemeral".to_owned(), ephemeral.to_string()),
            ],
        ),
    };
    Ok(request)
}

async fn find_instance(
    session: &NacosSession,
    group: &str,
    service_name: &str,
    cluster: &str,
    ip: &str,
    port: u16,
) -> Result<Option<NacosInstance>, RegistryError> {
    let path = match session.api_version() {
        NacosApiVersion::V2 => "/nacos/v2/ns/instance",
        NacosApiVersion::V3 => "/nacos/v3/admin/ns/instance",
    };
    let response = send_read::<InstanceWire>(
        session.native_request(Method::GET, path).query(&[
            ("namespaceId", session.namespace_id()),
            ("groupName", group),
            ("serviceName", service_name),
            ("clusterName", cluster),
            ("ip", ip),
            ("port", &port.to_string()),
        ]),
        "instance detail",
    )
    .await;
    match response {
        Ok(mut wire) => {
            if session.api_version() == NacosApiVersion::V2 {
                wire.ephemeral = read_service(session, group, service_name).await?.ephemeral;
            }
            Ok(Some(instance_from_wire(
                session.namespace_id(),
                group,
                service_name,
                wire,
            )))
        }
        Err(error) if error.code == RegistryErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

async fn send_read<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    operation: &str,
) -> Result<T, RegistryError> {
    let response = request.send().await.map_err(http_read_error)?;
    let status = response.status();
    let body = read_bounded_response(response, operation, MAX_READ_RESPONSE_BYTES).await?;
    decode_read_response(status, &body, operation)
}

async fn read_bounded_response(
    mut response: reqwest::Response,
    operation: &str,
    limit: usize,
) -> Result<Vec<u8>, RegistryError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(RegistryError::resource_exhausted(format!(
            "Nacos {operation} response exceeds the {limit}-byte safety limit"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        RegistryError::invalid_response(format!(
            "cannot read Nacos {operation} response: {}",
            error.without_url()
        ))
    })? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(RegistryError::resource_exhausted(format!(
                "Nacos {operation} response exceeds the {limit}-byte safety limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn decode_read_response<T: DeserializeOwned>(
    status: StatusCode,
    body: &[u8],
    operation: &str,
) -> Result<T, RegistryError> {
    if status == StatusCode::NOT_FOUND {
        return Err(RegistryError::not_found(format!(
            "Nacos {operation} was not found"
        )));
    }
    let envelope: Envelope<Value> = serde_json::from_slice(body).map_err(|_| {
        if status.is_success() {
            RegistryError::invalid_response(format!("invalid Nacos {operation} response"))
        } else {
            RegistryError::network(format!(
                "Nacos {operation} returned HTTP {}",
                status.as_u16()
            ))
        }
    })?;
    if envelope.code != 0 && envelope.code != 200 {
        let message = envelope.message.as_deref().unwrap_or_default();
        let normalized = message.to_ascii_lowercase();
        if normalized.contains("not exist") || normalized.contains("not found") {
            return Err(RegistryError::not_found(format!(
                "Nacos {operation} does not exist"
            )));
        }
        return Err(RegistryError::new(
            RegistryErrorCode::Network,
            format!("Nacos rejected {operation}: {message}"),
            false,
        ));
    }
    if !status.is_success() {
        return Err(RegistryError::network(format!(
            "Nacos {operation} returned HTTP {}",
            status.as_u16()
        )));
    }
    serde_json::from_value(envelope.data.ok_or_else(|| {
        RegistryError::invalid_response(format!("Nacos {operation} response has no data"))
    })?)
    .map_err(|_| {
        RegistryError::invalid_response(format!("invalid Nacos {operation} response data"))
    })
}

async fn send_write(
    request: reqwest::RequestBuilder,
    operation: &str,
) -> Result<(), RegistryError> {
    let response = request.send().await.map_err(|error| {
        RegistryError::mutation_outcome_unknown(format!(
            "Nacos {operation} transport failed after dispatch: {}",
            error.without_url()
        ))
    })?;
    let body = response
        .error_for_status()
        .map_err(|error| {
            RegistryError::mutation_outcome_unknown(format!(
                "Nacos {operation} returned an HTTP error after dispatch: {}",
                error.without_url()
            ))
        })?
        .bytes()
        .await
        .map_err(|error| {
            RegistryError::mutation_outcome_unknown(format!(
                "Nacos {operation} response could not be read after dispatch: {}",
                error.without_url()
            ))
        })?;
    interpret_write_response(&body, operation).map_err(|error| {
        if error.code == RegistryErrorCode::InvalidResponse {
            RegistryError::mutation_outcome_unknown(format!(
                "Nacos {operation} returned an invalid response after dispatch: {}",
                error.message
            ))
        } else {
            error
        }
    })
}

fn interpret_write_response(body: &[u8], operation: &str) -> Result<(), RegistryError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        RegistryError::invalid_response("response is not a supported JSON write result")
    })?;
    match value {
        Value::Bool(true) => return Ok(()),
        Value::Bool(false) => {
            return Err(RegistryError::conflict(format!(
                "Nacos did not apply {operation}"
            )));
        }
        Value::String(value) if value.eq_ignore_ascii_case("true") => return Ok(()),
        Value::String(value) if value.eq_ignore_ascii_case("false") => {
            return Err(RegistryError::conflict(format!(
                "Nacos did not apply {operation}"
            )));
        }
        value if value.is_object() => {
            let envelope: Envelope<Value> = serde_json::from_value(value).map_err(|_| {
                RegistryError::invalid_response("response is not a valid Nacos write envelope")
            })?;
            if envelope.code != 0 && envelope.code != 200 {
                return Err(RegistryError::conflict(format!(
                    "Nacos rejected {operation}: {}",
                    envelope.message.as_deref().unwrap_or("unknown response")
                )));
            }
            return match envelope.data {
                Some(Value::Bool(false)) | None => Err(RegistryError::conflict(format!(
                    "Nacos did not apply {operation}"
                ))),
                _ => Ok(()),
            };
        }
        _ => {}
    }
    Err(RegistryError::invalid_response(
        "response is not a supported Nacos write result",
    ))
}

#[cfg(test)]
fn unwrap_envelope<T>(envelope: Envelope<T>, operation: &str) -> Result<T, RegistryError> {
    if envelope.code != 0 && envelope.code != 200 {
        let message = envelope.message.as_deref().unwrap_or_default();
        let normalized_message = message.to_ascii_lowercase();
        if normalized_message.contains("not exist") || normalized_message.contains("not found") {
            return Err(RegistryError::not_found(format!(
                "Nacos {operation} does not exist"
            )));
        }
        return Err(RegistryError::new(
            RegistryErrorCode::Network,
            format!("Nacos rejected {operation}: {message}"),
            false,
        ));
    }
    envelope.data.ok_or_else(|| {
        RegistryError::invalid_response(format!("Nacos {operation} response has no data"))
    })
}

fn namespace_from_wire(wire: NamespaceWire) -> NacosNamespace {
    let name = if wire.namespace_show_name.is_empty() {
        wire.namespace.clone()
    } else {
        wire.namespace_show_name
    };
    let fingerprint = fingerprint(&json!({
        "id": wire.namespace,
        "name": name,
        "description": wire.namespace_desc,
        "configCount": wire.config_count,
    }));
    NacosNamespace {
        id: wire.namespace,
        name,
        description: wire.namespace_desc,
        config_count: wire.config_count,
        fingerprint,
    }
}

fn service_summary(namespace: &str, group: &str, name: String) -> NacosService {
    let fingerprint = fingerprint(&json!({ "namespace": namespace, "group": group, "name": name }));
    NacosService {
        namespace_id: namespace.to_owned(),
        group: group.to_owned(),
        name,
        protect_threshold: 0.0,
        ephemeral: false,
        metadata: BTreeMap::new(),
        fingerprint,
    }
}

fn service_from_wire(namespace: &str, group: &str, wire: ServiceWire) -> NacosService {
    let namespace_id = if wire.namespace_id.is_empty() {
        namespace.to_owned()
    } else {
        wire.namespace_id
    };
    let group = if wire.group_name.is_empty() {
        group.to_owned()
    } else {
        wire.group_name
    };
    let fingerprint = fingerprint(&json!({
        "namespace": namespace_id,
        "group": group,
        "name": wire.service_name,
        "protectThreshold": wire.protect_threshold,
        "ephemeral": wire.ephemeral,
        "metadata": wire.metadata,
    }));
    NacosService {
        namespace_id,
        group,
        name: wire.service_name,
        protect_threshold: wire.protect_threshold,
        ephemeral: wire.ephemeral,
        metadata: wire.metadata,
        fingerprint,
    }
}

fn instance_from_wire(
    namespace: &str,
    group: &str,
    service_name: &str,
    wire: InstanceWire,
) -> NacosInstance {
    let normalized_service = wire
        .service_name
        .split_once("@@")
        .map(|(_, service)| service)
        .filter(|service| !service.is_empty())
        .unwrap_or(service_name)
        .to_owned();
    let fingerprint = fingerprint(&json!({
        "namespace": namespace,
        "group": group,
        "serviceName": normalized_service,
        "cluster": wire.cluster_name,
        "ip": wire.ip,
        "port": wire.port,
        "weight": wire.weight,
        "healthy": wire.healthy,
        "enabled": wire.enabled,
        "ephemeral": wire.ephemeral,
        "metadata": wire.metadata,
    }));
    NacosInstance {
        namespace_id: namespace.to_owned(),
        group: group.to_owned(),
        service_name: normalized_service,
        cluster: wire.cluster_name,
        ip: wire.ip,
        port: wire.port,
        weight: wire.weight,
        healthy: wire.healthy,
        enabled: wire.enabled,
        ephemeral: wire.ephemeral,
        metadata: wire.metadata,
        fingerprint,
    }
}

fn action_target(action: &NacosNativeAction) -> String {
    match action {
        NacosNativeAction::CreateNamespace { namespace_id, .. }
        | NacosNativeAction::UpdateNamespace { namespace_id, .. }
        | NacosNativeAction::DeleteNamespace { namespace_id, .. } => {
            format!("namespace:{namespace_id}")
        }
        NacosNativeAction::CreateService {
            group,
            service_name,
            ..
        }
        | NacosNativeAction::UpdateService {
            group,
            service_name,
            ..
        }
        | NacosNativeAction::DeleteService {
            group,
            service_name,
            ..
        } => format!("service:{group}@@{service_name}"),
        NacosNativeAction::RegisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ..
        }
        | NacosNativeAction::UpdateInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ..
        }
        | NacosNativeAction::DeregisterInstance {
            group,
            service_name,
            cluster,
            ip,
            port,
            ..
        } => {
            format!("instance:{group}@@{service_name}/{cluster}/{ip}:{port}")
        }
    }
}

fn ensure_fingerprint(expected: &str, current: &str, resource: &str) -> Result<(), RegistryError> {
    if expected.eq_ignore_ascii_case(current) {
        Ok(())
    } else {
        Err(RegistryError::conflict(format!(
            "Nacos {resource} changed after it was loaded; refresh before writing"
        )))
    }
}

fn metadata_json(metadata: &BTreeMap<String, String>) -> Result<String, RegistryError> {
    serde_json::to_string(metadata)
        .map_err(|error| RegistryError::validation(format!("invalid Nacos metadata: {error}")))
}

fn fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("serializing JSON value cannot fail");
    format!("{:x}", Sha256::digest(bytes))
}

fn parse_page(cursor: Option<String>) -> Result<usize, RegistryError> {
    match cursor {
        None => Ok(1),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|page| *page > 0)
            .ok_or_else(|| RegistryError::validation("Nacos service cursor is invalid")),
    }
}

fn http_read_error(error: reqwest::Error) -> RegistryError {
    RegistryError::network(format!(
        "Nacos native API request failed: {}",
        error.without_url()
    ))
}

fn default_group() -> String {
    "DEFAULT_GROUP".to_owned()
}

fn default_cluster() -> String {
    "DEFAULT".to_owned()
}

fn default_weight() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_change_when_service_or_instance_state_changes() {
        let first = fingerprint(&json!({ "weight": 1.0, "enabled": true }));
        let second = fingerprint(&json!({ "weight": 2.0, "enabled": true }));
        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
    }

    #[test]
    fn audit_targets_never_include_nacos_metadata_values() {
        let action = NacosNativeAction::CreateService {
            group: "DEFAULT_GROUP".to_owned(),
            service_name: "payments".to_owned(),
            protect_threshold: 0.5,
            ephemeral: false,
            metadata: BTreeMap::from([(
                "credential".to_owned(),
                "must-not-cross-the-audit-boundary".to_owned(),
            )]),
        };

        let target = action_target(&action);
        assert_eq!(target, "service:DEFAULT_GROUP@@payments");
        assert!(!target.contains("must-not-cross-the-audit-boundary"));
    }

    #[test]
    fn v2_instance_normalization_strips_group_prefix_without_losing_identity() {
        let item = instance_from_wire(
            "public",
            "DEFAULT_GROUP",
            "payments",
            InstanceWire {
                service_name: "DEFAULT_GROUP@@payments".to_owned(),
                cluster_name: "DEFAULT".to_owned(),
                ip: "127.0.0.1".to_owned(),
                port: 8080,
                weight: 1.0,
                healthy: true,
                enabled: true,
                ephemeral: false,
                metadata: BTreeMap::new(),
            },
        );
        assert_eq!(item.service_name, "payments");
        assert_eq!(item.fingerprint.len(), 64);
    }

    #[test]
    fn confirmation_requires_the_requested_service_and_instance_state() {
        let service = service_from_wire(
            "public",
            "DEFAULT_GROUP",
            ServiceWire {
                namespace_id: "public".to_owned(),
                group_name: "DEFAULT_GROUP".to_owned(),
                service_name: "payments".to_owned(),
                protect_threshold: 0.75,
                ephemeral: false,
                metadata: BTreeMap::from([("zone".to_owned(), "east".to_owned())]),
            },
        );
        assert!(service_matches(
            &service,
            0.75,
            false,
            &BTreeMap::from([("zone".to_owned(), "east".to_owned())])
        ));
        assert!(!service_matches(&service, 0.5, false, &service.metadata));
        ensure_service_instance_lifetime(&service, false)
            .expect("persistent services accept persistent instances");
        assert_eq!(
            ensure_service_instance_lifetime(&service, true)
                .expect_err("service and instance lifetimes must match")
                .code,
            RegistryErrorCode::Conflict
        );

        let instance = instance_from_wire(
            "public",
            "DEFAULT_GROUP",
            "payments",
            InstanceWire {
                service_name: "DEFAULT_GROUP@@payments".to_owned(),
                cluster_name: "DEFAULT".to_owned(),
                ip: "127.0.0.1".to_owned(),
                port: 8080,
                weight: 2.0,
                healthy: true,
                enabled: false,
                ephemeral: false,
                metadata: BTreeMap::new(),
            },
        );
        assert!(instance_matches(
            &instance,
            2.0,
            false,
            false,
            &BTreeMap::new()
        ));
        assert!(!instance_matches(
            &instance,
            2.0,
            true,
            false,
            &BTreeMap::new()
        ));

        let ephemeral = NacosInstance {
            ephemeral: true,
            ..instance
        };
        assert_eq!(
            ensure_instance_lifetime(&false, &ephemeral)
                .expect_err("instance lifetime cannot change during an update")
                .code,
            RegistryErrorCode::Conflict
        );
        ensure_instance_lifetime(&true, &ephemeral)
            .expect("an ephemeral update must preserve the instance lifetime");
    }

    #[test]
    fn v2_and_v3_native_envelopes_match_the_documented_page_shapes() {
        let v2: Envelope<V2ServiceList> = serde_json::from_value(json!({
            "code": 0,
            "message": null,
            "data": { "count": 2, "services": ["payments", "orders"] }
        }))
        .expect("v2 success envelopes may contain a null message");
        let v2 = unwrap_envelope(v2, "service list").expect("v2 envelope should unwrap");
        assert_eq!(v2.count, 2);
        assert_eq!(v2.services, ["payments", "orders"]);

        let v3: Envelope<V3ServicePage> = serde_json::from_value(json!({
            "code": 0,
            "message": "success",
            "data": {
                "pageNumber": 1,
                "pagesAvailable": 2,
                "pageItems": [{ "name": "payments", "groupName": "DEFAULT_GROUP" }]
            }
        }))
        .expect("v3 service envelope should deserialize");
        let v3 = unwrap_envelope(v3, "service list").expect("v3 envelope should unwrap");
        assert_eq!(v3.page_number, 1);
        assert_eq!(v3.pages_available, 2);
        assert_eq!(v3.page_items[0].name, "payments");

        let v3_instances: Envelope<Vec<InstanceWire>> = serde_json::from_value(json!({
            "code": 0,
            "message": "success",
            "data": [{
                "serviceName": "payments",
                "clusterName": "DEFAULT",
                "ip": "127.0.0.1",
                "port": 8080,
                "weight": 1.0,
                "healthy": true,
                "enabled": true,
                "ephemeral": false,
                "metadata": {}
            }]
        }))
        .expect("v3 instance envelope should deserialize");
        assert_eq!(
            unwrap_envelope(v3_instances, "instance list")
                .expect("v3 instance envelope should unwrap")
                .len(),
            1
        );
    }

    #[test]
    fn native_write_responses_accept_legacy_booleans_and_admin_envelopes() {
        interpret_write_response(br#"true"#, "legacy namespace create")
            .expect("v1 namespace writes return a bare boolean");
        interpret_write_response(
            br#"{"code":0,"message":null,"data":true}"#,
            "service create",
        )
        .expect("v2 and v3 writes return an envelope");

        assert_eq!(
            interpret_write_response(br#"false"#, "legacy namespace delete")
                .expect_err("false means the server did not apply the write")
                .code,
            RegistryErrorCode::Conflict
        );
    }

    #[test]
    fn http_400_service_not_exist_is_a_semantic_not_found() {
        let error = match decode_read_response::<ServiceWire>(
            StatusCode::BAD_REQUEST,
            br#"{"code":21008,"message":"service not exist","data":"service not found"}"#,
            "service detail",
        ) {
            Err(error) => error,
            Ok(_) => panic!("Nacos uses HTTP 400 for a missing v2 service"),
        };
        assert_eq!(error.code, RegistryErrorCode::NotFound);
        assert!(!error.retryable);
    }

    #[test]
    fn only_eventual_visibility_errors_are_retried_after_a_write() {
        assert!(confirmation_retryable(&RegistryError::not_found(
            "not visible"
        )));
        assert!(confirmation_retryable(&RegistryError::conflict(
            "stale state"
        )));
        assert!(!confirmation_retryable(&RegistryError::permission_denied(
            "denied"
        )));
    }
}
