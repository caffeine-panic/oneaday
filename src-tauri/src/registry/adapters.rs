mod nacos_auth;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use etcd_client::{
    Certificate, ConnectOptions, GetOptions, Identity, SortOrder, SortTarget,
    TlsOptions as EtcdTlsOptions,
};
use nacos_sdk::api::{
    config::{ConfigService, ConfigServiceBuilder},
    error::Error as NacosError,
    props::ClientProps,
};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::credentials::ConnectionSecret;

use super::{
    AdapterId, AuthenticationMode, ConnectionProfile, EncodedValue, MutationPhase, MutationResult,
    NacosApiVersion, RegistryError, RegistryErrorCode, ResourceAddress, ResourceDocument,
    ResourceMutation, ResourceNode, ResourcePage, ResourceSearchPage, ResourceSearchRequest,
    mutations::{mutate_etcd, mutate_nacos, mutate_zookeeper},
};
use nacos_auth::NacosRequestAuth;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(super) enum RegistrySession {
    Etcd(Box<etcd_client::Client>),
    Zookeeper(zookeeper_client::Client),
    Nacos(NacosSession),
}

#[derive(Clone)]
pub(super) struct NacosSession {
    pub(super) config: ConfigService,
    http: reqwest::Client,
    endpoint: String,
    namespace: String,
    api_version: NacosApiVersion,
    request_auth: NacosRequestAuth,
    _credential: Option<Arc<ConnectionSecret>>,
}

impl NacosSession {
    pub(super) async fn probe_remote(&self) -> Result<(), RegistryError> {
        list_nacos(self, ResourceAddress::Root, None, 1)
            .await
            .map(|_| ())
    }
}

impl RegistrySession {
    pub(super) fn adapter_id(&self) -> AdapterId {
        match self {
            Self::Etcd(_) => AdapterId::Etcd,
            Self::Zookeeper(_) => AdapterId::Zookeeper,
            Self::Nacos(_) => AdapterId::Nacos,
        }
    }

