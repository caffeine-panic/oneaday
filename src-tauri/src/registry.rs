mod adapters;

use std::{collections::BTreeMap, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

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
    fn validate(&mut self) -> Result<(), RegistryError> {
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
    ) -> Result<ConnectionProbe, String> {
        let profile = ConnectionProfile {
            id: "probe".to_owned(),
            name: "Connection probe".to_owned(),
            adapter,
            endpoint: endpoint.to_owned(),
            namespace: String::new(),
            nacos_api_version: NacosApiVersion::default(),
        };

        RegistrySession::connect(&profile)
            .await
            .map_err(|error| error.message)?;

        Ok(ConnectionProbe {
            adapter,
            endpoint: endpoint.trim().to_owned(),
        })
    }
}

#[derive(Clone, Default)]
pub struct RegistryService {
    sessions: Arc<RwLock<BTreeMap<String, RegistrySession>>>,
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

    pub async fn read(
        &self,
        connection_id: &str,
        address: ResourceAddress,
    ) -> Result<ResourceDocument, RegistryError> {
        let session = self.session(connection_id).await?;
        session.read(address).await
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
}
