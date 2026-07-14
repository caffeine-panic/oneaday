use std::{collections::BTreeMap, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use etcd_client::{GetOptions, SortOrder, SortTarget};
use nacos_sdk::api::{
    config::{ConfigService, ConfigServiceBuilder},
    error::Error as NacosError,
    props::ClientProps,
};
use serde::Deserialize;

use super::{
    AdapterId, ConnectionProfile, EncodedValue, NacosApiVersion, RegistryError, RegistryErrorCode,
    ResourceAddress, ResourceDocument, ResourceNode, ResourcePage,
};

const OPERATION_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub(super) enum RegistrySession {
    Etcd(Box<etcd_client::Client>),
    Zookeeper(zookeeper_client::Client),
    Nacos(NacosSession),
}

#[derive(Clone)]
pub(super) struct NacosSession {
    config: ConfigService,
    http: reqwest::Client,
    endpoint: String,
    namespace: String,
    api_version: NacosApiVersion,
}

impl RegistrySession {
    pub(super) async fn connect(profile: &ConnectionProfile) -> Result<Self, RegistryError> {
        if profile.endpoint.trim().is_empty() {
            return Err(RegistryError::validation("endpoint cannot be blank"));
        }

        tokio::time::timeout(OPERATION_TIMEOUT, async {
            match profile.adapter {
                AdapterId::Etcd => Self::connect_etcd(profile).await,
                AdapterId::Zookeeper => Self::connect_zookeeper(profile).await,
                AdapterId::Nacos => Self::connect_nacos(profile).await,
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

    async fn connect_etcd(profile: &ConnectionProfile) -> Result<Self, RegistryError> {
        let endpoints = profile
            .endpoint
            .split(',')
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
            .collect::<Vec<_>>();
        let mut client = etcd_client::Client::connect(endpoints, None)
            .await
            .map_err(|error| RegistryError::network(format!("etcd connection failed: {error}")))?;
        client.status().await.map_err(|error| {
            RegistryError::network(format!("etcd status request failed: {error}"))
        })?;
        Ok(Self::Etcd(Box::new(client)))
    }

    async fn connect_zookeeper(profile: &ConnectionProfile) -> Result<Self, RegistryError> {
        zookeeper_client::Client::connect(&profile.endpoint)
            .await
            .map(Self::Zookeeper)
            .map_err(|error| {
                RegistryError::network(format!("ZooKeeper connection failed: {error}"))
            })
    }

    async fn connect_nacos(profile: &ConnectionProfile) -> Result<Self, RegistryError> {
        let sdk_namespace = public_namespace_for_sdk(&profile.namespace);
        let config = ConfigServiceBuilder::new(
            ClientProps::new()
                .server_addr(&profile.endpoint)
                .namespace(sdk_namespace)
                .app_name("atlas-registry"),
        )
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

        let http = reqwest::Client::builder()
            .timeout(OPERATION_TIMEOUT)
            .build()
            .map_err(|error| {
                RegistryError::invalid_response(format!("cannot build Nacos HTTP client: {error}"))
            })?;

        Ok(Self::Nacos(NacosSession {
            config,
            http,
            endpoint: profile.endpoint.clone(),
            namespace: profile.namespace.clone(),
            api_version: profile.nacos_api_version,
        }))
    }
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

async fn read_zookeeper(
    client: &zookeeper_client::Client,
    address: ResourceAddress,
) -> Result<ResourceDocument, RegistryError> {
    let path = match &address {
        ResourceAddress::Zookeeper { path } => path.clone(),
        _ => return Err(adapter_mismatch(AdapterId::Zookeeper, &address)),
    };
    let (data, stat) = client
        .get_data(&path)
        .await
        .map_err(|error| RegistryError::network(format!("ZooKeeper read failed: {error}")))?;
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
        NacosApiVersion::V2 => fetch_nacos_v2_page(session, page_number, limit).await?,
        NacosApiVersion::V3 => fetch_nacos_v3_page(session, page_number, limit).await?,
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

async fn fetch_nacos_v2_page(
    session: &NacosSession,
    page_number: usize,
    limit: usize,
) -> Result<NacosConfigPage, RegistryError> {
    let url = format!(
        "{}/nacos/v1/cs/configs",
        nacos_server_base(&session.endpoint)
    );
    session
        .http
        .get(url)
        .query(&[
            ("search", "blur".to_owned()),
            ("dataId", String::new()),
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
            RegistryError::invalid_response(format!("invalid Nacos 2.x list response: {error}"))
        })
}

async fn fetch_nacos_v3_page(
    session: &NacosSession,
    page_number: usize,
    limit: usize,
) -> Result<NacosConfigPage, RegistryError> {
    let url = format!(
        "{}/nacos/v3/admin/cs/config/list",
        nacos_server_base(&session.endpoint)
    );
    let envelope = session
        .http
        .get(url)
        .query(&[
            ("pageNo", page_number.to_string()),
            ("pageSize", limit.to_string()),
            (
                "namespaceId",
                public_namespace_for_api(&session.namespace).to_owned(),
            ),
            ("dataId", String::new()),
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
            RegistryError::invalid_response(format!("invalid Nacos 3.x list response: {error}"))
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
    RegistryError::network(format!("Nacos Admin API request failed: {error}"))
}

fn adapter_mismatch(adapter: AdapterId, address: &ResourceAddress) -> RegistryError {
    RegistryError::validation(format!(
        "resource address {address:?} does not belong to {adapter:?}"
    ))
}