    pub(super) async fn connect(
        profile: &ConnectionProfile,
        secret: Option<ConnectionSecret>,
    ) -> Result<Self, RegistryError> {
        if profile.endpoint.trim().is_empty() {
            return Err(RegistryError::validation("endpoint cannot be blank"));
        }
        if profile.auth.mode != AuthenticationMode::None && secret.is_none() {
            return Err(RegistryError::credential_missing(format!(
                "connection '{}' requires a credential",
                profile.id
            )));
        }
        let secret = secret.map(Arc::new);

        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match profile.adapter {
                AdapterId::Etcd => Self::connect_etcd(profile, secret.as_deref()).await,
                AdapterId::Zookeeper => Self::connect_zookeeper(profile, secret.as_deref()).await,
                AdapterId::Nacos => Self::connect_nacos(profile, secret).await,
            }
        })
        .await
        .map_err(|_| RegistryError::timeout("connection"))?
    }

    pub(super) async fn list(
        &self,
        parent: ResourceAddress,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<ResourcePage, RegistryError> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match self {
                Self::Etcd(client) => {
                    list_etcd(client.as_ref().clone(), parent, cursor, limit).await
                }
                Self::Zookeeper(client) => list_zookeeper(client, parent, cursor, limit).await,
                Self::Nacos(session) => list_nacos(session, parent, cursor, limit).await,
            }
        })
        .await
        .map_err(|_| RegistryError::timeout("resource listing"))?
    }

    pub(super) async fn read(
        &self,
        address: ResourceAddress,
    ) -> Result<ResourceDocument, RegistryError> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match self {
                Self::Etcd(client) => read_etcd(client.as_ref().clone(), address).await,
                Self::Zookeeper(client) => read_zookeeper(client, address).await,
                Self::Nacos(session) => read_nacos(session, address).await,
            }
        })
        .await
        .map_err(|_| RegistryError::timeout("resource read"))?
    }

    pub(super) async fn search(
        &self,
        request: ResourceSearchRequest,
        limit: usize,
    ) -> Result<ResourceSearchPage, RegistryError> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match self {
                Self::Etcd(client) => search_etcd(client.as_ref().clone(), request, limit).await,
                Self::Zookeeper(client) => search_zookeeper(client, request, limit).await,
                Self::Nacos(session) => search_nacos(session, request, limit).await,
            }
        })
        .await
        .map_err(|_| RegistryError::timeout("resource search"))?
    }

    pub(super) async fn mutate(
        &self,
        mutation: ResourceMutation,
        phase: MutationPhase,
    ) -> Result<MutationResult, RegistryError> {
        let timeout_phase = phase.clone();
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match self {
                Self::Etcd(client) => mutate_etcd(client.as_ref().clone(), mutation, &phase).await,
                Self::Zookeeper(client) => mutate_zookeeper(client, mutation, &phase).await,
                Self::Nacos(session) => mutate_nacos(session, mutation, &phase).await,
            }
        })
        .await
        .map_err(|_| timeout_phase.timeout_error())?
    }

    async fn connect_etcd(
        profile: &ConnectionProfile,
        secret: Option<&ConnectionSecret>,
    ) -> Result<Self, RegistryError> {
        let endpoints = profile
            .endpoint
            .split(',')
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
            .map(|endpoint| etcd_endpoint(endpoint, profile.tls.enabled))
            .collect::<Vec<_>>();
        let mut options = ConnectOptions::default()
            .with_connect_timeout(OPERATION_TIMEOUT)
            .with_timeout(OPERATION_TIMEOUT);
        if profile.auth.mode == AuthenticationMode::UsernamePassword {
            options = options.with_user(
                profile.auth.username.clone(),
                required_secret(profile, secret)?.expose().to_owned(),
            );
        }
        if profile.tls.enabled {
            options = options.with_tls(etcd_tls_options(profile).await?);
        }
        let mut client = etcd_client::Client::connect(endpoints, Some(options))
            .await
            .map_err(|error| RegistryError::network(format!("etcd connection failed: {error}")))?;
        client.status().await.map_err(|error| {
            RegistryError::network(format!("etcd status request failed: {error}"))
        })?;
        Ok(Self::Etcd(Box::new(client)))
    }

    async fn connect_zookeeper(
        profile: &ConnectionProfile,
        secret: Option<&ConnectionSecret>,
    ) -> Result<Self, RegistryError> {
        let mut connector = zookeeper_client::Client::connector();
        let digest = if profile.auth.mode == AuthenticationMode::Digest {
            Some(Zeroizing::new(format!(
                "{}:{}",
                profile.auth.username,
                required_secret(profile, secret)?.expose()
            )))
        } else {
            None
        };
        if let Some(digest) = digest.as_ref() {
            connector = connector.with_auth("digest", digest.as_bytes());
        }
        if profile.tls.enabled {
            connector = connector.with_tls(zookeeper_tls_options(profile).await?);
        }
        let connection = if profile.tls.enabled {
            connector.secure_connect(&profile.endpoint).await
        } else {
            connector.connect(&profile.endpoint).await
        };
        connection.map(Self::Zookeeper).map_err(|error| {
            RegistryError::network(format!("ZooKeeper connection failed: {error}"))
        })
    }

    async fn connect_nacos(
        profile: &ConnectionProfile,
        secret: Option<Arc<ConnectionSecret>>,
    ) -> Result<Self, RegistryError> {
        let sdk_namespace = public_namespace_for_sdk(&profile.namespace);
        let http = reqwest::Client::builder()
            .timeout(OPERATION_TIMEOUT)
            .build()
            .map_err(|error| {
                RegistryError::invalid_response(format!("cannot build Nacos HTTP client: {error}"))
            })?;
        let builder = ConfigServiceBuilder::new(
            ClientProps::new()
                .server_addr(&profile.endpoint)
                .namespace(sdk_namespace)
                .app_name("atlas-registry"),
        );
        let (builder, request_auth) =
            nacos_auth::configure(builder, http.clone(), profile, secret.as_ref())?;
        let config = builder
            .build()
            .await
            .map_err(|error| RegistryError::network(format!("Nacos connection failed: {error}")))?;

        match config
            .get_config(
                "__atlas_registry_probe__".to_owned(),
                "DEFAULT_GROUP".to_owned(),
            )
            .await
        {
            Ok(_) | Err(NacosError::ConfigNotFound(_)) => {}
            Err(error) => {
                return Err(RegistryError::network(format!(
                    "Nacos connection failed: {error}"
                )));
            }
        }

        Ok(Self::Nacos(NacosSession {
            config,
            http,
            endpoint: profile.endpoint.clone(),
            namespace: profile.namespace.clone(),
            api_version: profile.nacos_api_version,
            request_auth,
            _credential: secret,
        }))
    }
}

