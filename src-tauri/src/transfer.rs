use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{RwLock, Semaphore},
    task::JoinSet,
};

use crate::registry::{
    AdapterId, MutationValue, RegistryError, RegistryErrorCode, RegistryService, ResourceAddress,
    ResourceDocument, ResourceMutation, ResourceSnapshot,
};

const EXPORT_FORMAT: &str = "atlas-registry-export";
const EXPORT_VERSION: u32 = 1;
pub(crate) const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMPORT_RESOURCES: usize = 50;
const IMPORT_PLAN_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportFile {
    format: String,
    version: u32,
    exported_at_ms: u64,
    include_values: bool,
    resources: Vec<ExportResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResource {
    address: ResourceAddress,
    name: String,
    version: Option<String>,
    content_type: Option<String>,
    metadata: BTreeMap<String, String>,
    snapshot: ResourceSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<MutationValue>,
}

pub(crate) fn build_export_file(
    document: &ResourceDocument,
    include_value: bool,
    exported_at_ms: u64,
) -> Result<Vec<u8>, RegistryError> {
    let export = ExportFile {
        format: EXPORT_FORMAT.to_owned(),
        version: EXPORT_VERSION,
        exported_at_ms,
        include_values: include_value,
        resources: vec![ExportResource {
            address: document.address.clone(),
            name: document.name.clone(),
            version: document.version.clone(),
            content_type: document.content_type.clone(),
            metadata: document.metadata.clone(),
            snapshot: document.snapshot()?,
            value: include_value.then(|| MutationValue {
                content: document.value.content.clone(),
                encoding: document.value.encoding,
            }),
        }],
    };
    serde_json::to_vec_pretty(&export)
        .map_err(|error| RegistryError::storage(format!("cannot serialize export file: {error}")))
}

pub(crate) fn suggested_export_file_name(document: &ResourceDocument) -> String {
    let mut safe_name = document
        .name
        .chars()
        .take(80)
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    safe_name = safe_name.trim_matches(['.', '_']).to_owned();
    if safe_name.is_empty() {
        safe_name = "resource".to_owned();
    }
    format!("atlas-{safe_name}.json")
}

fn parse_import_file(bytes: &[u8]) -> Result<ExportFile, RegistryError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(RegistryError::validation(format!(
            "import file is larger than {} MiB",
            MAX_IMPORT_BYTES / (1024 * 1024)
        )));
    }
    let export: ExportFile = serde_json::from_slice(bytes)
        .map_err(|error| RegistryError::validation(format!("invalid import JSON: {error}")))?;
    if export.format != EXPORT_FORMAT || export.version != EXPORT_VERSION {
        return Err(RegistryError::validation(
            "file is not a supported Atlas Registry export",
        ));
    }
    if export.resources.is_empty() {
        return Err(RegistryError::validation(
            "import file contains no resources",
        ));
    }
    if export.resources.len() > MAX_IMPORT_RESOURCES {
        return Err(RegistryError::validation(format!(
            "import file contains more than {MAX_IMPORT_RESOURCES} resources"
        )));
    }

    let mut addresses = BTreeSet::new();
    for resource in &export.resources {
        let address_key = serde_json::to_string(&resource.address).map_err(|error| {
            RegistryError::validation(format!("cannot validate import address: {error}"))
        })?;
        if address_key.len() > 4096 {
            return Err(RegistryError::validation(
                "import resource address exceeds the 4096-byte safety limit",
            ));
        }
        if !addresses.insert(address_key) {
            return Err(RegistryError::validation(
                "import file contains a duplicate resource address",
            ));
        }
        let validation_value = resource.value.clone().unwrap_or(MutationValue {
            content: String::new(),
            encoding: crate::registry::ValueEncoding::Utf8,
        });
        ResourceMutation::Create {
            address: resource.address.clone(),
            value: validation_value,
            content_type: resource.content_type.clone(),
        }
        .validate()?;
        if resource.snapshot.sha256.len() != 64
            || !resource
                .snapshot
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RegistryError::validation(
                "import resource snapshot has an invalid SHA-256 digest",
            ));
        }
        if resource.snapshot.version != resource.version {
            return Err(RegistryError::validation(
                "import resource version does not match its snapshot version",
            ));
        }

        match (&resource.value, export.include_values) {
            (Some(_), false) => {
                return Err(RegistryError::validation(
                    "metadata-only export unexpectedly contains resource values",
                ));
            }
            (None, true) => {
                return Err(RegistryError::validation(
                    "value export is missing a resource value",
                ));
            }
            (Some(value), true) => {
                let bytes = value.decoded()?;
                let actual = ResourceSnapshot::from_bytes(&bytes, resource.version.clone());
                if actual.sha256 != resource.snapshot.sha256
                    || actual.size_bytes != resource.snapshot.size_bytes
                    || actual.encoding != resource.snapshot.encoding
                {
                    return Err(RegistryError::validation(format!(
                        "resource '{}' does not match its exported snapshot",
                        display_address(&resource.address)
                    )));
                }
            }
            (None, false) => {}
        }
    }
    Ok(export)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportAction {
    Create,
    Update,
    SkippedNoValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreviewItem {
    pub address: ResourceAddress,
    pub name: String,
    pub action: ImportAction,
    pub size_bytes: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub plan_id: String,
    pub file_name: String,
    pub resources: Vec<ImportPreviewItem>,
    pub creates: usize,
    pub updates: usize,
    pub skipped: usize,
    pub expires_in_seconds: u64,
}

#[derive(Clone)]
pub(crate) struct ImportPlanEntry {
    pub preview: ImportPreviewItem,
    pub mutation: ResourceMutation,
}

pub(crate) struct ImportPlan {
    pub entries: Vec<ImportPlanEntry>,
}

struct StoredImportPlan {
    connection_id: String,
    expires_at: Instant,
    plan: ImportPlan,
}

#[derive(Clone, Default)]
pub struct TransferService {
    plans: Arc<RwLock<BTreeMap<String, StoredImportPlan>>>,
}

impl TransferService {
    pub async fn prepare_import(
        &self,
        registry: &RegistryService,
        connection_id: &str,
        file_name: String,
        bytes: &[u8],
    ) -> Result<ImportPreview, RegistryError> {
        let export = parse_import_file(bytes)?;
        let adapter = registry.connection_adapter(connection_id).await?;
        let mut preview_items = Vec::with_capacity(export.resources.len());
        let mut entries = Vec::new();
        let mut creates = 0;
        let mut updates = 0;
        let mut skipped = 0;

        let semaphore = Arc::new(Semaphore::new(8));
        let mut reads = JoinSet::new();
        let mut resolved = Vec::with_capacity(export.resources.len());
        for (index, resource) in export.resources.into_iter().enumerate() {
            validate_adapter_address(adapter, &resource.address)?;
            if resource.value.is_none() {
                resolved.push((index, resource, None));
                continue;
            }
            let registry = registry.clone();
            let connection_id = connection_id.to_owned();
            let semaphore = semaphore.clone();
            reads.spawn(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .expect("import read semaphore is never closed");
                let current = registry
                    .read(&connection_id, resource.address.clone())
                    .await;
                (index, resource, Some(current))
            });
        }
        while let Some(result) = reads.join_next().await {
            resolved.push(result.map_err(|error| {
                RegistryError::storage(format!("import preview task failed: {error}"))
            })?);
        }
        resolved.sort_by_key(|(index, _, _)| *index);

        for (_, resource, current) in resolved {
            let display_name = display_address(&resource.address);
            let Some(value) = resource.value else {
                skipped += 1;
                preview_items.push(ImportPreviewItem {
                    address: resource.address,
                    name: display_name,
                    action: ImportAction::SkippedNoValue,
                    size_bytes: resource.snapshot.size_bytes,
                    sha256: resource.snapshot.sha256,
                });
                continue;
            };
            let (action, mutation) =
                match current.expect("value-bearing import resources always have a read result") {
                    Ok(current) => {
                        let expected_version = current.version.ok_or_else(|| {
                            RegistryError::validation(
                                "existing resource does not expose a conditional update version",
                            )
                        })?;
                        updates += 1;
                        (
                            ImportAction::Update,
                            ResourceMutation::Update {
                                address: resource.address.clone(),
                                value,
                                content_type: resource.content_type,
                                expected_version,
                            },
                        )
                    }
                    Err(error) if error.code == RegistryErrorCode::NotFound => {
                        creates += 1;
                        (
                            ImportAction::Create,
                            ResourceMutation::Create {
                                address: resource.address.clone(),
                                value,
                                content_type: resource.content_type,
                            },
                        )
                    }
                    Err(error) => return Err(error),
                };
            let preview = ImportPreviewItem {
                address: resource.address,
                name: display_name,
                action,
                size_bytes: resource.snapshot.size_bytes,
                sha256: resource.snapshot.sha256,
            };
            preview_items.push(preview.clone());
            entries.push(ImportPlanEntry { preview, mutation });
        }

        self.purge_expired().await;
        let plan_id = next_plan_id();
        self.plans.write().await.insert(
            plan_id.clone(),
            StoredImportPlan {
                connection_id: connection_id.to_owned(),
                expires_at: Instant::now() + IMPORT_PLAN_TTL,
                plan: ImportPlan { entries },
            },
        );
        Ok(ImportPreview {
            plan_id,
            file_name,
            resources: preview_items,
            creates,
            updates,
            skipped,
            expires_in_seconds: IMPORT_PLAN_TTL.as_secs(),
        })
    }

    pub(crate) async fn take_plan(
        &self,
        plan_id: &str,
        connection_id: &str,
    ) -> Result<ImportPlan, RegistryError> {
        self.purge_expired().await;
        let mut plans = self.plans.write().await;
        match plans.get(plan_id) {
            Some(stored) if stored.connection_id != connection_id => {
                return Err(RegistryError::validation(
                    "import plan belongs to a different connection",
                ));
            }
            None => {
                return Err(RegistryError::validation(
                    "import plan is missing, expired, or already applied",
                ));
            }
            Some(_) => {}
        }
        Ok(plans
            .remove(plan_id)
            .expect("validated import plan should still exist")
            .plan)
    }

    async fn purge_expired(&self) {
        let now = Instant::now();
        self.plans
            .write()
            .await
            .retain(|_, plan| plan.expires_at > now);
    }
}

