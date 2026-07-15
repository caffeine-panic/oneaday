mod adapters;
mod mutations;
mod watch;

use std::{
    collections::BTreeMap,
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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterDescriptor {
    pub id: AdapterId,
    pub status: AdapterStatus,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NacosApiVersion {
    #[default]
    V2,
    V3,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
#[serde(rename_all = "camelCase")]
pub enum AuthenticationMode {
    #[default]
    None,
    UsernamePassword,
    Digest,
    Custom,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
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
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ResourceAddress {
    Root,
    Etcd { key_base64: String },
    EtcdPrefix { prefix_base64: String },
    Zookeeper { path: String },
    NacosConfig { group: String, data_id: String },
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
#[serde(rename_all = "camelCase")]
pub enum WatchChangeKind {
    Created,
    Updated,
    Deleted,
    ChildrenChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
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
#[serde(tag = "operation", rename_all = "camelCase")]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MutationOperation {
    Create,
    Update,
    Delete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
            .map(|id| AdapterDescriptor {
                id,
                status: AdapterStatus::Available,
                capabilities: vec![
                    Capability::Probe,
                    Capability::Browse,
                    Capability::Search,
                    Capability::Read,
                    Capability::Watch,
                    Capability::Create,
                    Capability::Update,
                    Capability::Delete,
                ],
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
    use super::{
        MutationPhase, OperationId, RegistryError, RegistryErrorCode, RegistryService,
        ResourceAddress, ResourceSearchRequest, SubscriptionId, WatchChangeKind, WatchEvent,
        WatchRequest, WatchStatusState,
    };

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