fn required_secret<'a>(
    profile: &ConnectionProfile,
    secret: Option<&'a ConnectionSecret>,
) -> Result<&'a ConnectionSecret, RegistryError> {
    secret.ok_or_else(|| {
        RegistryError::credential_missing(format!(
            "connection '{}' requires a credential",
            profile.id
        ))
    })
}

fn etcd_endpoint(endpoint: &str, tls: bool) -> String {
    if tls && !endpoint.contains("://") {
        format!("https://{endpoint}")
    } else {
        endpoint.to_owned()
    }
}

async fn etcd_tls_options(profile: &ConnectionProfile) -> Result<EtcdTlsOptions, RegistryError> {
    let mut options = EtcdTlsOptions::new().with_enabled_roots();
    if !profile.tls.ca_certificate_path.is_empty() {
        let ca = read_tls_file(&profile.tls.ca_certificate_path, "CA certificate").await?;
        options = options.ca_certificate(Certificate::from_pem(ca));
    }
    if !profile.tls.client_certificate_path.is_empty() {
        let certificate =
            read_tls_file(&profile.tls.client_certificate_path, "client certificate").await?;
        let private_key = Zeroizing::new(
            read_tls_file(&profile.tls.client_key_path, "client private key").await?,
        );
        options = options.identity(Identity::from_pem(certificate, private_key.as_slice()));
    }
    if !profile.tls.server_name.is_empty() {
        options = options.domain_name(profile.tls.server_name.clone());
    }
    Ok(options)
}

async fn zookeeper_tls_options(
    profile: &ConnectionProfile,
) -> Result<zookeeper_client::TlsOptions, RegistryError> {
    let ca = read_tls_text(&profile.tls.ca_certificate_path, "CA certificate").await?;
    let mut options = zookeeper_client::TlsOptions::new()
        .with_pem_ca(&ca)
        .map_err(|error| {
            RegistryError::tls_configuration(format!(
                "ZooKeeper CA certificate is invalid: {error}"
            ))
        })?;
    if !profile.tls.client_certificate_path.is_empty() {
        let certificate =
            read_tls_text(&profile.tls.client_certificate_path, "client certificate").await?;
        let private_key = Zeroizing::new(
            read_tls_text(&profile.tls.client_key_path, "client private key").await?,
        );
        options = options
            .with_pem_identity(&certificate, private_key.as_str())
            .map_err(|error| {
                RegistryError::tls_configuration(format!(
                    "ZooKeeper client identity is invalid: {error}"
                ))
            })?;
    }
    Ok(options)
}

async fn read_tls_file(path: &str, label: &str) -> Result<Vec<u8>, RegistryError> {
    tokio::fs::read(path).await.map_err(|error| {
        RegistryError::tls_configuration(format!("cannot read TLS {label} at '{path}': {error}"))
    })
}

async fn read_tls_text(path: &str, label: &str) -> Result<String, RegistryError> {
    tokio::fs::read_to_string(path).await.map_err(|error| {
        RegistryError::tls_configuration(format!("cannot read TLS {label} at '{path}': {error}"))
    })
}

#[derive(Default)]
struct EtcdChild {
    readable: bool,
    has_children: bool,
}

async fn list_etcd(
    mut client: etcd_client::Client,
    parent: ResourceAddress,
    cursor: Option<String>,
    limit: usize,
) -> Result<ResourcePage, RegistryError> {
    let parent_prefix = match &parent {
        ResourceAddress::Root => Vec::new(),
        ResourceAddress::EtcdPrefix { prefix_base64 } => {
            decode_base64(prefix_base64, "etcd prefix")?
        }
        _ => return Err(adapter_mismatch(AdapterId::Etcd, &parent)),
    };
    let start = match cursor {
        Some(cursor) => decode_base64(&cursor, "etcd page cursor")?,
        None if parent_prefix.is_empty() => vec![0],
        None => parent_prefix.clone(),
    };

    let scan_limit = (limit.saturating_mul(64)).clamp(256, 4096) as i64;
    let mut options = GetOptions::new()
        .with_keys_only()
        .with_limit(scan_limit)
        .with_sort(SortTarget::Key, SortOrder::Ascend);
    options = if parent_prefix.is_empty() {
        options.with_from_key()
    } else {
        options.with_range(prefix_end(&parent_prefix))
    };

    let response = client
        .get(start, Some(options))
        .await
        .map_err(|error| RegistryError::network(format!("etcd list failed: {error}")))?;
    let mut children = BTreeMap::<Vec<u8>, EtcdChild>::new();
    for key_value in response.kvs() {
        let key = key_value.key();
        if let Some((child_key, readable, has_children)) = etcd_immediate_child(&parent_prefix, key)
        {
            let child = children.entry(child_key).or_default();
            child.readable |= readable;
            child.has_children |= has_children;
        }
    }

    let has_unreturned_nodes = children.len() > limit;
    let selected = children.into_iter().take(limit).collect::<Vec<_>>();
    let next_cursor = if response.more() || has_unreturned_nodes {
        selected
            .last()
            .map(|(key, child)| STANDARD.encode(etcd_cursor_after(key, child.has_children)))
    } else {
        None
    };
    let items = selected
        .into_iter()
        .map(|(key, child)| {
            let address = if key.ends_with(b"/") {
                ResourceAddress::EtcdPrefix {
                    prefix_base64: STANDARD.encode(&key),
                }
            } else {
                ResourceAddress::Etcd {
                    key_base64: STANDARD.encode(&key),
                }
            };
            ResourceNode {
                name: display_etcd_name(&parent_prefix, &key),
                address,
                readable: child.readable,
                has_children: Some(child.has_children),
            }
        })
        .collect();

    Ok(ResourcePage {
        parent,
        items,
        next_cursor,
    })
}