pub(crate) fn now_ms() -> Result<u64, RegistryError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|_| RegistryError::storage("system clock is before the Unix epoch"))
}

fn validate_adapter_address(
    adapter: AdapterId,
    address: &ResourceAddress,
) -> Result<(), RegistryError> {
    let matches = matches!(
        (adapter, address),
        (AdapterId::Etcd, ResourceAddress::Etcd { .. })
            | (AdapterId::Zookeeper, ResourceAddress::Zookeeper { .. })
            | (AdapterId::Nacos, ResourceAddress::NacosConfig { .. })
    );
    if matches {
        Ok(())
    } else {
        Err(RegistryError::validation(
            "import resource type does not match the open connection adapter",
        ))
    }
}

fn display_address(address: &ResourceAddress) -> String {
    match address {
        ResourceAddress::Root => "/".to_owned(),
        ResourceAddress::Etcd { key_base64 } => STANDARD
            .decode(key_base64)
            .ok()
            .and_then(|key| String::from_utf8(key).ok())
            .unwrap_or_else(|| format!("base64:{key_base64}")),
        ResourceAddress::EtcdPrefix { prefix_base64 } => format!("base64:{prefix_base64}"),
        ResourceAddress::Zookeeper { path } => path.clone(),
        ResourceAddress::NacosConfig { group, data_id } => format!("{group} / {data_id}"),
    }
}

