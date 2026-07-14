mod adapters;

use std::{collections::BTreeMap, future::Future, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueEncoding {
    Utf8,
    Base64,
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
                capabilities: vec![Capability::Probe, Capability::Browse, Capability::Read],
            })
            .collect()
    }

    pub async fn probe(
        &self,
        adapter: AdapterId,
        endpoint: &str,
    ) -> Result<ConnectionProbe, RegistryError> {
        let profile = ConnectionProfile {
            id: "probe".to_owned(),
            name: "Connection probe".to_owned(),
            adapter,
            endpoint: endpoint.to_owned(),
            namespace: String::new(),
            nacos_api_version: NacosApiVersion::default(),
        };

        RegistrySession::connect(&profile).await?;

        Ok(ConnectionProbe {
            adapter,
            endpoint: endpoint.trim().to_owned(),
        })
    }
}

#[derive(Clone, Default)]
pub struct RegistryService {
    sessions: Arc<RwLock<BTreeMap<String, RegistrySession>>>,
    operations: Arc<RwLock<BTreeMap<OperationId, CancellationToken>>>,
}

impl RegistryService {
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
            () = token.cancelled() => Err(RegistryError::cancelled()),
        };
        self.operations.write().await.remove(&operation_id);
        result
    }
}