async fn search_etcd(
    mut client: etcd_client::Client,
    request: ResourceSearchRequest,
    limit: usize,
) -> Result<ResourceSearchPage, RegistryError> {
    let parent_prefix = match &request.scope {
        ResourceAddress::Root => Vec::new(),
        ResourceAddress::EtcdPrefix { prefix_base64 } => {
            decode_base64(prefix_base64, "etcd search prefix")?
        }
        _ => return Err(adapter_mismatch(AdapterId::Etcd, &request.scope)),
    };
    let start = match &request.cursor {
        Some(cursor) => decode_base64(cursor, "etcd search cursor")?,
        None if parent_prefix.is_empty() => vec![0],
        None => parent_prefix.clone(),
    };
    let scan_limit = (limit.saturating_mul(64)).clamp(256, 4096) as i64;
    let mut options = GetOptions::new()
        .with_keys_only()
        .with_limit(scan_limit)
        .with_sort(SortTarget::Key, SortOrder::Ascend);
    options = if parent_prefix.is_empty() {
        options.with_from_key()
    } else {
        options.with_range(prefix_end(&parent_prefix))
    };
    let response = client
        .get(start, Some(options))
        .await
        .map_err(|error| RegistryError::network(format!("etcd search failed: {error}")))?;
    let query = request.query.to_lowercase();
    let mut items = Vec::new();
    let mut scanned = 0usize;
    let mut last_scanned = None;
    for key_value in response.kvs() {
        scanned += 1;
        let key = key_value.key();
        last_scanned = Some(key.to_vec());
        let name = display_bytes(key);
        if name.to_lowercase().contains(&query) {
            items.push(ResourceNode {
                address: ResourceAddress::Etcd {
                    key_base64: STANDARD.encode(key),
                },
                name,
                readable: true,
                has_children: Some(false),
            });
            if items.len() == limit {
                break;
            }
        }
    }
    let has_more_in_response = scanned < response.kvs().len();
    let next_cursor = (response.more() || has_more_in_response)
        .then(|| last_scanned.map(|key| STANDARD.encode(etcd_cursor_after(&key, false))))
        .flatten();

    Ok(ResourceSearchPage {
        scope: request.scope,
        items,
        exhaustive: next_cursor.is_none(),
        next_cursor,
        scanned,
    })
}

async fn read_etcd(
    mut client: etcd_client::Client,
    address: ResourceAddress,
) -> Result<ResourceDocument, RegistryError> {
    let key = match &address {
        ResourceAddress::Etcd { key_base64 } => decode_base64(key_base64, "etcd key")?,
        ResourceAddress::EtcdPrefix { prefix_base64 } => {
            decode_base64(prefix_base64, "etcd prefix")?
        }
        _ => return Err(adapter_mismatch(AdapterId::Etcd, &address)),
    };
    let response = client
        .get(key.clone(), None)
        .await
        .map_err(|error| RegistryError::network(format!("etcd read failed: {error}")))?;
    let key_value = response
        .kvs()
        .first()
        .ok_or_else(|| RegistryError::not_found("etcd key does not exist"))?;
    let mut metadata = BTreeMap::new();
    metadata.insert(
        "createRevision".to_owned(),
        key_value.create_revision().to_string(),
    );
    metadata.insert(
        "modRevision".to_owned(),
        key_value.mod_revision().to_string(),
    );
    metadata.insert("lease".to_owned(), key_value.lease().to_string());

    Ok(ResourceDocument {
        address,
        name: display_bytes(&key),
        value: EncodedValue::try_from_inline_bytes(key_value.value())?,
        content_type: infer_content_type(&key),
        version: Some(key_value.mod_revision().to_string()),
        metadata,
    })
}