fn next_plan_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!(
        "import-{}-{timestamp}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::registry::{EncodedValue, RegistryErrorCode, ResourceAddress, ResourceDocument};

    use super::{
        ImportAction, ImportPlan, ImportPreview, ImportPreviewItem, StoredImportPlan,
        TransferService, build_export_file, parse_import_file, suggested_export_file_name,
    };

    fn document() -> ResourceDocument {
        ResourceDocument {
            address: ResourceAddress::Zookeeper {
                path: "/services/payment".to_owned(),
            },
            name: "payment".to_owned(),
            value: EncodedValue::from_bytes(b"TOP_SECRET_VALUE"),
            content_type: Some("text".to_owned()),
            version: Some("7".to_owned()),
            metadata: BTreeMap::from([("modifiedZxid".to_owned(), "42".to_owned())]),
        }
    }

    #[test]
    fn metadata_only_export_omits_resource_values_by_default() {
        let bytes = build_export_file(&document(), false, 1_700_000_000_000).unwrap();
        let json = String::from_utf8(bytes).unwrap();

        assert!(json.contains("atlas-registry-export"));
        assert!(!json.contains("TOP_SECRET_VALUE"));
        assert!(!json.contains("\"value\""));
        assert!(json.contains("\"sha256\""));
    }

    #[test]
    fn value_export_round_trips_only_when_explicitly_enabled() {
        let bytes = build_export_file(&document(), true, 1_700_000_000_000).unwrap();
        let parsed = parse_import_file(&bytes).unwrap();

        assert!(parsed.include_values);
        assert_eq!(parsed.resources.len(), 1);
        assert_eq!(
            parsed.resources[0].value.as_ref().unwrap().content,
            "TOP_SECRET_VALUE"
        );
    }

    #[test]
    fn import_rejects_a_value_that_does_not_match_its_snapshot() {
        let bytes = build_export_file(&document(), true, 1_700_000_000_000).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["resources"][0]["value"]["content"] = "tampered".into();

        let error = parse_import_file(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert_eq!(error.code, RegistryErrorCode::Validation);
    }

    #[test]
    fn import_rejects_duplicate_resource_addresses() {
        let bytes = build_export_file(&document(), true, 1_700_000_000_000).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let duplicate = json["resources"][0].clone();
        json["resources"].as_array_mut().unwrap().push(duplicate);

        let error = parse_import_file(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert_eq!(error.code, RegistryErrorCode::Validation);
        assert!(error.message.contains("duplicate"));
    }

    #[test]
    fn import_preview_serialization_never_contains_resource_values() {
        let preview = ImportPreview {
            plan_id: "plan-redacted".to_owned(),
            file_name: "safe.json".to_owned(),
            resources: vec![ImportPreviewItem {
                address: document().address,
                name: "payment".to_owned(),
                action: ImportAction::Update,
                size_bytes: 16,
                sha256: "digest-only".to_owned(),
            }],
            creates: 0,
            updates: 1,
            skipped: 0,
            expires_in_seconds: 600,
        };
        let json = serde_json::to_string(&preview).unwrap();

        assert!(!json.contains("TOP_SECRET_VALUE"));
        assert!(!json.contains("content"));
        assert!(!json.contains("value"));
    }

    #[test]
    fn suggested_export_names_cannot_escape_the_chosen_directory() {
        let mut resource = document();
        resource.name = "../../payment config".to_owned();

        assert_eq!(
            suggested_export_file_name(&resource),
            "atlas-payment_config.json"
        );
    }

    #[tokio::test]
    async fn import_plans_are_connection_bound_and_one_time() {
        let service = TransferService::default();
        service.plans.write().await.insert(
            "plan-1".to_owned(),
            StoredImportPlan {
                connection_id: "connection-a".to_owned(),
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
                plan: ImportPlan { entries: vec![] },
            },
        );

        assert!(service.take_plan("plan-1", "connection-b").await.is_err());
        assert!(service.take_plan("plan-1", "connection-a").await.is_ok());
        assert!(service.take_plan("plan-1", "connection-a").await.is_err());
    }

    #[tokio::test]
    async fn expired_import_plans_cannot_be_applied() {
        let service = TransferService::default();
        service.plans.write().await.insert(
            "expired".to_owned(),
            StoredImportPlan {
                connection_id: "connection-a".to_owned(),
                expires_at: std::time::Instant::now() - std::time::Duration::from_secs(1),
                plan: ImportPlan { entries: vec![] },
            },
        );

        let error = service
            .take_plan("expired", "connection-a")
            .await
            .err()
            .expect("expired plan must be rejected");
        assert_eq!(error.code, RegistryErrorCode::Validation);
    }
}
