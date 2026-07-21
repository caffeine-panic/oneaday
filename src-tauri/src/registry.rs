mod adapters;
mod mutations;
mod nacos_native;
mod watch;

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

use crate::credentials::ConnectionSecret;
use adapters::RegistrySession;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "lowercase")]
pub enum AdapterId {
    Etcd,
    Zookeeper,
    Nacos,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterStatus {
    Available,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    Probe,
    Browse,
    Search,
    Read,
    Watch,
    Create,
    Update,
    Delete,
    History,
    Lease,
    Transaction,
    Acl,
    Ephemeral,
    Namespace,
    Service,
    Instance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterDescriptor {
    pub id: AdapterId,
    pub status: AdapterStatus,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "lowercase")]
pub enum NacosApiVersion {
    #[default]
    V2,
    V3,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosNamespace {
    pub id: String,
    pub name: String,
    pub description: String,
    pub config_count: u64,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosService {
    pub namespace_id: String,
    pub group: String,
    pub name: String,
    pub protect_threshold: f64,
    pub ephemeral: bool,
    pub metadata: BTreeMap<String, String>,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosServicePage {
    pub items: Vec<NacosService>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosInstance {
    pub namespace_id: String,
    pub group: String,
    pub service_name: String,
    pub cluster: String,
    pub ip: String,
    pub port: u16,
    pub weight: f64,
    pub healthy: bool,
    pub enabled: bool,
    pub ephemeral: bool,
    pub metadata: BTreeMap<String, String>,
    pub fingerprint: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NacosNativeOperation {
    CreateNamespace,
    UpdateNamespace,
    DeleteNamespace,
    CreateService,
    UpdateService,
    DeleteService,
    RegisterInstance,
    UpdateInstance,
    DeregisterInstance,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NacosNativeAction {
    CreateNamespace {
        namespace_id: String,
        name: String,
        description: String,
    },
    UpdateNamespace {
        namespace_id: String,
        name: String,
        description: String,
        expected_fingerprint: String,
    },
    DeleteNamespace {
        namespace_id: String,
        expected_fingerprint: String,
    },
    CreateService {
        group: String,
        service_name: String,
        protect_threshold: f64,
        ephemeral: bool,
        metadata: BTreeMap<String, String>,
    },
    UpdateService {
        group: String,
        service_name: String,
        protect_threshold: f64,
        ephemeral: bool,
        metadata: BTreeMap<String, String>,
        expected_fingerprint: String,
    },
    DeleteService {
        group: String,
        service_name: String,
        expected_fingerprint: String,
    },
    RegisterInstance {
        group: String,
        service_name: String,
        cluster: String,
        ip: String,
        port: u16,
        weight: f64,
        enabled: bool,
        ephemeral: bool,
        metadata: BTreeMap<String, String>,
    },
    UpdateInstance {
        group: String,
        service_name: String,
        cluster: String,
        ip: String,
        port: u16,
        weight: f64,
        enabled: bool,
        ephemeral: bool,
        metadata: BTreeMap<String, String>,
        expected_fingerprint: String,
    },
    DeregisterInstance {
        group: String,
        service_name: String,
        cluster: String,
        ip: String,
        port: u16,
        #[serde(default)]
        ephemeral: bool,
        expected_fingerprint: String,
    },
}

impl NacosNativeAction {
    pub fn validate(&mut self) -> Result<(), RegistryError> {
        match self {
            Self::CreateNamespace {
                namespace_id,
                name,
                description,
            }
            | Self::UpdateNamespace {
                namespace_id,
                name,
                description,
                ..
            } => {
                normalize_nacos_identifier(namespace_id, "namespace id", 128)?;
                normalize_nacos_identifier(name, "namespace name", 128)?;
                normalize_nacos_description(description)?;
            }
            Self::DeleteNamespace { namespace_id, .. } => {
                normalize_nacos_identifier(namespace_id, "namespace id", 128)?;
            }
            Self::CreateService {
                group,
                service_name,
                protect_threshold,
                metadata,
                ..
            }
            | Self::UpdateService {
                group,
                service_name,
                protect_threshold,
                metadata,
                ..
            } => {
                validate_nacos_service(group, service_name)?;
                if !protect_threshold.is_finite() || !(0.0..=1.0).contains(protect_threshold) {
                    return Err(RegistryError::validation(
                        "Nacos service protect threshold must be between 0 and 1",
                    ));
                }
                validate_nacos_metadata(metadata)?;
            }
            Self::DeleteService {
                group,
                service_name,
                ..
            } => validate_nacos_service(group, service_name)?,
            Self::RegisterInstance {
                group,
                service_name,
                cluster,
                ip,
                port,
                weight,
                ephemeral: _,
                metadata,
                ..
            }
            | Self::UpdateInstance {
                group,
                service_name,
                cluster,
                ip,
                port,
                weight,
                ephemeral: _,
                metadata,
                ..
            } => {
                validate_nacos_instance(
                    group,
                    service_name,
                    cluster,
                    ip,
                    *port,
                    *weight,
                    metadata,
                )?;
            }
            Self::DeregisterInstance {
                group,
                service_name,
                cluster,
                ip,
                port,
                ..
            } => {
                validate_nacos_instance(
                    group,
                    service_name,
                    cluster,
                    ip,
                    *port,
                    1.0,
                    &BTreeMap::new(),
                )?;
            }
        }
        if let Some(fingerprint) = self.expected_fingerprint_mut() {
            *fingerprint = fingerprint.trim().to_owned();
            if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(RegistryError::validation(
                    "Nacos native conditional mutation requires a SHA-256 fingerprint",
                ));
            }
        }
        if matches!(
            self,
            Self::DeleteNamespace { namespace_id, .. } if namespace_id == "public"
        ) {
            return Err(RegistryError::validation(
                "the Nacos public namespace cannot be deleted",
            ));
        }
        Ok(())
    }

    pub fn operation(&self) -> NacosNativeOperation {
        match self {
            Self::CreateNamespace { .. } => NacosNativeOperation::CreateNamespace,
            Self::UpdateNamespace { .. } => NacosNativeOperation::UpdateNamespace,
            Self::DeleteNamespace { .. } => NacosNativeOperation::DeleteNamespace,
            Self::CreateService { .. } => NacosNativeOperation::CreateService,
            Self::UpdateService { .. } => NacosNativeOperation::UpdateService,
            Self::DeleteService { .. } => NacosNativeOperation::DeleteService,
            Self::RegisterInstance { .. } => NacosNativeOperation::RegisterInstance,
            Self::UpdateInstance { .. } => NacosNativeOperation::UpdateInstance,
            Self::DeregisterInstance { .. } => NacosNativeOperation::DeregisterInstance,
        }
    }

    pub fn expected_fingerprint(&self) -> Option<&str> {
        match self {
            Self::UpdateNamespace {
                expected_fingerprint,
                ..
            }
            | Self::DeleteNamespace {
                expected_fingerprint,
                ..
            }
            | Self::UpdateService {
                expected_fingerprint,
                ..
            }
            | Self::DeleteService {
                expected_fingerprint,
                ..
            }
            | Self::UpdateInstance {
                expected_fingerprint,
                ..
            }
            | Self::DeregisterInstance {
                expected_fingerprint,
                ..
            } => Some(expected_fingerprint),
            _ => None,
        }
    }

    fn expected_fingerprint_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::UpdateNamespace {
                expected_fingerprint,
                ..
            }
            | Self::DeleteNamespace {
                expected_fingerprint,
                ..
            }
            | Self::UpdateService {
                expected_fingerprint,
                ..
            }
            | Self::DeleteService {
                expected_fingerprint,
                ..
            }
            | Self::UpdateInstance {
                expected_fingerprint,
                ..
            }
            | Self::DeregisterInstance {
                expected_fingerprint,
                ..
            } => Some(expected_fingerprint),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NacosNativeActionResult {
    pub operation: NacosNativeOperation,
    pub target: String,
    pub consistency: MutationConsistency,
}

fn normalize_nacos_identifier(
    value: &mut String,
    label: &str,
    max: usize,
) -> Result<(), RegistryError> {
    *value = value.trim().to_owned();
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(RegistryError::validation(format!(
            "Nacos {label} must contain 1–{max} printable characters"
        )));
    }
    Ok(())
}

fn normalize_nacos_description(value: &mut String) -> Result<(), RegistryError> {
    *value = value.trim().to_owned();
    if value.len() > 1024 || value.chars().any(|character| character == '\0') {
        return Err(RegistryError::validation(
            "Nacos namespace description cannot exceed 1024 characters",
        ));
    }
    Ok(())
}

fn validate_nacos_service(
    group: &mut String,
    service_name: &mut String,
) -> Result<(), RegistryError> {
    normalize_nacos_identifier(group, "group", 128)?;
    normalize_nacos_identifier(service_name, "service name", 256)
}

fn validate_nacos_instance(
    group: &mut String,
    service_name: &mut String,
    cluster: &mut String,
    ip: &mut String,
    port: u16,
    weight: f64,
    metadata: &BTreeMap<String, String>,
) -> Result<(), RegistryError> {
    validate_nacos_service(group, service_name)?;
    normalize_nacos_identifier(cluster, "cluster", 128)?;
    *ip = ip.trim().to_owned();
    ip.parse::<std::net::IpAddr>()
        .map_err(|_| RegistryError::validation("Nacos instance IP is invalid"))?;
    if port == 0 {
        return Err(RegistryError::validation(
            "Nacos instance port must be between 1 and 65535",
        ));
    }
    if !weight.is_finite() || !(0.0..=10_000.0).contains(&weight) {
        return Err(RegistryError::validation(
            "Nacos instance weight must be between 0 and 10000",
        ));
    }
    validate_nacos_metadata(metadata)
}

fn validate_nacos_metadata(metadata: &BTreeMap<String, String>) -> Result<(), RegistryError> {
    if metadata.len() > 128 {
        return Err(RegistryError::validation(
            "Nacos metadata cannot exceed 128 entries",
        ));
    }
    let bytes = serde_json::to_vec(metadata)
        .map_err(|error| RegistryError::validation(format!("invalid Nacos metadata: {error}")))?;
    if bytes.len() > 16 * 1024 {
        return Err(RegistryError::validation(
            "Nacos metadata cannot exceed 16 KiB",
        ));
    }
    if metadata
        .iter()
        .any(|(key, value)| key.trim().is_empty() || key.contains('\0') || value.contains('\0'))
    {
        return Err(RegistryError::validation(
            "Nacos metadata keys must be non-empty and metadata cannot contain NUL",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ConnectionEnvironment {
    #[default]
    Unspecified,
    Development,
    Testing,
    Staging,
    Production,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub enum AuthenticationMode {
    #[default]
    None,
    UsernamePassword,
    Digest,
    Custom,
    MseAccessKey,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionAuth {
    #[serde(default)]
    pub mode: AuthenticationMode,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub custom_key: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TlsProfile {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ca_certificate_path: String,
    #[serde(default)]
    pub client_certificate_path: String,
    #[serde(default)]
    pub client_key_path: String,
    #[serde(default)]
    pub server_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub adapter: AdapterId,
    pub endpoint: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub nacos_api_version: NacosApiVersion,
    #[serde(default)]
    pub environment: ConnectionEnvironment,
    #[serde(default)]
    pub auth: ConnectionAuth,
    #[serde(default)]
    pub tls: TlsProfile,
}

impl ConnectionProfile {
    pub(crate) fn validate(&mut self) -> Result<(), RegistryError> {
        self.id = self.id.trim().to_owned();
        self.name = self.name.trim().to_owned();
        self.endpoint = self.endpoint.trim().to_owned();
        self.namespace = self.namespace.trim().to_owned();
        self.auth.username = self.auth.username.trim().to_owned();
        self.auth.custom_key = self.auth.custom_key.trim().to_owned();
        self.tls.ca_certificate_path = self.tls.ca_certificate_path.trim().to_owned();
        self.tls.client_certificate_path = self.tls.client_certificate_path.trim().to_owned();
        self.tls.client_key_path = self.tls.client_key_path.trim().to_owned();
        self.tls.server_name = self.tls.server_name.trim().to_owned();

        if self.id.is_empty() {
            return Err(RegistryError::validation("connection id cannot be blank"));
        }
        if self.name.is_empty() {
            return Err(RegistryError::validation("connection name cannot be blank"));
        }
        if self.endpoint.is_empty() {
            return Err(RegistryError::validation("endpoint cannot be blank"));
        }
        match (self.adapter, self.auth.mode) {
            (_, AuthenticationMode::None) => {}
            (AdapterId::Etcd | AdapterId::Nacos, AuthenticationMode::UsernamePassword)
            | (AdapterId::Zookeeper, AuthenticationMode::Digest) => {
                if self.auth.username.is_empty() {
                    return Err(RegistryError::validation(
                        "authenticated connections require a username",
                    ));
                }
            }
            (AdapterId::Nacos, AuthenticationMode::Custom) => {
                if self.auth.custom_key.is_empty() {
                    return Err(RegistryError::validation(
                        "custom Nacos authentication requires a context key",
                    ));
                }
            }
            (AdapterId::Nacos, AuthenticationMode::MseAccessKey) => {
                if self.auth.username.is_empty() {
                    return Err(RegistryError::validation(
                        "MSE authentication requires an AccessKey ID",
                    ));
                }
            }
            _ => {
                return Err(RegistryError::validation(
                    "authentication mode is not supported by this adapter",
                ));
            }
        }
        let has_client_certificate = !self.tls.client_certificate_path.is_empty();
        let has_client_key = !self.tls.client_key_path.is_empty();
        if has_client_certificate != has_client_key {
            return Err(RegistryError::validation(
                "TLS client certificate and private key paths must be configured together",
            ));
        }
        if self.tls.enabled
            && self.adapter == AdapterId::Zookeeper
            && self.tls.ca_certificate_path.is_empty()
        {
            return Err(RegistryError::validation(
                "ZooKeeper TLS requires a CA certificate path",
            ));
        }
        if self.adapter == AdapterId::Zookeeper && !self.tls.server_name.is_empty() {
            return Err(RegistryError::validation(
                "ZooKeeper derives the TLS server name from each endpoint; an override is not supported",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionSession {
    pub id: String,
    pub name: String,
    pub adapter: AdapterId,
    pub endpoint: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProbe {
    pub adapter: AdapterId,
    pub endpoint: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResourceAddress {
    Root,
    Etcd {
        #[serde(alias = "key_base64")]
        key_base64: String,
    },
    EtcdPrefix {
        #[serde(alias = "prefix_base64")]
        prefix_base64: String,
    },
    Zookeeper {
        path: String,
    },
    NacosConfig {
        group: String,
        #[serde(alias = "data_id")]
        data_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceNode {
    pub address: ResourceAddress,
    pub name: String,
    pub readable: bool,
    /// `None` means that the backend does not expose the child count without another request.
    pub has_children: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePage {
    pub parent: ResourceAddress,
    pub items: Vec<ResourceNode>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourcePageRequest {
    pub parent: ResourceAddress,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSearchRequest {
    pub scope: ResourceAddress,
    pub query: String,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

impl ResourceSearchRequest {
    fn validate(&mut self) -> Result<(), RegistryError> {
        self.query = self.query.trim().to_owned();
        if self.query.is_empty() {
            return Err(RegistryError::validation("search query cannot be blank"));
        }
        if self.query.len() > 256 {
            return Err(RegistryError::validation(
                "search query cannot exceed 256 bytes",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSearchPage {
    pub scope: ResourceAddress,
    pub items: Vec<ResourceNode>,
    pub next_cursor: Option<String>,
    /// Number of identifiers examined by this bounded request. Values are never read by search.
    pub scanned: usize,
    pub exhaustive: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceHistoryRequest {
    pub address: ResourceAddress,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

impl ResourceHistoryRequest {
    fn validate(&self) -> Result<(), RegistryError> {
        match &self.address {
            ResourceAddress::NacosConfig { group, data_id }
                if !group.trim().is_empty() && !data_id.trim().is_empty() =>
            {
                Ok(())
            }
            _ => Err(RegistryError::unsupported(
                "server resource history is currently available for Nacos configurations",
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceHistoryEntry {
    pub revision_id: String,
    pub address: ResourceAddress,
    pub md5: Option<String>,
    pub operation: Option<String>,
    pub source_user: Option<String>,
    pub source_ip: Option<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub publish_type: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceHistoryPage {
    pub address: ResourceAddress,
    pub items: Vec<ResourceHistoryEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceHistoryDocument {
    pub entry: ResourceHistoryEntry,
    pub value: EncodedValue,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZookeeperAclEntry {
    pub scheme: String,
    pub id: String,
    pub permissions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ZookeeperCreateMode {
    PersistentSequential,
    Ephemeral,
    EphemeralSequential,
}

impl ZookeeperCreateMode {
    pub fn is_sequential(self) -> bool {
        matches!(self, Self::PersistentSequential | Self::EphemeralSequential)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ZookeeperNativeAction {
    SetAcl {
        address: ResourceAddress,
        expected_acl_version: i32,
        entries: Vec<ZookeeperAclEntry>,
    },
    Create {
        address: ResourceAddress,
        value: MutationValue,
        mode: ZookeeperCreateMode,
    },
}

impl ZookeeperNativeAction {
    pub fn validate(&self) -> Result<(), RegistryError> {
        match self {
            Self::SetAcl {
                address,
                expected_acl_version,
                entries,
            } => {
                validate_mutation_address(address)?;
                if *expected_acl_version < 0 {
                    return Err(RegistryError::validation(
                        "ZooKeeper ACL version cannot be negative",
                    ));
                }
                if entries.is_empty() || entries.len() > 256 {
                    return Err(RegistryError::validation(
                        "ZooKeeper ACL requires between 1 and 256 entries",
                    ));
                }
                let mut identities = BTreeSet::new();
                let mut has_admin = false;
                for entry in entries {
                    let scheme = entry.scheme.trim();
                    let id = entry.id.trim();
                    if scheme.is_empty() || scheme.len() > 64 || id.len() > 512 {
                        return Err(RegistryError::validation(
                            "ZooKeeper ACL scheme must be 1–64 characters and id at most 512 characters",
                        ));
                    }
                    if !identities.insert((scheme.to_owned(), id.to_owned())) {
                        return Err(RegistryError::validation(
                            "ZooKeeper ACL contains a duplicate scheme/id identity",
                        ));
                    }
                    if entry.permissions.is_empty() {
                        return Err(RegistryError::validation(
                            "every ZooKeeper ACL entry must grant at least one permission",
                        ));
                    }
                    let mut permissions = BTreeSet::new();
                    for permission in &entry.permissions {
                        let normalized = permission.trim().to_ascii_lowercase();
                        if !matches!(
                            normalized.as_str(),
                            "read" | "write" | "create" | "delete" | "admin"
                        ) {
                            return Err(RegistryError::validation(format!(
                                "unsupported ZooKeeper ACL permission: {permission}"
                            )));
                        }
                        if !permissions.insert(normalized.clone()) {
                            return Err(RegistryError::validation(
                                "ZooKeeper ACL entry contains a duplicate permission",
                            ));
                        }
                        has_admin |= normalized == "admin";
                    }
                }
                if !has_admin {
                    return Err(RegistryError::validation(
                        "ZooKeeper ACL must keep at least one ADMIN identity to avoid an unrecoverable ACL",
                    ));
                }
            }
            Self::Create { address, value, .. } => {
                validate_mutation_address(address)?;
                value.decoded()?;
            }
        }
        Ok(())
    }

    pub fn address(&self) -> &ResourceAddress {
        match self {
            Self::SetAcl { address, .. } | Self::Create { address, .. } => address,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ZookeeperNativeActionResult {
    SetAcl {
        address: ResourceAddress,
        previous_acl_version: i32,
        current_acl_version: i32,
        previous_entries: Vec<ZookeeperAclEntry>,
        current_entries: Vec<ZookeeperAclEntry>,
        consistency: MutationConsistency,
    },
    Create {
        requested_address: ResourceAddress,
        address: ResourceAddress,
        mode: ZookeeperCreateMode,
        sequence: Option<String>,
        current: ResourceSnapshot,
        consistency: MutationConsistency,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeResourceInfo {
    EtcdLease {
        address: ResourceAddress,
        lease_id: String,
        remaining_ttl_seconds: i64,
        granted_ttl_seconds: i64,
    },
    ZookeeperAcl {
        address: ResourceAddress,
        acl_version: i32,
        entries: Vec<ZookeeperAclEntry>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EtcdLeaseAction {
    GrantAndAttach {
        address: ResourceAddress,
        expected_version: String,
        ttl_seconds: i64,
    },
    Attach {
        address: ResourceAddress,
        expected_version: String,
        lease_id: String,
    },
    Detach {
        address: ResourceAddress,
        expected_version: String,
    },
    KeepAlive {
        address: ResourceAddress,
        lease_id: String,
    },
    Revoke {
        address: ResourceAddress,
        expected_version: String,
        lease_id: String,
    },
}

impl EtcdLeaseAction {
    pub fn validate(&self) -> Result<(), RegistryError> {
        let ResourceAddress::Etcd { key_base64 } = self.address() else {
            return Err(RegistryError::validation(
                "etcd lease actions require an exact etcd key",
            ));
        };
        let key = STANDARD
            .decode(key_base64)
            .map_err(|_| RegistryError::validation("etcd key is not valid base64"))?;
        if key.is_empty() {
            return Err(RegistryError::validation("etcd key cannot be empty"));
        }
        if let Some(expected_version) = self.expected_version() {
            parse_positive_i64(expected_version, "etcd mod revision")?;
        }
        if let Some(lease_id) = self.lease_id() {
            parse_positive_i64(lease_id, "etcd lease id")?;
        }
        if let Self::GrantAndAttach { ttl_seconds, .. } = self
            && *ttl_seconds <= 0
        {
            return Err(RegistryError::validation(
                "etcd lease TTL must be a positive number of seconds",
            ));
        }
        Ok(())
    }

    pub fn address(&self) -> &ResourceAddress {
        match self {
            Self::GrantAndAttach { address, .. }
            | Self::Attach { address, .. }
            | Self::Detach { address, .. }
            | Self::KeepAlive { address, .. }
            | Self::Revoke { address, .. } => address,
        }
    }

    pub fn expected_version(&self) -> Option<&str> {
        match self {
            Self::GrantAndAttach {
                expected_version, ..
            }
            | Self::Attach {
                expected_version, ..
            }
            | Self::Detach {
                expected_version, ..
            }
            | Self::Revoke {
                expected_version, ..
            } => Some(expected_version),
            Self::KeepAlive { .. } => None,
        }
    }

    pub fn lease_id(&self) -> Option<&str> {
        match self {
            Self::Attach { lease_id, .. }
            | Self::KeepAlive { lease_id, .. }
            | Self::Revoke { lease_id, .. } => Some(lease_id),
            Self::GrantAndAttach { .. } | Self::Detach { .. } => None,
        }
    }
}

fn parse_positive_i64(value: &str, label: &str) -> Result<i64, RegistryError> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| RegistryError::validation(format!("{label} must be a positive integer")))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EtcdLeaseActionResult {
    GrantAndAttach {
        address: ResourceAddress,
        lease_id: String,
        remaining_ttl_seconds: i64,
        granted_ttl_seconds: i64,
        previous: ResourceSnapshot,
        current: ResourceSnapshot,
        consistency: MutationConsistency,
    },
    Attach {
        address: ResourceAddress,
        lease_id: String,
        remaining_ttl_seconds: i64,
        granted_ttl_seconds: i64,
        previous: ResourceSnapshot,
        current: ResourceSnapshot,
        consistency: MutationConsistency,
    },
    Detach {
        address: ResourceAddress,
        previous_lease_id: String,
        previous: ResourceSnapshot,
        current: ResourceSnapshot,
        consistency: MutationConsistency,
    },
    KeepAlive {
        address: ResourceAddress,
        lease_id: String,
        remaining_ttl_seconds: i64,
    },
    Revoke {
        address: ResourceAddress,
        lease_id: String,
        previous: ResourceSnapshot,
        consistency: MutationConsistency,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self, RegistryError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RegistryError::validation("operation id cannot be blank"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SubscriptionId(String);

impl SubscriptionId {
    pub fn new(value: impl Into<String>) -> Result<Self, RegistryError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(RegistryError::validation("subscription id cannot be blank"));
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WatchRequest {
    pub address: ResourceAddress,
    pub start_version: Option<String>,
}

impl WatchRequest {
    pub fn validate(&self) -> Result<(), RegistryError> {
        match &self.address {
            ResourceAddress::Root => {
                return Err(RegistryError::validation(
                    "watch requires an explicit resource or etcd prefix",
                ));
            }
            ResourceAddress::Etcd { key_base64 } => {
                validate_watch_bytes(key_base64, "etcd key")?;
            }
            ResourceAddress::EtcdPrefix { prefix_base64 } => {
                validate_watch_bytes(prefix_base64, "etcd prefix")?;
            }
            ResourceAddress::Zookeeper { path } => {
                if !is_canonical_zookeeper_path(path) {
                    return Err(RegistryError::validation(
                        "ZooKeeper watch requires a canonical absolute path",
                    ));
                }
                if self.start_version.is_some() {
                    return Err(RegistryError::validation(
                        "ZooKeeper watch cannot resume from a resource version",
                    ));
                }
            }
            ResourceAddress::NacosConfig { group, data_id } => {
                if group.trim().is_empty() || data_id.trim().is_empty() {
                    return Err(RegistryError::validation(
                        "Nacos watch requires both group and dataId",
                    ));
                }
                if self.start_version.is_some() {
                    return Err(RegistryError::validation(
                        "Nacos watch cannot resume from a resource version",
                    ));
                }
            }
        }
        if let Some(version) = &self.start_version {
            version
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|version| *version >= 0)
                .ok_or_else(|| {
                    RegistryError::validation(
                        "etcd watch startVersion must be a non-negative revision",
                    )
                })?;
        }
        Ok(())
    }
}

fn validate_watch_bytes(value: &str, label: &str) -> Result<(), RegistryError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| RegistryError::validation(format!("{label} is not valid base64")))?;
    if decoded.is_empty() {
        return Err(RegistryError::validation(format!(
            "{label} cannot be empty"
        )));
    }
    Ok(())
}

fn is_canonical_zookeeper_path(path: &str) -> bool {
    path.starts_with('/')
        && (path == "/" || !path.ends_with('/'))
        && !path.contains('\0')
        && !path
            .split('/')
            .skip(1)
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub enum WatchStatusState {
    Starting,
    Live,
    Reconnecting,
    Compacted,
    SessionExpired,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub enum WatchChangeKind {
    Created,
    Updated,
    Deleted,
    ChildrenChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WatchEvent {
    Status {
        subscription_id: String,
        state: WatchStatusState,
        message: Option<String>,
        retry_in_ms: Option<u64>,
    },
    Change {
        subscription_id: String,
        change: WatchChangeKind,
        address: ResourceAddress,
        version: Option<String>,
    },
}

impl WatchEvent {
    pub(crate) fn status(
        subscription_id: &SubscriptionId,
        state: WatchStatusState,
        message: Option<String>,
        retry_in_ms: Option<u64>,
    ) -> Self {
        Self::Status {
            subscription_id: subscription_id.as_str().to_owned(),
            state,
            message,
            retry_in_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueEncoding {
    Utf8,
    Base64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationValue {
    pub content: String,
    pub encoding: ValueEncoding,
}

impl MutationValue {
    pub(crate) fn decoded(&self) -> Result<Vec<u8>, RegistryError> {
        let bytes = match self.encoding {
            ValueEncoding::Utf8 => self.content.as_bytes().to_vec(),
            ValueEncoding::Base64 => STANDARD
                .decode(&self.content)
                .map_err(|_| RegistryError::validation("mutation value is not valid base64"))?,
        };
        if bytes.len() > EncodedValue::MAX_INLINE_BYTES {
            return Err(RegistryError::new(
                RegistryErrorCode::ValueTooLarge,
                format!(
                    "mutation is {} bytes; the current safety limit is {} bytes",
                    bytes.len(),
                    EncodedValue::MAX_INLINE_BYTES
                ),
                false,
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResourceMutation {
    Create {
        address: ResourceAddress,
        value: MutationValue,
        content_type: Option<String>,
    },
    Update {
        address: ResourceAddress,
        value: MutationValue,
        content_type: Option<String>,
        expected_version: String,
    },
    Delete {
        address: ResourceAddress,
        expected_version: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTransaction {
    pub mutations: Vec<ResourceMutation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EtcdTransactionResult {
    pub revision: String,
    pub results: Vec<MutationResult>,
}

impl EtcdTransaction {
    const MAX_MUTATIONS: usize = 32;

    pub fn validate(&self) -> Result<(), RegistryError> {
        if !(2..=Self::MAX_MUTATIONS).contains(&self.mutations.len()) {
            return Err(RegistryError::validation(format!(
                "etcd transaction requires between 2 and {} mutations",
                Self::MAX_MUTATIONS
            )));
        }

        let mut keys = BTreeSet::new();
        let mut payload_bytes = 0usize;
        for mutation in &self.mutations {
            mutation.validate()?;
            let ResourceAddress::Etcd { key_base64 } = mutation.address() else {
                return Err(RegistryError::validation(
                    "etcd transactions can only mutate exact etcd keys",
                ));
            };
            let key = STANDARD
                .decode(key_base64)
                .map_err(|_| RegistryError::validation("etcd key is not valid base64"))?;
            let key_bytes = key.len();
            if !keys.insert(key) {
                return Err(RegistryError::validation(
                    "etcd transaction contains a duplicate key",
                ));
            }
            add_transaction_payload(&mut payload_bytes, key_bytes)?;
            match mutation {
                ResourceMutation::Create {
                    value,
                    content_type,
                    ..
                } => {
                    add_transaction_payload(&mut payload_bytes, value.decoded()?.len())?;
                    add_transaction_payload(
                        &mut payload_bytes,
                        content_type.as_deref().map_or(0, str::len),
                    )?;
                }
                ResourceMutation::Update {
                    value,
                    content_type,
                    expected_version,
                    ..
                } => {
                    add_transaction_payload(&mut payload_bytes, value.decoded()?.len())?;
                    add_transaction_payload(
                        &mut payload_bytes,
                        content_type.as_deref().map_or(0, str::len),
                    )?;
                    add_transaction_payload(&mut payload_bytes, expected_version.len())?;
                }
                ResourceMutation::Delete {
                    expected_version, ..
                } => add_transaction_payload(&mut payload_bytes, expected_version.len())?,
            }
        }
        if payload_bytes > EncodedValue::MAX_INLINE_BYTES {
            return Err(RegistryError::new(
                RegistryErrorCode::ValueTooLarge,
                format!(
                    "transaction payload totals {payload_bytes} bytes; the safety limit is {} bytes",
                    EncodedValue::MAX_INLINE_BYTES
                ),
                false,
            ));
        }
        Ok(())
    }
}

fn add_transaction_payload(total: &mut usize, bytes: usize) -> Result<(), RegistryError> {
    *total = total
        .checked_add(bytes)
        .ok_or_else(|| RegistryError::validation("transaction payload is too large"))?;
    Ok(())
}

impl ResourceMutation {
    pub fn validate(&self) -> Result<(), RegistryError> {
        validate_mutation_address(self.address())?;
        match self {
            Self::Create { value, .. } => {
                value.decoded()?;
            }
            Self::Update {
                value,
                expected_version,
                ..
            } => {
                validate_expected_version(expected_version)?;
                value.decoded()?;
            }
            Self::Delete {
                expected_version, ..
            } => validate_expected_version(expected_version)?,
        }
        Ok(())
    }

    pub fn decoded_value(&self) -> Result<Vec<u8>, RegistryError> {
        match self {
            Self::Create { value, .. } | Self::Update { value, .. } => value.decoded(),
            Self::Delete { .. } => Err(RegistryError::validation(
                "delete mutation does not contain a value",
            )),
        }
    }

    pub fn address(&self) -> &ResourceAddress {
        match self {
            Self::Create { address, .. }
            | Self::Update { address, .. }
            | Self::Delete { address, .. } => address,
        }
    }

    pub fn operation(&self) -> MutationOperation {
        match self {
            Self::Create { .. } => MutationOperation::Create,
            Self::Update { .. } => MutationOperation::Update,
            Self::Delete { .. } => MutationOperation::Delete,
        }
    }

    pub fn expected_version(&self) -> Option<&str> {
        match self {
            Self::Create { .. } => None,
            Self::Update {
                expected_version, ..
            }
            | Self::Delete {
                expected_version, ..
            } => Some(expected_version),
        }
    }
}

fn validate_expected_version(expected_version: &str) -> Result<(), RegistryError> {
    if expected_version.trim().is_empty() {
        Err(RegistryError::validation(
            "conditional mutation requires an expected version",
        ))
    } else {
        Ok(())
    }
}

fn validate_mutation_address(address: &ResourceAddress) -> Result<(), RegistryError> {
    match address {
        ResourceAddress::Root | ResourceAddress::EtcdPrefix { .. } => Err(
            RegistryError::validation("only leaf resources can be mutated"),
        ),
        ResourceAddress::Etcd { key_base64 } => {
            let key = STANDARD
                .decode(key_base64)
                .map_err(|_| RegistryError::validation("etcd key is not valid base64"))?;
            if key.is_empty() {
                return Err(RegistryError::validation("etcd key cannot be empty"));
            }
            Ok(())
        }
        ResourceAddress::Zookeeper { path } => {
            if path == "/" || !is_canonical_zookeeper_path(path) {
                return Err(RegistryError::validation(
                    "ZooKeeper mutation requires a canonical absolute non-root path",
                ));
            }
            Ok(())
        }
        ResourceAddress::NacosConfig { group, data_id } => {
            if group.trim().is_empty() || data_id.trim().is_empty() {
                return Err(RegistryError::validation(
                    "Nacos mutation requires both group and dataId",
                ));
            }
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncodedValue {
    pub content: String,
    pub encoding: ValueEncoding,
    pub size_bytes: usize,
}

impl EncodedValue {
    pub const MAX_INLINE_BYTES: usize = 1024 * 1024;

    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(content) => Self {
                content: content.to_owned(),
                encoding: ValueEncoding::Utf8,
                size_bytes: bytes.len(),
            },
            Err(_) => Self {
                content: STANDARD.encode(bytes),
                encoding: ValueEncoding::Base64,
                size_bytes: bytes.len(),
            },
        }
    }

    pub fn try_from_inline_bytes(bytes: &[u8]) -> Result<Self, RegistryError> {
        if bytes.len() > Self::MAX_INLINE_BYTES {
            return Err(RegistryError::new(
                RegistryErrorCode::ValueTooLarge,
                format!(
                    "resource is {} bytes; inline display is limited to {} bytes",
                    bytes.len(),
                    Self::MAX_INLINE_BYTES
                ),
                false,
            ));
        }
        Ok(Self::from_bytes(bytes))
    }

    fn decoded(&self) -> Result<Vec<u8>, RegistryError> {
        match self.encoding {
            ValueEncoding::Utf8 => Ok(self.content.as_bytes().to_vec()),
            ValueEncoding::Base64 => STANDARD
                .decode(&self.content)
                .map_err(|_| RegistryError::invalid_response("resource value is not valid base64")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub version: Option<String>,
    pub sha256: String,
    pub size_bytes: usize,
    pub encoding: ValueEncoding,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationOperation {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationConsistency {
    Atomic,
    CheckedBeforeMutation,
}

#[derive(Clone, Default)]
pub(crate) struct MutationPhase(Arc<AtomicU8>);

#[derive(Clone, Copy, Eq, PartialEq)]
enum MutationPhaseState {
    PreDispatch = 0,
    Dispatched = 1,
    Finalizing = 2,
}

impl MutationPhase {
    pub(crate) fn mark_dispatched(&self) {
        self.0
            .store(MutationPhaseState::Dispatched as u8, Ordering::SeqCst);
    }

    pub(crate) fn mark_finalizing(&self) {
        self.0
            .store(MutationPhaseState::Finalizing as u8, Ordering::SeqCst);
    }

    fn state(&self) -> MutationPhaseState {
        match self.0.load(Ordering::SeqCst) {
            1 => MutationPhaseState::Dispatched,
            2 => MutationPhaseState::Finalizing,
            _ => MutationPhaseState::PreDispatch,
        }
    }

    pub(crate) fn timeout_error(&self) -> RegistryError {
        match self.state() {
            MutationPhaseState::PreDispatch => {
                RegistryError::timeout("resource mutation preflight")
            }
            MutationPhaseState::Dispatched | MutationPhaseState::Finalizing => {
                RegistryError::mutation_outcome_unknown(
                    "resource mutation timed out after write dispatch; refresh the resource to determine its remote state",
                )
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult {
    pub operation: MutationOperation,
    pub address: ResourceAddress,
    pub previous: Option<ResourceSnapshot>,
    pub current: Option<ResourceSnapshot>,
    pub consistency: MutationConsistency,
}

impl ResourceSnapshot {
    pub fn from_bytes(bytes: &[u8], version: Option<String>) -> Self {
        Self {
            version,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size_bytes: bytes.len(),
            encoding: if std::str::from_utf8(bytes).is_ok() {
                ValueEncoding::Utf8
            } else {
                ValueEncoding::Base64
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDocument {
    pub address: ResourceAddress,
    pub name: String,
    pub value: EncodedValue,
    pub content_type: Option<String>,
    pub version: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

impl ResourceDocument {
    pub fn snapshot(&self) -> Result<ResourceSnapshot, RegistryError> {
        Ok(ResourceSnapshot::from_bytes(
            &self.value.decoded()?,
            self.version.clone(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RegistryErrorCode {
    Validation,
    NotConnected,
    Unsupported,
    NotFound,
    Network,
    InvalidResponse,
    Timeout,
    ValueTooLarge,
    Conflict,
    OutcomeUnknown,
    PermissionDenied,
    ResourceExhausted,
    AuditIncomplete,
    CredentialMissing,
    CredentialStore,
    TlsConfiguration,
    Storage,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(test, ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RegistryError {
    pub code: RegistryErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl RegistryError {
    pub(crate) fn new(
        code: RegistryErrorCode,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::Validation, message, false)
    }

    pub(crate) fn network(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::Network, message, true)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::NotFound, message, false)
    }

    pub(crate) fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::InvalidResponse, message, false)
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::Unsupported, message, false)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::Conflict, message, false)
    }

    pub(crate) fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::PermissionDenied, message, false)
    }

    pub(crate) fn audit_incomplete(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::AuditIncomplete, message, false)
    }

    pub(crate) fn resource_exhausted(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::ResourceExhausted, message, true)
    }

    pub(crate) fn credential_missing(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::CredentialMissing, message, false)
    }

    pub(crate) fn credential_store(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::CredentialStore, message, true)
    }

    pub(crate) fn tls_configuration(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::TlsConfiguration, message, false)
    }

    pub(crate) fn timeout(operation: &str) -> Self {
        Self::new(
            RegistryErrorCode::Timeout,
            format!("{operation} timed out after 8 seconds"),
            true,
        )
    }

    pub(crate) fn storage(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::Storage, message, false)
    }

    fn cancelled() -> Self {
        Self::new(
            RegistryErrorCode::Cancelled,
            "operation was cancelled",
            true,
        )
    }

    fn outcome_unknown() -> Self {
        Self::mutation_outcome_unknown(
            "mutation was cancelled after dispatch; refresh the resource to determine its remote state",
        )
    }

    pub(crate) fn mutation_outcome_unknown(message: impl Into<String>) -> Self {
        Self::new(RegistryErrorCode::OutcomeUnknown, message, false)
    }
}

#[derive(Default)]
pub struct RegistryCatalog;

impl RegistryCatalog {
    pub fn descriptors(&self) -> Vec<AdapterDescriptor> {
        [AdapterId::Etcd, AdapterId::Zookeeper, AdapterId::Nacos]
            .into_iter()
            .map(|id| {
                let mut capabilities = vec![
                    Capability::Probe,
                    Capability::Browse,
                    Capability::Search,
                    Capability::Read,
                    Capability::Watch,
                    Capability::Create,
                    Capability::Update,
                    Capability::Delete,
                ];
                match id {
                    AdapterId::Etcd => {
                        capabilities.push(Capability::Lease);
                        capabilities.push(Capability::Transaction);
                    }
                    AdapterId::Zookeeper => {
                        capabilities.push(Capability::Acl);
                        capabilities.push(Capability::Ephemeral);
                    }
                    AdapterId::Nacos => {
                        capabilities.push(Capability::History);
                        capabilities.push(Capability::Namespace);
                        capabilities.push(Capability::Service);
                        capabilities.push(Capability::Instance);
                    }
                }
                AdapterDescriptor {
                    id,
                    status: AdapterStatus::Available,
                    capabilities,
                }
            })
            .collect()
    }
}

#[derive(Clone)]
struct WatchRegistration {
    connection_id: String,
    token: CancellationToken,
}

#[derive(Clone, Default)]
pub struct RegistryService {
    sessions: Arc<RwLock<BTreeMap<String, RegistrySession>>>,
    operations: Arc<RwLock<BTreeMap<OperationId, CancellationToken>>>,
    subscriptions: Arc<RwLock<BTreeMap<SubscriptionId, WatchRegistration>>>,
}

impl RegistryService {
    pub(crate) async fn connection_adapter(
        &self,
        connection_id: &str,
    ) -> Result<AdapterId, RegistryError> {
        Ok(self.session(connection_id).await?.adapter_id())
    }

    pub async fn probe_cancellable(
        &self,
        operation_id: OperationId,
        mut profile: ConnectionProfile,
    ) -> Result<ConnectionProbe, RegistryError> {
        profile.validate()?;
        let adapter = profile.adapter;
        let endpoint = profile.endpoint.clone();
        self.run_operation(operation_id, async move {
            RegistrySession::connect(&profile, None).await?;
            Ok(ConnectionProbe { adapter, endpoint })
        })
        .await
    }

    pub async fn probe_with_credentials_cancellable(
        &self,
        operation_id: OperationId,
        mut profile: ConnectionProfile,
        secret: Option<ConnectionSecret>,
    ) -> Result<ConnectionProbe, RegistryError> {
        profile.validate()?;
        let adapter = profile.adapter;
        let endpoint = profile.endpoint.clone();
        self.run_operation(operation_id, async move {
            RegistrySession::connect(&profile, secret).await?;
            Ok(ConnectionProbe { adapter, endpoint })
        })
        .await
    }

    pub async fn open(
        &self,
        mut profile: ConnectionProfile,
    ) -> Result<ConnectionSession, RegistryError> {
        profile.validate()?;
        self.open_with_credentials(profile, None).await
    }

    pub async fn open_with_credentials(
        &self,
        mut profile: ConnectionProfile,
        secret: Option<ConnectionSecret>,
    ) -> Result<ConnectionSession, RegistryError> {
        profile.validate()?;
        let session = RegistrySession::connect(&profile, secret).await?;
        let summary = ConnectionSession {
            id: profile.id.clone(),
            name: profile.name,
            adapter: profile.adapter,
            endpoint: profile.endpoint,
        };
        self.sessions
            .write()
            .await
            .insert(summary.id.clone(), session);
        Ok(summary)
    }

    pub async fn open_cancellable(
        &self,
        operation_id: OperationId,
        profile: ConnectionProfile,
    ) -> Result<ConnectionSession, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move { service.open(profile).await })
            .await
    }

    pub async fn open_with_credentials_cancellable(
        &self,
        operation_id: OperationId,
        profile: ConnectionProfile,
        secret: Option<ConnectionSecret>,
    ) -> Result<ConnectionSession, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.open_with_credentials(profile, secret).await
        })
        .await
    }

    pub async fn close(&self, connection_id: &str) -> Result<(), RegistryError> {
        self.cancel_watches_for_connection(connection_id).await;
        self.sessions
            .write()
            .await
            .remove(connection_id)
            .map(|_| ())
            .ok_or_else(|| Self::not_connected(connection_id))
    }

    pub async fn list(
        &self,
        connection_id: &str,
        parent: ResourceAddress,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<ResourcePage, RegistryError> {
        let session = self.session(connection_id).await?;
        session.list(parent, cursor, limit.clamp(1, 200)).await
    }

    pub async fn list_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        request: ResourcePageRequest,
    ) -> Result<ResourcePage, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service
                .list(
                    &connection_id,
                    request.parent,
                    request.cursor,
                    request.limit.unwrap_or(100),
                )
                .await
        })
        .await
    }

    pub async fn read(
        &self,
        connection_id: &str,
        address: ResourceAddress,
    ) -> Result<ResourceDocument, RegistryError> {
        let session = self.session(connection_id).await?;
        session.read(address).await
    }

    pub async fn read_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        address: ResourceAddress,
    ) -> Result<ResourceDocument, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.read(&connection_id, address).await
        })
        .await
    }

    pub async fn search(
        &self,
        connection_id: &str,
        mut request: ResourceSearchRequest,
    ) -> Result<ResourceSearchPage, RegistryError> {
        request.validate()?;
        let limit = request.limit.unwrap_or(100).clamp(1, 200);
        let session = self.session(connection_id).await?;
        session.search(request, limit).await
    }

    pub async fn search_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        request: ResourceSearchRequest,
    ) -> Result<ResourceSearchPage, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.search(&connection_id, request).await
        })
        .await
    }

    pub async fn history(
        &self,
        connection_id: &str,
        request: ResourceHistoryRequest,
    ) -> Result<ResourceHistoryPage, RegistryError> {
        request.validate()?;
        let limit = request.limit.unwrap_or(50).clamp(1, 200);
        let session = self.session(connection_id).await?;
        session.history(request, limit).await
    }

    pub async fn history_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        request: ResourceHistoryRequest,
    ) -> Result<ResourceHistoryPage, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.history(&connection_id, request).await
        })
        .await
    }

    pub async fn read_history_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        address: ResourceAddress,
        revision_id: String,
    ) -> Result<ResourceHistoryDocument, RegistryError> {
        ResourceHistoryRequest {
            address: address.clone(),
            cursor: None,
            limit: None,
        }
        .validate()?;
        let service = self.clone();
        self.run_operation(operation_id, async move {
            let session = service.session(&connection_id).await?;
            session.read_history(address, revision_id).await
        })
        .await
    }

    pub async fn list_nacos_namespaces(
        &self,
        connection_id: &str,
    ) -> Result<Vec<NacosNamespace>, RegistryError> {
        self.session(connection_id)
            .await?
            .list_nacos_namespaces()
            .await
    }

    pub async fn list_nacos_namespaces_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
    ) -> Result<Vec<NacosNamespace>, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.list_nacos_namespaces(&connection_id).await
        })
        .await
    }

    pub async fn list_nacos_services(
        &self,
        connection_id: &str,
        group: String,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<NacosServicePage, RegistryError> {
        self.session(connection_id)
            .await?
            .list_nacos_services(group, cursor, limit.clamp(1, 100))
            .await
    }

    pub async fn list_nacos_services_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        group: String,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<NacosServicePage, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service
                .list_nacos_services(&connection_id, group, cursor, limit)
                .await
        })
        .await
    }

    pub async fn read_nacos_service(
        &self,
        connection_id: &str,
        group: String,
        service_name: String,
    ) -> Result<NacosService, RegistryError> {
        self.session(connection_id)
            .await?
            .read_nacos_service(group, service_name)
            .await
    }

    pub async fn read_nacos_service_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        group: String,
        service_name: String,
    ) -> Result<NacosService, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service
                .read_nacos_service(&connection_id, group, service_name)
                .await
        })
        .await
    }

    pub async fn list_nacos_instances(
        &self,
        connection_id: &str,
        group: String,
        service_name: String,
    ) -> Result<Vec<NacosInstance>, RegistryError> {
        self.session(connection_id)
            .await?
            .list_nacos_instances(group, service_name)
            .await
    }

    pub async fn list_nacos_instances_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        group: String,
        service_name: String,
    ) -> Result<Vec<NacosInstance>, RegistryError> {
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service
                .list_nacos_instances(&connection_id, group, service_name)
                .await
        })
        .await
    }

    pub async fn execute_nacos_native_action(
        &self,
        connection_id: &str,
        action: NacosNativeAction,
    ) -> Result<NacosNativeActionResult, RegistryError> {
        self.execute_nacos_native_action_with_phase(connection_id, action, MutationPhase::default())
            .await
    }

    pub(crate) async fn execute_nacos_native_action_with_phase(
        &self,
        connection_id: &str,
        mut action: NacosNativeAction,
        phase: MutationPhase,
    ) -> Result<NacosNativeActionResult, RegistryError> {
        action.validate()?;
        self.session(connection_id)
            .await?
            .execute_nacos_native_action(action, phase)
            .await
    }

    pub async fn inspect_native_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        address: ResourceAddress,
    ) -> Result<NativeResourceInfo, RegistryError> {
        if !matches!(
            &address,
            ResourceAddress::Etcd { .. } | ResourceAddress::Zookeeper { .. }
        ) {
            return Err(RegistryError::unsupported(
                "native inspection requires an etcd key or ZooKeeper znode",
            ));
        }
        let service = self.clone();
        self.run_operation(operation_id, async move {
            service.inspect_native(&connection_id, address).await
        })
        .await
    }

    pub async fn inspect_native(
        &self,
        connection_id: &str,
        address: ResourceAddress,
    ) -> Result<NativeResourceInfo, RegistryError> {
        if !matches!(
            &address,
            ResourceAddress::Etcd { .. } | ResourceAddress::Zookeeper { .. }
        ) {
            return Err(RegistryError::unsupported(
                "native inspection requires an etcd key or ZooKeeper znode",
            ));
        }
        self.session(connection_id)
            .await?
            .inspect_native(address)
            .await
    }

    pub async fn mutate(
        &self,
        connection_id: &str,
        mutation: ResourceMutation,
    ) -> Result<MutationResult, RegistryError> {
        self.mutate_with_phase(connection_id, mutation, MutationPhase::default())
            .await
    }

    pub(crate) async fn mutate_with_phase(
        &self,
        connection_id: &str,
        mutation: ResourceMutation,
        phase: MutationPhase,
    ) -> Result<MutationResult, RegistryError> {
        mutation.validate()?;
        let session = self.session(connection_id).await?;
        session.mutate(mutation, phase).await
    }

    pub async fn mutate_cancellable(
        &self,
        operation_id: OperationId,
        connection_id: String,
        mutation: ResourceMutation,
    ) -> Result<MutationResult, RegistryError> {
        let service = self.clone();
        let phase = MutationPhase::default();
        let running_phase = phase.clone();
        self.run_mutation_workflow(operation_id, phase, async move {
            service
                .mutate_with_phase(&connection_id, mutation, running_phase)
                .await
        })
        .await
    }

    pub async fn execute_etcd_transaction(
        &self,
        connection_id: &str,
        transaction: EtcdTransaction,
    ) -> Result<EtcdTransactionResult, RegistryError> {
        self.execute_etcd_transaction_with_phase(
            connection_id,
            transaction,
            MutationPhase::default(),
        )
        .await
    }

    pub(crate) async fn execute_etcd_transaction_with_phase(
        &self,
        connection_id: &str,
        transaction: EtcdTransaction,
        phase: MutationPhase,
    ) -> Result<EtcdTransactionResult, RegistryError> {
        transaction.validate()?;
        let session = self.session(connection_id).await?;
        session.execute_etcd_transaction(transaction, phase).await
    }

    pub async fn execute_etcd_lease_action(
        &self,
        connection_id: &str,
        action: EtcdLeaseAction,
    ) -> Result<EtcdLeaseActionResult, RegistryError> {
        self.execute_etcd_lease_action_with_phase(connection_id, action, MutationPhase::default())
            .await
    }

    pub(crate) async fn execute_etcd_lease_action_with_phase(
        &self,
        connection_id: &str,
        action: EtcdLeaseAction,
        phase: MutationPhase,
    ) -> Result<EtcdLeaseActionResult, RegistryError> {
        action.validate()?;
        let session = self.session(connection_id).await?;
        session.execute_etcd_lease_action(action, phase).await
    }

    pub async fn execute_zookeeper_native_action(
        &self,
        connection_id: &str,
        action: ZookeeperNativeAction,
    ) -> Result<ZookeeperNativeActionResult, RegistryError> {
        self.execute_zookeeper_native_action_with_phase(
            connection_id,
            action,
            MutationPhase::default(),
        )
        .await
    }

    pub(crate) async fn execute_zookeeper_native_action_with_phase(
        &self,
        connection_id: &str,
        action: ZookeeperNativeAction,
        phase: MutationPhase,
    ) -> Result<ZookeeperNativeActionResult, RegistryError> {
        action.validate()?;
        let session = self.session(connection_id).await?;
        session.execute_zookeeper_native_action(action, phase).await
    }

    pub(crate) async fn run_mutation_workflow<T>(
        &self,
        operation_id: OperationId,
        phase: MutationPhase,
        operation: impl Future<Output = Result<T, RegistryError>>,
    ) -> Result<T, RegistryError> {
        let token = CancellationToken::new();
        {
            let mut operations = self.operations.write().await;
            if operations.contains_key(&operation_id) {
                return Err(RegistryError::validation(format!(
                    "operation id '{}' is already active",
                    operation_id.0
                )));
            }
            operations.insert(operation_id.clone(), token.clone());
        }
        tokio::pin!(operation);
        let result = tokio::select! {
            biased;
            result = &mut operation => result,
            () = token.cancelled() => match phase.state() {
                MutationPhaseState::PreDispatch => Err(RegistryError::cancelled()),
                MutationPhaseState::Dispatched => Err(RegistryError::outcome_unknown()),
                MutationPhaseState::Finalizing => operation.await,
            },
        };
        self.operations.write().await.remove(&operation_id);
        result
    }

    pub async fn cancel(&self, operation_id: &OperationId) -> bool {
        if let Some(token) = self.operations.read().await.get(operation_id).cloned() {
            token.cancel();
            true
        } else {
            false
        }
    }

    pub async fn stop_watch(&self, subscription_id: &SubscriptionId) -> bool {
        if let Some(registration) = self
            .subscriptions
            .read()
            .await
            .get(subscription_id)
            .cloned()
        {
            registration.token.cancel();
            true
        } else {
            false
        }
    }

    pub async fn start_watch(
        &self,
        subscription_id: SubscriptionId,
        connection_id: String,
        request: WatchRequest,
    ) -> Result<mpsc::Receiver<WatchEvent>, RegistryError> {
        request.validate()?;
        let session = self.session(&connection_id).await?;
        let running_id = subscription_id.clone();
        self.start_watch_workflow(
            subscription_id,
            connection_id,
            move |token, events| async move {
                watch::run(session, running_id, request, token, events).await
            },
        )
        .await
    }

    async fn cancel_watches_for_connection(&self, connection_id: &str) {
        let tokens = self
            .subscriptions
            .read()
            .await
            .values()
            .filter(|registration| registration.connection_id == connection_id)
            .map(|registration| registration.token.clone())
            .collect::<Vec<_>>();
        for token in tokens {
            token.cancel();
        }
    }

    pub(crate) async fn start_watch_workflow<Run, RunFuture>(
        &self,
        subscription_id: SubscriptionId,
        connection_id: String,
        run: Run,
    ) -> Result<mpsc::Receiver<WatchEvent>, RegistryError>
    where
        Run: FnOnce(CancellationToken, mpsc::Sender<WatchEvent>) -> RunFuture + Send + 'static,
        RunFuture: Future<Output = Result<(), RegistryError>> + Send + 'static,
    {
        let token = CancellationToken::new();
        {
            let mut subscriptions = self.subscriptions.write().await;
            if subscriptions.contains_key(&subscription_id) {
                return Err(RegistryError::validation(format!(
                    "subscription id '{}' is already active",
                    subscription_id.as_str()
                )));
            }
            subscriptions.insert(
                subscription_id.clone(),
                WatchRegistration {
                    connection_id,
                    token: token.clone(),
                },
            );
        }

        let (events, receiver) = mpsc::channel(64);
        let registrations = self.subscriptions.clone();
        tokio::spawn(async move {
            if events
                .send(WatchEvent::status(
                    &subscription_id,
                    WatchStatusState::Starting,
                    None,
                    None,
                ))
                .await
                .is_err()
            {
                registrations.write().await.remove(&subscription_id);
                return;
            }

            let running = run(token.clone(), events.clone());
            tokio::pin!(running);
            let terminal = tokio::select! {
                result = &mut running => match result {
                    Ok(()) => WatchEvent::status(
                        &subscription_id,
                        WatchStatusState::Stopped,
                        None,
                        None,
                    ),
                    Err(error) => WatchEvent::status(
                        &subscription_id,
                        WatchStatusState::Failed,
                        Some(error.message),
                        None,
                    ),
                },
                () = events.closed() => {
                    token.cancel();
                    let _ = (&mut running).await;
                    registrations.write().await.remove(&subscription_id);
                    return;
                }
            };
            let _ = events.send(terminal).await;
            registrations.write().await.remove(&subscription_id);
        });
        Ok(receiver)
    }

    async fn session(&self, connection_id: &str) -> Result<RegistrySession, RegistryError> {
        self.sessions
            .read()
            .await
            .get(connection_id)
            .cloned()
            .ok_or_else(|| Self::not_connected(connection_id))
    }

    fn not_connected(connection_id: &str) -> RegistryError {
        RegistryError::new(
            RegistryErrorCode::NotConnected,
            format!("connection '{connection_id}' is not open"),
            true,
        )
    }

    async fn run_operation<T>(
        &self,
        operation_id: OperationId,
        operation: impl Future<Output = Result<T, RegistryError>>,
    ) -> Result<T, RegistryError> {
        self.run_operation_with_cancel_error(operation_id, operation, RegistryError::cancelled())
            .await
    }

    async fn run_operation_with_cancel_error<T>(
        &self,
        operation_id: OperationId,
        operation: impl Future<Output = Result<T, RegistryError>>,
        cancel_error: RegistryError,
    ) -> Result<T, RegistryError> {
        self.run_operation_with_cancel_error_factory(operation_id, operation, move || cancel_error)
            .await
    }

    async fn run_operation_with_cancel_error_factory<T>(
        &self,
        operation_id: OperationId,
        operation: impl Future<Output = Result<T, RegistryError>>,
        cancel_error: impl FnOnce() -> RegistryError,
    ) -> Result<T, RegistryError> {
        let token = CancellationToken::new();
        {
            let mut operations = self.operations.write().await;
            if operations.contains_key(&operation_id) {
                return Err(RegistryError::validation(format!(
                    "operation id '{}' is already active",
                    operation_id.0
                )));
            }
            operations.insert(operation_id.clone(), token.clone());
        }
        let result = tokio::select! {
            result = operation => result,
            () = token.cancelled() => Err(cancel_error()),
        };
        self.operations.write().await.remove(&operation_id);
        result
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    use super::{
        EncodedValue, EtcdLeaseAction, EtcdTransaction, MutationPhase, NacosNativeAction,
        NativeResourceInfo, OperationId, RegistryError, RegistryErrorCode, RegistryService,
        ResourceAddress, ResourceMutation, ResourceSearchRequest, SubscriptionId, WatchChangeKind,
        WatchEvent, WatchRequest, WatchStatusState, ZookeeperNativeAction,
    };

    #[test]
    fn nacos_native_contract_bounds_metadata_and_requires_conditional_fingerprints() {
        let mut action: NacosNativeAction = serde_json::from_value(serde_json::json!({
            "action": "updateInstance",
            "group": " DEFAULT_GROUP ",
            "serviceName": "payments",
            "cluster": "DEFAULT",
            "ip": "127.0.0.1",
            "port": 8080,
            "weight": 1.0,
            "enabled": true,
            "ephemeral": false,
            "metadata": { "zone": "east" },
            "expectedFingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }))
        .expect("camelCase Nacos native action should deserialize");
        action
            .validate()
            .expect("bounded instance update should be valid");

        let mut unsafe_delete: NacosNativeAction = serde_json::from_value(serde_json::json!({
            "action": "deleteNamespace",
            "namespaceId": "public",
            "expectedFingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }))
        .expect("public delete should deserialize before validation");
        assert_eq!(
            unsafe_delete.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );

        let mut ephemeral_instance: NacosNativeAction = serde_json::from_value(serde_json::json!({
            "action": "registerInstance",
            "group": "DEFAULT_GROUP",
            "serviceName": "payments",
            "cluster": "DEFAULT",
            "ip": "127.0.0.1",
            "port": 8080,
            "weight": 1.0,
            "enabled": true,
            "ephemeral": true,
            "metadata": {}
        }))
        .expect("ephemeral instance should deserialize before validation");
        ephemeral_instance
            .validate()
            .expect("ephemeral instances are managed by the Naming SDK heartbeat");
    }

    #[test]
    fn zookeeper_native_contract_requires_versioned_admin_acl_and_bounded_create_value() {
        let action: ZookeeperNativeAction = serde_json::from_value(serde_json::json!({
            "action": "setAcl",
            "address": { "type": "zookeeper", "path": "/atlas/key" },
            "expectedAclVersion": 3,
            "entries": [{
                "scheme": "world",
                "id": "anyone",
                "permissions": ["read", "admin"]
            }]
        }))
        .expect("camelCase ZooKeeper ACL action should deserialize");
        action
            .validate()
            .expect("versioned ACL retaining ADMIN should be valid");

        let lockout: ZookeeperNativeAction = serde_json::from_value(serde_json::json!({
            "action": "setAcl",
            "address": { "type": "zookeeper", "path": "/atlas/key" },
            "expectedAclVersion": 3,
            "entries": [{
                "scheme": "world",
                "id": "anyone",
                "permissions": ["read"]
            }]
        }))
        .expect("unsafe ACL should deserialize before validation");
        assert_eq!(
            lockout.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );

        let ephemeral: ZookeeperNativeAction = serde_json::from_value(serde_json::json!({
            "action": "create",
            "address": { "type": "zookeeper", "path": "/atlas/member-" },
            "value": { "content": "online", "encoding": "utf8" },
            "mode": "ephemeralSequential"
        }))
        .expect("native create should deserialize");
        ephemeral
            .validate()
            .expect("bounded ephemeral create should be valid");
    }

    #[test]
    fn etcd_lease_action_contract_keeps_ids_lossless_and_validates_key_safety() {
        let action: EtcdLeaseAction = serde_json::from_value(serde_json::json!({
            "action": "attach",
            "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2tleQ==" },
            "expectedVersion": "42",
            "leaseId": "9223372036854775807"
        }))
        .expect("camelCase lease action should deserialize");
        action
            .validate()
            .expect("positive 64-bit lease id should be lossless");

        let invalid: EtcdLeaseAction = serde_json::from_value(serde_json::json!({
            "action": "grantAndAttach",
            "address": { "type": "zookeeper", "path": "/atlas/key" },
            "expectedVersion": "7",
            "ttlSeconds": 60
        }))
        .expect("adapter mismatch should deserialize before validation");
        assert_eq!(
            invalid.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );
    }

    #[test]
    fn etcd_transaction_contract_is_bounded_and_rejects_duplicate_keys() {
        let transaction: EtcdTransaction = serde_json::from_value(serde_json::json!({
            "mutations": [
                {
                    "operation": "create",
                    "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2E=" },
                    "value": { "content": "first", "encoding": "utf8" }
                },
                {
                    "operation": "update",
                    "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2I=" },
                    "value": { "content": "second", "encoding": "utf8" },
                    "expectedVersion": "42"
                }
            ]
        }))
        .expect("camelCase transaction should deserialize");
        transaction
            .validate()
            .expect("two distinct etcd keys are valid");

        let duplicate: EtcdTransaction = serde_json::from_value(serde_json::json!({
            "mutations": [
                {
                    "operation": "delete",
                    "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2E=" },
                    "expectedVersion": "7"
                },
                {
                    "operation": "update",
                    "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2E=" },
                    "value": { "content": "replacement", "encoding": "utf8" },
                    "expectedVersion": "7"
                }
            ]
        }))
        .expect("duplicate transaction should deserialize before validation");
        let error = duplicate
            .validate()
            .expect_err("one key cannot be written twice");
        assert_eq!(error.code, RegistryErrorCode::Validation);
        assert!(error.message.contains("duplicate"));

        let oversized_keys = EtcdTransaction {
            mutations: vec![
                ResourceMutation::Delete {
                    address: ResourceAddress::Etcd {
                        key_base64: STANDARD.encode(vec![b'k'; EncodedValue::MAX_INLINE_BYTES]),
                    },
                    expected_version: "1".to_owned(),
                },
                ResourceMutation::Delete {
                    address: ResourceAddress::Etcd {
                        key_base64: STANDARD.encode(b"second"),
                    },
                    expected_version: "2".to_owned(),
                },
            ],
        };
        assert_eq!(
            oversized_keys
                .validate()
                .expect_err("keys and versions count toward the transaction payload")
                .code,
            RegistryErrorCode::ValueTooLarge
        );
    }

    #[test]
    fn native_lease_contract_keeps_the_64_bit_identifier_lossless() {
        let info = NativeResourceInfo::EtcdLease {
            address: ResourceAddress::Etcd {
                key_base64: "Y29uZmln".to_owned(),
            },
            lease_id: i64::MAX.to_string(),
            remaining_ttl_seconds: 30,
            granted_ttl_seconds: 60,
        };

        let json = serde_json::to_value(info).unwrap();
        assert_eq!(json["kind"], "etcdLease");
        assert_eq!(json["leaseId"], i64::MAX.to_string());
        assert_eq!(json["remainingTtlSeconds"], 30);
        assert_eq!(json["address"]["keyBase64"], "Y29uZmln");
        assert!(json["address"].get("key_base64").is_none());
    }

    #[test]
    fn mutation_contract_accepts_camel_case_version_fields() {
        let mutation: ResourceMutation = serde_json::from_value(serde_json::json!({
            "operation": "delete",
            "address": { "type": "zookeeper", "path": "/config/app" },
            "expectedVersion": "7"
        }))
        .unwrap();

        assert!(matches!(
            mutation,
            ResourceMutation::Delete {
                expected_version,
                ..
            } if expected_version == "7"
        ));
    }

    #[test]
    fn resource_address_accepts_legacy_snake_case_exports() {
        let address: ResourceAddress = serde_json::from_value(serde_json::json!({
            "type": "nacosConfig",
            "group": "DEFAULT_GROUP",
            "data_id": "application.yaml"
        }))
        .unwrap();

        assert!(matches!(
            address,
            ResourceAddress::NacosConfig { group, data_id }
                if group == "DEFAULT_GROUP" && data_id == "application.yaml"
        ));
    }

    #[test]
    fn search_request_trims_queries_and_rejects_unbounded_input() {
        let mut request = ResourceSearchRequest {
            scope: ResourceAddress::Root,
            query: "  application  ".to_owned(),
            cursor: None,
            limit: None,
        };
        request.validate().unwrap();
        assert_eq!(request.query, "application");

        request.query = "   ".to_owned();
        assert_eq!(
            request.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );

        request.query = "x".repeat(257);
        assert_eq!(
            request.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_operation_and_removes_its_registration() {
        let service = RegistryService::default();
        let operation_id = OperationId::new("pending-probe".to_owned()).unwrap();
        let running_service = service.clone();
        let running_id = operation_id.clone();
        let task = tokio::spawn(async move {
            running_service
                .run_operation(
                    running_id,
                    std::future::pending::<Result<(), RegistryError>>(),
                )
                .await
        });

        for _ in 0..10 {
            if service.operations.read().await.contains_key(&operation_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(service.operations.read().await.contains_key(&operation_id));
        assert!(service.cancel(&operation_id).await);

        let error = task
            .await
            .expect("operation task should finish")
            .expect_err("cancelled operation should return an error");
        assert_eq!(error.code, RegistryErrorCode::Cancelled);
        assert!(!service.cancel(&operation_id).await);
    }

    #[tokio::test]
    async fn cancelling_a_mutation_reports_an_unknown_remote_outcome() {
        let service = RegistryService::default();
        let operation_id = OperationId::new("pending-mutation".to_owned()).unwrap();
        let running_service = service.clone();
        let running_id = operation_id.clone();
        let task = tokio::spawn(async move {
            running_service
                .run_operation_with_cancel_error(
                    running_id,
                    std::future::pending::<Result<(), RegistryError>>(),
                    RegistryError::outcome_unknown(),
                )
                .await
        });

        for _ in 0..10 {
            if service.operations.read().await.contains_key(&operation_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(service.cancel(&operation_id).await);
        let error = task
            .await
            .expect("operation task should finish")
            .expect_err("cancelled mutation cannot claim a known outcome");
        assert_eq!(error.code, RegistryErrorCode::OutcomeUnknown);
        assert!(!error.retryable);
    }

    #[test]
    fn mutation_timeout_reports_an_unknown_remote_outcome() {
        let error = RegistryError::mutation_outcome_unknown("timed out after 8 seconds");

        assert_eq!(error.code, RegistryErrorCode::OutcomeUnknown);
        assert!(error.message.contains("timed out"));
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn mutation_workflow_cancellation_uses_the_dispatch_phase() {
        let service = RegistryService::default();
        let phase = MutationPhase::default();
        let running_service = service.clone();
        let running_phase = phase.clone();
        let operation_id = OperationId::new("workflow-phase".to_owned()).unwrap();
        let running_id = operation_id.clone();
        let task = tokio::spawn(async move {
            running_service
                .run_mutation_workflow(
                    running_id,
                    running_phase,
                    std::future::pending::<Result<(), RegistryError>>(),
                )
                .await
        });

        for _ in 0..10 {
            if service.operations.read().await.contains_key(&operation_id) {
                break;
            }
            tokio::task::yield_now().await;
        }
        phase.mark_dispatched();
        assert!(service.cancel(&operation_id).await);
        let error = task.await.unwrap().unwrap_err();

        assert_eq!(error.code, RegistryErrorCode::OutcomeUnknown);
    }

    #[tokio::test]
    async fn cancellation_does_not_interrupt_final_audit_recording() {
        let service = RegistryService::default();
        let phase = MutationPhase::default();
        let running_service = service.clone();
        let running_phase = phase.clone();
        let operation_id = OperationId::new("workflow-finalizing".to_owned()).unwrap();
        let running_id = operation_id.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let workflow_phase = running_phase.clone();
            running_service
                .run_mutation_workflow(running_id, running_phase, async move {
                    workflow_phase.mark_finalizing();
                    let _ = entered_tx.send(());
                    finish_rx.await.unwrap();
                    Ok::<_, RegistryError>(())
                })
                .await
        });

        entered_rx.await.unwrap();
        assert!(service.cancel(&operation_id).await);
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        finish_tx.send(()).unwrap();

        assert!(task.await.unwrap().is_ok());
    }

    #[test]
    fn watch_request_rejects_root_and_non_etcd_resume_versions() {
        let root = WatchRequest {
            address: ResourceAddress::Root,
            start_version: None,
        };
        assert_eq!(
            root.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );

        let zookeeper = WatchRequest {
            address: ResourceAddress::Zookeeper {
                path: "/config/app".to_owned(),
            },
            start_version: Some("9".to_owned()),
        };
        assert_eq!(
            zookeeper.validate().unwrap_err().code,
            RegistryErrorCode::Validation
        );
    }

    #[test]
    fn watch_event_contract_never_contains_a_resource_value() {
        let event = WatchEvent::Change {
            subscription_id: "watch-1".to_owned(),
            change: WatchChangeKind::Updated,
            address: ResourceAddress::NacosConfig {
                group: "DEFAULT_GROUP".to_owned(),
                data_id: "application.yaml".to_owned(),
            },
            version: Some("md5-only".to_owned()),
        };

        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["kind"], "change");
        assert_eq!(json["change"], "updated");
        assert_eq!(json["subscriptionId"], "watch-1");
        assert!(json.get("subscription_id").is_none());
        assert!(json.get("value").is_none());
        assert!(json.get("content").is_none());
    }

    #[tokio::test]
    async fn watch_subscription_ids_are_unique_and_cancellation_is_terminal() {
        let service = RegistryService::default();
        let subscription_id = SubscriptionId::new("watch-1").unwrap();
        let mut events = service
            .start_watch_workflow(
                subscription_id.clone(),
                "connection-a".to_owned(),
                |token, _events| async move {
                    token.cancelled().await;
                    Ok(())
                },
            )
            .await
            .unwrap();

        assert!(matches!(
            events.recv().await,
            Some(WatchEvent::Status {
                state: WatchStatusState::Starting,
                ..
            })
        ));
        let duplicate = service
            .start_watch_workflow(
                subscription_id.clone(),
                "connection-a".to_owned(),
                |_token, _events| async move { Ok(()) },
            )
            .await
            .unwrap_err();
        assert_eq!(duplicate.code, RegistryErrorCode::Validation);

        assert!(service.stop_watch(&subscription_id).await);
        assert!(matches!(
            events.recv().await,
            Some(WatchEvent::Status {
                state: WatchStatusState::Stopped,
                ..
            })
        ));
        for _ in 0..10 {
            if !service.stop_watch(&subscription_id).await {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("watch registration should be removed after cancellation");
    }

    #[tokio::test]
    async fn closing_a_connection_cancels_only_its_watch_subscriptions() {
        let service = RegistryService::default();
        let first = SubscriptionId::new("first").unwrap();
        let second = SubscriptionId::new("second").unwrap();
        let mut first_events = service
            .start_watch_workflow(
                first.clone(),
                "connection-a".to_owned(),
                |token, _events| async move {
                    token.cancelled().await;
                    Ok(())
                },
            )
            .await
            .unwrap();
        let _second_events = service
            .start_watch_workflow(
                second.clone(),
                "connection-b".to_owned(),
                |token, _events| async move {
                    token.cancelled().await;
                    Ok(())
                },
            )
            .await
            .unwrap();
        let _ = first_events.recv().await;

        service.cancel_watches_for_connection("connection-a").await;

        assert!(matches!(
            first_events.recv().await,
            Some(WatchEvent::Status {
                state: WatchStatusState::Stopped,
                ..
            })
        ));
        assert!(service.stop_watch(&second).await);
    }
}