async fn list_zookeeper(
    client: &zookeeper_client::Client,
    parent: ResourceAddress,
    cursor: Option<String>,
    limit: usize,
) -> Result<ResourcePage, RegistryError> {
    let path = match &parent {
        ResourceAddress::Root => "/",
        ResourceAddress::Zookeeper { path } => path,
        _ => return Err(adapter_mismatch(AdapterId::Zookeeper, &parent)),
    };
    let offset = parse_page_offset(cursor)?;
    let mut children = client
        .list_children(path)
        .await
        .map_err(|error| RegistryError::network(format!("ZooKeeper list failed: {error}")))?;
    children.sort();
    let items = children
        .iter()
        .skip(offset)
        .take(limit)
        .map(|name| ResourceNode {
            address: ResourceAddress::Zookeeper {
                path: join_zookeeper_path(path, name),
            },
            name: name.clone(),
            readable: true,
            has_children: None,
        })
        .collect::<Vec<_>>();
    let consumed = offset + items.len();
    let next_cursor = (consumed < children.len()).then(|| consumed.to_string());

    Ok(ResourcePage {
        parent,
        items,
        next_cursor,
    })
}

struct ZookeeperSearchWindow {
    items: Vec<String>,
    scanned: usize,
    next_offset: Option<usize>,
}

fn search_zookeeper_children(
    mut children: Vec<String>,
    _path: &str,
    query: &str,
    offset: usize,
    limit: usize,
) -> ZookeeperSearchWindow {
    children.sort();
    let query = query.to_lowercase();
    let mut items = Vec::new();
    let mut scanned = 0usize;
    let mut next_offset = None;
    for (index, name) in children.iter().enumerate().skip(offset) {
        scanned += 1;
        if name.to_lowercase().contains(&query) {
            items.push(name.clone());
            if items.len() == limit {
                next_offset = (index + 1 < children.len()).then_some(index + 1);
                break;
            }
        }
    }
    ZookeeperSearchWindow {
        items,
        scanned,
        next_offset,
    }
}

async fn search_zookeeper(
    client: &zookeeper_client::Client,
    request: ResourceSearchRequest,
    limit: usize,
) -> Result<ResourceSearchPage, RegistryError> {
    let path = match &request.scope {
        ResourceAddress::Root => "/",
        ResourceAddress::Zookeeper { path } => path,
        _ => return Err(adapter_mismatch(AdapterId::Zookeeper, &request.scope)),
    };
    let offset = parse_page_offset(request.cursor.clone())?;
    let children = client
        .list_children(path)
        .await
        .map_err(|error| RegistryError::network(format!("ZooKeeper search failed: {error}")))?;
    let window = search_zookeeper_children(children, path, &request.query, offset, limit);
    let items = window
        .items
        .into_iter()
        .map(|name| ResourceNode {
            address: ResourceAddress::Zookeeper {
                path: join_zookeeper_path(path, &name),
            },
            name,
            readable: true,
            has_children: None,
        })
        .collect();
    Ok(ResourceSearchPage {
        scope: request.scope,
        items,
        next_cursor: window.next_offset.map(|offset| offset.to_string()),
        scanned: window.scanned,
        exhaustive: window.next_offset.is_none(),
    })
}

