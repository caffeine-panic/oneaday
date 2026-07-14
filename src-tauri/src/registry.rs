mod adapters;
mod mutations;

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
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

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
    Read,
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
}

impl ConnectionProfile {
    pub(crate) fn validate(&mut self) -> Result<(), RegistryError> {
        self.id = self.id.trim().to_owned();
        self.name = self.name.trim().to_owned();
        self.endpoint = self.endpoint.trim().to_owned();
        self.namespace = self.namespace.trim().to_owned();

        if self.id.is_empty() {
            return Err(RegistryError::validation("connection id cannot be blank"));
        }
        if self.name.is_empty() {
            return Err(RegistryError::validation("connection name cannot be blank"));
        }
        if self.endpoint.is_empty() {
            return Err(RegistryError::validation("endpoint cannot be blank"));
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
    fn decoded(&self) -> Result<Vec<u8>, RegistryError> {
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
            if !path.starts_with('/')
                || path == "/"
                || path.ends_with('/')
                || path.contains('\0')
                || path
                    .split('/')
                    .skip(1)
                    .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
                    Capability::Read,
                    Capability::Create,
                    Capability::Update,
                    Capability::Delete,
                ],
            })
            .collect()
    }
}

#[derive(Clone, Default)]
pub struct RegistryService {
    sessions: Arc<RwLock<BTreeMap<String, RegistrySession>>>,
    operations: Arc<RwLock<BTreeMap<OperationId, CancellationToken>>>,
}

impl RegistryService {
    pub async fn probe_cancellable(
        &self,
        operation_id: OperationId,
        mut profile: ConnectionProfile,
    ) -> Result<ConnectionProbe, RegistryError> {
        profile.validate()?;
        let adapter = profile.adapter;
        let endpoint = profile.endpoint.clone();
        self.run_operation(operation_id, async move {
            RegistrySession::connect(&profile).await?;
            Ok(ConnectionProbe { adapter, endpoint })
        })
        .await
    }

    pub async fn open(
        &self,
        mut profile: ConnectionProfile,
    ) -> Result<ConnectionSession, RegistryError> {
        profile.validate()?;
        let session = RegistrySession::connect(&profile).await?;
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

    pub async fn close(&self, connection_id: &str) -> Result<(), RegistryError> {
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
    use super::{MutationPhase, OperationId, RegistryError, RegistryErrorCode, RegistryService};

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
}
