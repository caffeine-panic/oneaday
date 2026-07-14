use nacos_sdk::api::{config::ConfigServiceBuilder, props::ClientProps};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterId {
    Etcd,
    Zookeeper,
    Nacos,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterDescriptor {
    pub id: AdapterId,
    pub status: &'static str,
    pub capabilities: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProbe {
    pub adapter: AdapterId,
    pub endpoint: String,
}

trait RegistryAdapter {
    async fn probe(&self, endpoint: &str) -> Result<(), String>;
}

struct EtcdAdapter;
struct ZookeeperAdapter;
struct NacosAdapter;

impl RegistryAdapter for EtcdAdapter {
    async fn probe(&self, endpoint: &str) -> Result<(), String> {
        etcd_client::Client::connect([endpoint], None)
            .await
            .map(|_| ())
            .map_err(|error| format!("etcd connection failed: {error}"))
    }
}

impl RegistryAdapter for ZookeeperAdapter {
    async fn probe(&self, endpoint: &str) -> Result<(), String> {
        zookeeper_client::Client::connect(endpoint)
            .await
            .map(|_| ())
            .map_err(|error| format!("ZooKeeper connection failed: {error}"))
    }
}

impl RegistryAdapter for NacosAdapter {
    async fn probe(&self, endpoint: &str) -> Result<(), String> {
        let config_service = ConfigServiceBuilder::new(
            ClientProps::new()
                .server_addr(endpoint)
                .namespace("")
                .app_name("atlas-registry"),
        )
        .build()
        .await
        .map_err(|error| format!("Nacos connection failed: {error}"))?;

        config_service
            .get_config(
                "__atlas_registry_probe__".to_owned(),
                "DEFAULT_GROUP".to_owned(),
            )
            .await
            .map(|_| ())
            .map_err(|error| format!("Nacos connection failed: {error}"))
    }
}

#[derive(Default)]
pub struct RegistryCatalog;

impl RegistryCatalog {
    pub fn descriptors(&self) -> Vec<AdapterDescriptor> {
        vec![
            AdapterDescriptor {
                id: AdapterId::Etcd,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "watch",
                    "lease",
                    "transaction",
                ],
            },
            AdapterDescriptor {
                id: AdapterId::Zookeeper,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "watch",
                    "acl",
                    "ephemeral",
                ],
            },
            AdapterDescriptor {
                id: AdapterId::Nacos,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "listen",
                    "namespace",
                    "service",
                ],
            },
        ]
    }

    pub async fn probe(
        &self,
        adapter: AdapterId,
        endpoint: &str,
    ) -> Result<ConnectionProbe, String> {
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            return Err("endpoint cannot be blank".into());
        }

        match adapter {
            AdapterId::Etcd => EtcdAdapter.probe(endpoint).await?,
            AdapterId::Zookeeper => ZookeeperAdapter.probe(endpoint).await?,
            AdapterId::Nacos => NacosAdapter.probe(endpoint).await?,
        }

        Ok(ConnectionProbe {
            adapter,
            endpoint: endpoint.to_owned(),
        })
    }
}