async fn read_zookeeper(
    client: &zookeeper_client::Client,
    address: ResourceAddress,
) -> Result<ResourceDocument, RegistryError> {
    let path = match &address {
        ResourceAddress::Zookeeper { path } => path.clone(),
        _ => return Err(adapter_mismatch(AdapterId::Zookeeper, &address)),
    };
    let (data, stat) = client.get_data(&path).await.map_err(|error| match error {
        zookeeper_client::Error::NoNode => {
            RegistryError::not_found("ZooKeeper znode does not exist")
        }
        other => RegistryError::network(format!("ZooKeeper read failed: {other}")),
    })?;
    let mut metadata = BTreeMap::new();
    metadata.insert("createdZxid".to_owned(), stat.czxid.to_string());
    metadata.insert("modifiedZxid".to_owned(), stat.mzxid.to_string());
    metadata.insert("createdAtMs".to_owned(), stat.ctime.to_string());
    metadata.insert("modifiedAtMs".to_owned(), stat.mtime.to_string());
    metadata.insert("children".to_owned(), stat.num_children.to_string());
    metadata.insert(
        "ephemeralOwner".to_owned(),
        stat.ephemeral_owner.to_string(),
    );

    Ok(ResourceDocument {
        address,
        name: path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("/")
            .to_owned(),
        value: EncodedValue::try_from_inline_bytes(&data)?,
        content_type: infer_content_type(path.as_bytes()),
        version: Some(stat.version.to_string()),
        metadata,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosEnvelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosConfigPage {
    #[serde(default)]
    page_number: usize,
    #[serde(default)]
    pages_available: usize,
    #[serde(default)]
    page_items: Vec<NacosConfigItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosConfigItem {
    data_id: String,
    #[serde(alias = "group")]
    group_name: String,
}

async fn list_nacos(
    session: &NacosSession,
    parent: ResourceAddress,
    cursor: Option<String>,
    limit: usize,
) -> Result<ResourcePage, RegistryError> {
    if parent != ResourceAddress::Root {
        return Err(RegistryError::unsupported(
            "Nacos configuration resources are listed as a flat, paginated collection",
        ));
    }
    let page_number = parse_page_number(cursor)?;
    let page = match session.api_version {
        NacosApiVersion::V2 => fetch_nacos_v2_page(session, page_number, limit, "").await?,
        NacosApiVersion::V3 => fetch_nacos_v3_page(session, page_number, limit, "").await?,
    };
    let next_cursor =
        (page.page_number < page.pages_available).then(|| (page.page_number + 1).to_string());
    let items = page
        .page_items
        .into_iter()
        .map(|item| {
            let group = item.group_name;
            let data_id = item.data_id;
            ResourceNode {
                address: ResourceAddress::NacosConfig {
                    group: group.clone(),
                    data_id: data_id.clone(),
                },
                name: format!("{group} / {data_id}"),
                readable: true,
                has_children: Some(false),
            }
        })
        .collect();

    Ok(ResourcePage {
        parent,
        items,
        next_cursor,
    })
}

async fn search_nacos(
    session: &NacosSession,
    request: ResourceSearchRequest,
    limit: usize,
) -> Result<ResourceSearchPage, RegistryError> {
    if request.scope != ResourceAddress::Root {
        return Err(RegistryError::unsupported(
            "Nacos search uses the flat configuration scope",
        ));
    }
    let page_number = parse_page_number(request.cursor.clone())?;
    let page = match session.api_version {
        NacosApiVersion::V2 => {
            fetch_nacos_v2_page(session, page_number, limit, &request.query).await?
        }
        NacosApiVersion::V3 => {
            fetch_nacos_v3_page(session, page_number, limit, &request.query).await?
        }
    };
    let scanned = page.page_items.len();
    let next_cursor =
        (page.page_number < page.pages_available).then(|| (page.page_number + 1).to_string());
    let items = page
        .page_items
        .into_iter()
        .map(|item| {
            let group = item.group_name;
            let data_id = item.data_id;
            ResourceNode {
                address: ResourceAddress::NacosConfig {
                    group: group.clone(),
                    data_id: data_id.clone(),
                },
                name: format!("{group} / {data_id}"),
                readable: true,
                has_children: Some(false),
            }
        })
        .collect();
    Ok(ResourceSearchPage {
        scope: request.scope,
        items,
        exhaustive: next_cursor.is_none(),
        next_cursor,
        scanned,
    })
}

async fn fetch_nacos_v2_page(
    session: &NacosSession,
    page_number: usize,
    limit: usize,
    data_id: &str,
) -> Result<NacosConfigPage, RegistryError> {
    let url = format!(
        "{}/nacos/v1/cs/configs",
        nacos_server_base(&session.endpoint)
    );
    session
        .request_auth
        .apply(session.http.get(url))
        .query(&[
            ("search", "blur".to_owned()),
            ("dataId", data_id.to_owned()),
            ("group", String::new()),
            (
                "tenant",
                public_namespace_for_sdk(&session.namespace).to_owned(),
            ),
            ("pageNo", page_number.to_string()),
            ("pageSize", limit.to_string()),
        ])
        .send()
        .await
        .map_err(nacos_http_error)?
        .error_for_status()
        .map_err(nacos_http_error)?
        .json::<NacosConfigPage>()
        .await
        .map_err(|error| {
            RegistryError::invalid_response(format!(
                "invalid Nacos 2.x list response: {}",
                error.without_url()
            ))
        })
}

async fn fetch_nacos_v3_page(
    session: &NacosSession,
    page_number: usize,
    limit: usize,
    data_id: &str,
) -> Result<NacosConfigPage, RegistryError> {
    let url = format!(
        "{}/nacos/v3/admin/cs/config/list",
        nacos_server_base(&session.endpoint)
    );
    let envelope = session
        .request_auth
        .apply(session.http.get(url))
        .query(&[
            ("pageNo", page_number.to_string()),
            ("pageSize", limit.to_string()),
            (
                "namespaceId",
                public_namespace_for_api(&session.namespace).to_owned(),
            ),
            ("dataId", data_id.to_owned()),
            ("groupName", String::new()),
            ("configDetail", String::new()),
            ("search", "blur".to_owned()),
        ])
        .send()
        .await
        .map_err(nacos_http_error)?
        .error_for_status()
        .map_err(nacos_http_error)?
        .json::<NacosEnvelope<NacosConfigPage>>()
        .await
        .map_err(|error| {
            RegistryError::invalid_response(format!(
                "invalid Nacos 3.x list response: {}",
                error.without_url()
            ))
        })?;
    if envelope.code != 0 && envelope.code != 200 {
        return Err(RegistryError::new(
            RegistryErrorCode::Network,
            format!(
                "Nacos Admin API rejected the list request: {}",
                envelope.message
            ),
            false,
        ));
    }
    envelope
        .data
        .ok_or_else(|| RegistryError::invalid_response("Nacos list response has no data"))
}

async fn read_nacos(
    session: &NacosSession,
    address: ResourceAddress,
) -> Result<ResourceDocument, RegistryError> {
    let (group, data_id) = match &address {
        ResourceAddress::NacosConfig { group, data_id } => (group, data_id),
        _ => return Err(adapter_mismatch(AdapterId::Nacos, &address)),
    };
    let response = session
        .config
        .get_config(data_id.clone(), group.clone())
        .await
        .map_err(|error| match error {
            NacosError::ConfigNotFound(_) => {
                RegistryError::not_found("Nacos config does not exist")
            }
            other => RegistryError::network(format!("Nacos config read failed: {other}")),
        })?;
    let mut metadata = BTreeMap::new();
    metadata.insert("namespace".to_owned(), response.namespace().clone());
    metadata.insert("group".to_owned(), response.group().clone());
    metadata.insert("md5".to_owned(), response.md5().clone());

    Ok(ResourceDocument {
        address,
        name: response.data_id().clone(),
        value: EncodedValue::try_from_inline_bytes(response.content().as_bytes())?,
        content_type: Some(response.content_type().clone()),
        version: Some(response.md5().clone()),
        metadata,
    })
}

fn decode_base64(value: &str, label: &str) -> Result<Vec<u8>, RegistryError> {
    STANDARD
        .decode(value)
        .map_err(|_| RegistryError::validation(format!("{label} is not valid base64")))
}

fn etcd_immediate_child(parent_prefix: &[u8], key: &[u8]) -> Option<(Vec<u8>, bool, bool)> {
    let remaining = key.strip_prefix(parent_prefix)?;
    if remaining.is_empty() {
        return None;
    }
    let separator = if parent_prefix.is_empty() && remaining.first() == Some(&b'/') {
        remaining[1..]
            .iter()
            .position(|byte| *byte == b'/')
            .map(|index| index + 1)
    } else {
        remaining.iter().position(|byte| *byte == b'/')
    };
    match separator {
        Some(index) => {
            let mut child = parent_prefix.to_vec();
            child.extend_from_slice(&remaining[..=index]);
            Some((child.clone(), key == child, true))
        }
        None => Some((key.to_vec(), true, false)),
    }
}

fn etcd_cursor_after(key: &[u8], has_children: bool) -> Vec<u8> {
    if has_children {
        prefix_end(key)
    } else {
        let mut next = key.to_vec();
        next.push(0);
        next
    }
}

fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last + 1);
            return end;
        }
    }
    vec![0]
}

fn display_etcd_name(parent_prefix: &[u8], key: &[u8]) -> String {
    let relative = if parent_prefix.is_empty() {
        key
    } else {
        key.strip_prefix(parent_prefix).unwrap_or(key)
    };
    display_bytes(relative)
}

fn display_bytes(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .unwrap_or_else(|_| format!("base64:{}", STANDARD.encode(bytes)))
}

fn infer_content_type(name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?.to_ascii_lowercase();
    let content_type = if name.ends_with(".json") {
        "json"
    } else if name.ends_with(".yaml") || name.ends_with(".yml") {
        "yaml"
    } else if name.ends_with(".toml") {
        "toml"
    } else if name.ends_with(".xml") {
        "xml"
    } else if name.ends_with(".properties") {
        "properties"
    } else {
        return None;
    };
    Some(content_type.to_owned())
}

fn parse_page_offset(cursor: Option<String>) -> Result<usize, RegistryError> {
    cursor
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| RegistryError::validation("page cursor is invalid"))
        })
        .transpose()
        .map(|value| value.unwrap_or(0))
}

fn parse_page_number(cursor: Option<String>) -> Result<usize, RegistryError> {
    let page = parse_page_offset(cursor)?.max(1);
    Ok(page)
}

fn join_zookeeper_path(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{}/{child}", parent.trim_end_matches('/'))
    }
}

fn public_namespace_for_sdk(namespace: &str) -> &str {
    if namespace.is_empty() || namespace == "public" {
        ""
    } else {
        namespace
    }
}

fn public_namespace_for_api(namespace: &str) -> &str {
    if namespace.is_empty() {
        "public"
    } else {
        namespace
    }
}

fn nacos_server_base(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    let endpoint = endpoint.strip_suffix("/nacos").unwrap_or(endpoint);
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_owned()
    } else {
        format!("http://{endpoint}")
    }
}

fn nacos_http_error(error: reqwest::Error) -> RegistryError {
    RegistryError::network(format!(
        "Nacos Admin API request failed: {}",
        error.without_url()
    ))
}

fn adapter_mismatch(adapter: AdapterId, address: &ResourceAddress) -> RegistryError {
    RegistryError::validation(format!(
        "resource address {address:?} does not belong to {adapter:?}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        etcd_cursor_after, etcd_immediate_child, nacos_http_error, search_zookeeper_children,
    };

    #[test]
    fn etcd_cursor_keeps_exact_keys_and_folder_prefixes_in_separate_ranges() {
        let exact = etcd_immediate_child(b"", b"a").expect("exact key should be visible");
        let dotted = etcd_immediate_child(b"", b"a.foo").expect("sibling key should be visible");
        let nested = etcd_immediate_child(b"", b"a/x").expect("nested key should be visible");

        assert_eq!(exact, (b"a".to_vec(), true, false));
        assert_eq!(dotted, (b"a.foo".to_vec(), true, false));
        assert_eq!(nested, (b"a/".to_vec(), false, true));
        assert!(etcd_cursor_after(&exact.0, exact.2) < dotted.0);
        assert!(etcd_cursor_after(&dotted.0, dotted.2) < b"a/x".to_vec());
        assert!(etcd_cursor_after(&nested.0, nested.2) > b"a/x".to_vec());
    }

    #[test]
    fn zookeeper_search_is_identifier_only_bounded_and_cursor_driven() {
        let children = vec![
            "alpha-service".to_owned(),
            "beta".to_owned(),
            "config-service".to_owned(),
            "database".to_owned(),
            "service-z".to_owned(),
        ];

        let first = search_zookeeper_children(children.clone(), "/services", "SERVICE", 0, 2);
        assert_eq!(first.items, vec!["alpha-service", "config-service"]);
        assert_eq!(first.scanned, 3);
        assert_eq!(first.next_offset, Some(3));

        let second = search_zookeeper_children(children, "/services", "service", 3, 2);
        assert_eq!(second.items, vec!["service-z"]);
        assert_eq!(second.scanned, 2);
        assert_eq!(second.next_offset, None);
    }

    #[tokio::test]
    async fn nacos_http_errors_never_return_secret_query_parameters() {
        let error = reqwest::Client::new()
            .get("http://[::1/nacos/v1/cs/configs?accessToken=TOP_SECRET_TOKEN")
            .send()
            .await
            .expect_err("invalid URL should reject the request before network access");

        let sanitized = nacos_http_error(error);
        assert!(!sanitized.message.contains("TOP_SECRET_TOKEN"));
        assert!(!sanitized.message.contains("accessToken"));
    }
}
