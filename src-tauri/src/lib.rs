pub mod audit;
mod audited_mutation;
pub mod connections;
pub mod credentials;
pub mod diagnostics;
pub mod registry;
pub mod transfer;
pub mod updates;

use registry::{
    AdapterDescriptor, AdapterId, ConnectionProbe, ConnectionProfile, ConnectionSession,
    EtcdLeaseAction, EtcdLeaseActionResult, EtcdTransaction, EtcdTransactionResult, MutationResult,
    NacosInstance, NacosNamespace, NacosNativeAction, NacosNativeActionResult,
    NacosNativeOperation, NacosService, NacosServicePage, NativeResourceInfo, OperationId,
    RegistryCatalog, RegistryError, RegistryService, ResourceAddress, ResourceDocument,
    ResourceHistoryDocument, ResourceHistoryPage, ResourceHistoryRequest, ResourceMutation,
    ResourcePage, ResourcePageRequest, ResourceSearchPage, ResourceSearchRequest, ResourceSnapshot,
    SubscriptionId, WatchEvent, WatchRequest, ZookeeperCreateMode, ZookeeperNativeAction,
    ZookeeperNativeActionResult,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, ipc::Channel};
use tauri_plugin_dialog::DialogExt;

use connections::ConnectionStore;
use credentials::{CredentialUpdate, CredentialVault, TransientCredential};

#[tauri::command]
fn registry_capabilities() -> Vec<AdapterDescriptor> {
    RegistryCatalog.descriptors()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportReceipt {
    file_name: String,
    connection_count: usize,
}

#[tauri::command]
async fn export_diagnostic_bundle<R: tauri::Runtime>(
    app: AppHandle<R>,
    credentials: State<'_, CredentialVault>,
) -> Result<Option<DiagnosticExportReceipt>, RegistryError> {
    let profiles =
        ConnectionStore::new(connection_profiles_path(&app)?, credentials.inner().clone())
            .load()
            .await?;
    let bytes = diagnostics::build(
        &profiles,
        &RegistryCatalog.descriptors(),
        transfer::now_ms()?,
    )?;
    let dialog_app = app.clone();
    let chosen_path = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Export privacy-safe Atlas Registry diagnostics")
            .set_file_name("atlas-registry-diagnostics.json")
            .add_filter("Atlas Registry diagnostics", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|error| RegistryError::storage(format!("diagnostic dialog failed: {error}")))?;
    let Some(chosen_path) = chosen_path else {
        return Ok(None);
    };
    let path = chosen_path.into_path().map_err(|error| {
        RegistryError::storage(format!(
            "diagnostic destination is not a local file: {error}"
        ))
    })?;
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot create diagnostics: {error}")))?;
    file.write_all(&bytes)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot write diagnostics: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| RegistryError::storage(format!("cannot sync diagnostics: {error}")))?;
    Ok(Some(DiagnosticExportReceipt {
        file_name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "atlas-registry-diagnostics.json".to_owned()),
        connection_count: profiles.len(),
    }))
}

#[tauri::command]
async fn probe_connection(
    service: State<'_, RegistryService>,
    credentials: State<'_, CredentialVault>,
    profile: ConnectionProfile,
    operation_id: String,
    transient_credential: Option<TransientCredential>,
) -> Result<ConnectionProbe, RegistryError> {
    let secret = credentials.resolve(&profile, transient_credential).await?;
    service
        .probe_with_credentials_cancellable(OperationId::new(operation_id)?, profile, secret)
        .await
}

#[tauri::command]
async fn load_connection_profiles<R: tauri::Runtime>(
    app: AppHandle<R>,
    credentials: State<'_, CredentialVault>,
) -> Result<Vec<ConnectionProfile>, RegistryError> {
    ConnectionStore::new(connection_profiles_path(&app)?, credentials.inner().clone())
        .load()
        .await
}

#[tauri::command]
async fn upsert_connection_profile<R: tauri::Runtime>(
    app: AppHandle<R>,
    credentials: State<'_, CredentialVault>,
    profile: ConnectionProfile,
    credential_update: CredentialUpdate,
) -> Result<Vec<ConnectionProfile>, RegistryError> {
    ConnectionStore::new(connection_profiles_path(&app)?, credentials.inner().clone())
        .upsert(profile, credential_update)
        .await
}

#[tauri::command]
async fn delete_connection_profile<R: tauri::Runtime>(
    app: AppHandle<R>,
    credentials: State<'_, CredentialVault>,
    connection_id: String,
) -> Result<Vec<ConnectionProfile>, RegistryError> {
    ConnectionStore::new(connection_profiles_path(&app)?, credentials.inner().clone())
        .delete(&connection_id)
        .await
}

fn connection_profiles_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<std::path::PathBuf, RegistryError> {
    app_config_directory(app).map(|directory| directory.join("connections.json"))
}

fn app_config_directory<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<std::path::PathBuf, RegistryError> {
    app.path().app_config_dir().map_err(|error| {
        RegistryError::storage(format!(
            "cannot resolve application config directory: {error}"
        ))
    })
}

#[tauri::command]
async fn open_connection(
    service: State<'_, RegistryService>,
    credentials: State<'_, CredentialVault>,
    profile: ConnectionProfile,
    operation_id: String,
    transient_credential: Option<TransientCredential>,
) -> Result<ConnectionSession, RegistryError> {
    let secret = credentials.resolve(&profile, transient_credential).await?;
    service
        .open_with_credentials_cancellable(OperationId::new(operation_id)?, profile, secret)
        .await
}

#[tauri::command]
async fn close_connection(
    service: State<'_, RegistryService>,
    connection_id: String,
) -> Result<(), RegistryError> {
    service.close(&connection_id).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResourcesRequest {
    connection_id: String,
    operation_id: String,
    page: ResourcePageRequest,
}

#[tauri::command]
async fn list_resources(
    service: State<'_, RegistryService>,
    request: ListResourcesRequest,
) -> Result<ResourcePage, RegistryError> {
    service
        .list_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.page,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadResourceRequest {
    connection_id: String,
    operation_id: String,
    address: ResourceAddress,
}

#[tauri::command]
async fn read_resource(
    service: State<'_, RegistryService>,
    request: ReadResourceRequest,
) -> Result<ResourceDocument, RegistryError> {
    service
        .read_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.address,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResourcesRequest {
    connection_id: String,
    operation_id: String,
    search: ResourceSearchRequest,
}

#[tauri::command]
async fn search_resources(
    service: State<'_, RegistryService>,
    request: SearchResourcesRequest,
) -> Result<ResourceSearchPage, RegistryError> {
    service
        .search_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.search,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResourceHistoryRequest {
    connection_id: String,
    operation_id: String,
    history: ResourceHistoryRequest,
}

#[tauri::command]
async fn list_resource_history(
    service: State<'_, RegistryService>,
    request: ListResourceHistoryRequest,
) -> Result<ResourceHistoryPage, RegistryError> {
    service
        .history_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.history,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadResourceHistoryRequest {
    connection_id: String,
    operation_id: String,
    address: ResourceAddress,
    revision_id: String,
}

#[tauri::command]
async fn read_resource_history(
    service: State<'_, RegistryService>,
    request: ReadResourceHistoryRequest,
) -> Result<ResourceHistoryDocument, RegistryError> {
    service
        .read_history_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.address,
            request.revision_id,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectNativeResourceRequest {
    connection_id: String,
    operation_id: String,
    address: ResourceAddress,
}

#[tauri::command]
async fn inspect_native_resource(
    service: State<'_, RegistryService>,
    request: InspectNativeResourceRequest,
) -> Result<NativeResourceInfo, RegistryError> {
    service
        .inspect_native_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.address,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutateResourceRequest {
    connection_id: String,
    operation_id: String,
    mutation: ResourceMutation,
}

#[tauri::command]
async fn mutate_resource<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    request: MutateResourceRequest,
) -> Result<MutationResult, RegistryError> {
    let directory = app_config_directory(&app)?;
    execute_audited_mutation(
        &directory,
        service.inner(),
        audit.inner(),
        &request.connection_id,
        &request.operation_id,
        request.mutation,
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteEtcdTransactionRequest {
    connection_id: String,
    operation_id: String,
    transaction: EtcdTransaction,
    confirmed: bool,
}

#[tauri::command]
async fn execute_etcd_transaction<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    request: ExecuteEtcdTransactionRequest,
) -> Result<EtcdTransactionResult, RegistryError> {
    if !request.confirmed {
        return Err(RegistryError::validation(
            "etcd transaction requires explicit confirmation",
        ));
    }
    let directory = app_config_directory(&app)?;
    execute_audited_etcd_transaction(
        &directory,
        service.inner(),
        audit.inner(),
        &request.connection_id,
        &request.operation_id,
        request.transaction,
    )
    .await
}

async fn execute_audited_etcd_transaction(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    transaction: EtcdTransaction,
) -> Result<EtcdTransactionResult, RegistryError> {
    transaction.validate()?;
    if service.connection_adapter(connection_id).await? != AdapterId::Etcd {
        return Err(RegistryError::unsupported(
            "etcd transactions require an etcd connection",
        ));
    }
    let workflow_service = service.clone();
    audited_mutation::run(
        directory,
        service,
        audit,
        connection_id,
        operation_id,
        |workflow_phase| async move {
            for mutation in &transaction.mutations {
                let previous = match mutation.expected_version() {
                    Some(expected_version) => {
                        let document = workflow_service
                            .read(connection_id, mutation.address().clone())
                            .await?;
                        if document.version.as_deref() != Some(expected_version.trim()) {
                            return Err(RegistryError::conflict(format!(
                                "resource version changed: expected {}, current {}",
                                expected_version.trim(),
                                document.version.as_deref().unwrap_or("unversioned")
                            )));
                        }
                        Some(document.snapshot()?)
                    }
                    None => None,
                };
                audit
                    .record_started_in(
                        directory,
                        connection_id,
                        operation_id,
                        mutation,
                        previous.as_ref(),
                    )
                    .await?;
            }
            let result = workflow_service
                .execute_etcd_transaction_with_phase(
                    connection_id,
                    transaction,
                    workflow_phase.clone(),
                )
                .await?;
            workflow_phase.mark_finalizing();
            for item in &result.results {
                audit
                    .record_applied_in(directory, connection_id, operation_id, item)
                    .await
                    .map_err(|error| {
                        RegistryError::audit_incomplete(format!(
                            "transaction succeeded, but an audit completion record failed: {}",
                            error.message
                        ))
                    })?;
            }
            Ok(result)
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteEtcdLeaseActionRequest {
    connection_id: String,
    operation_id: String,
    lease_action: EtcdLeaseAction,
    confirmed: bool,
}

#[tauri::command]
async fn execute_etcd_lease_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    request: ExecuteEtcdLeaseActionRequest,
) -> Result<EtcdLeaseActionResult, RegistryError> {
    if !request.confirmed {
        return Err(RegistryError::validation(
            "etcd lease mutation requires explicit confirmation",
        ));
    }
    let directory = app_config_directory(&app)?;
    execute_audited_etcd_lease_action(
        &directory,
        service.inner(),
        audit.inner(),
        &request.connection_id,
        &request.operation_id,
        request.lease_action,
    )
    .await
}

async fn execute_audited_etcd_lease_action(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    action: EtcdLeaseAction,
) -> Result<EtcdLeaseActionResult, RegistryError> {
    action.validate()?;
    if service.connection_adapter(connection_id).await? != AdapterId::Etcd {
        return Err(RegistryError::unsupported(
            "etcd lease actions require an etcd connection",
        ));
    }
    let operation = match &action {
        EtcdLeaseAction::GrantAndAttach { .. } => {
            audit::NativeAuditOperation::EtcdLeaseGrantAndAttach
        }
        EtcdLeaseAction::Attach { .. } => audit::NativeAuditOperation::EtcdLeaseAttach,
        EtcdLeaseAction::Detach { .. } => audit::NativeAuditOperation::EtcdLeaseDetach,
        EtcdLeaseAction::KeepAlive { .. } => audit::NativeAuditOperation::EtcdLeaseKeepAlive,
        EtcdLeaseAction::Revoke { .. } => audit::NativeAuditOperation::EtcdLeaseRevoke,
    };
    let target = match &action {
        EtcdLeaseAction::GrantAndAttach { ttl_seconds, .. } => format!("ttl:{ttl_seconds}s"),
        EtcdLeaseAction::Attach { lease_id, .. }
        | EtcdLeaseAction::KeepAlive { lease_id, .. }
        | EtcdLeaseAction::Revoke { lease_id, .. } => format!("lease:{lease_id}"),
        EtcdLeaseAction::Detach { .. } => "lease:detach".to_owned(),
    };
    let workflow_service = service.clone();
    let address = action.address().clone();
    let expected_version = action.expected_version().map(str::to_owned);
    audited_mutation::run(
        directory,
        service,
        audit,
        connection_id,
        operation_id,
        |workflow_phase| async move {
            let document = workflow_service
                .read(connection_id, address.clone())
                .await?;
            if let Some(expected_version) = expected_version.as_deref()
                && document.version.as_deref() != Some(expected_version.trim())
            {
                return Err(RegistryError::conflict(format!(
                    "resource version changed: expected {}, current {}",
                    expected_version.trim(),
                    document.version.as_deref().unwrap_or("unversioned")
                )));
            }
            let previous = document.snapshot()?;
            audit
                .record_native_started_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    Some(&address),
                    expected_version.as_deref(),
                    Some(&previous),
                    Some(&target),
                )
                .await?;
            let result = workflow_service
                .execute_etcd_lease_action_with_phase(connection_id, action, workflow_phase.clone())
                .await?;
            workflow_phase.mark_finalizing();
            let (applied_previous, applied_current, consistency) = match &result {
                EtcdLeaseActionResult::GrantAndAttach {
                    previous,
                    current,
                    consistency,
                    ..
                }
                | EtcdLeaseActionResult::Attach {
                    previous,
                    current,
                    consistency,
                    ..
                }
                | EtcdLeaseActionResult::Detach {
                    previous,
                    current,
                    consistency,
                    ..
                } => (Some(previous), Some(current), *consistency),
                EtcdLeaseActionResult::KeepAlive { .. } => (
                    Some(&previous),
                    Some(&previous),
                    registry::MutationConsistency::CheckedBeforeMutation,
                ),
                EtcdLeaseActionResult::Revoke {
                    previous,
                    consistency,
                    ..
                } => (Some(previous), None, *consistency),
            };
            audit
                .record_native_applied_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    Some(&address),
                    applied_previous,
                    applied_current,
                    consistency,
                    Some(&target),
                )
                .await
                .map_err(|error| {
                    RegistryError::audit_incomplete(format!(
                        "lease mutation succeeded, but the audit completion record failed: {}",
                        error.message
                    ))
                })?;
            Ok(result)
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteZookeeperNativeActionRequest {
    connection_id: String,
    operation_id: String,
    native_action: ZookeeperNativeAction,
    confirmed: bool,
}

#[tauri::command]
async fn execute_zookeeper_native_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    request: ExecuteZookeeperNativeActionRequest,
) -> Result<ZookeeperNativeActionResult, RegistryError> {
    if !request.confirmed {
        return Err(RegistryError::validation(
            "ZooKeeper native mutation requires explicit confirmation",
        ));
    }
    let directory = app_config_directory(&app)?;
    execute_audited_zookeeper_native_action(
        &directory,
        service.inner(),
        audit.inner(),
        &request.connection_id,
        &request.operation_id,
        request.native_action,
    )
    .await
}

async fn execute_audited_zookeeper_native_action(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    action: ZookeeperNativeAction,
) -> Result<ZookeeperNativeActionResult, RegistryError> {
    action.validate()?;
    if service.connection_adapter(connection_id).await? != AdapterId::Zookeeper {
        return Err(RegistryError::unsupported(
            "ZooKeeper native actions require a ZooKeeper connection",
        ));
    }
    let (operation, expected_version, target) = match &action {
        ZookeeperNativeAction::SetAcl {
            expected_acl_version,
            entries,
            ..
        } => (
            audit::NativeAuditOperation::ZookeeperAclSet,
            Some(expected_acl_version.to_string()),
            format!("aclEntries:{}", entries.len()),
        ),
        ZookeeperNativeAction::Create { mode, .. } => {
            let operation = match mode {
                ZookeeperCreateMode::PersistentSequential => {
                    audit::NativeAuditOperation::ZookeeperPersistentSequentialCreate
                }
                ZookeeperCreateMode::Ephemeral => {
                    audit::NativeAuditOperation::ZookeeperEphemeralCreate
                }
                ZookeeperCreateMode::EphemeralSequential => {
                    audit::NativeAuditOperation::ZookeeperEphemeralSequentialCreate
                }
            };
            (operation, None, format!("createMode:{mode:?}"))
        }
    };
    let address = action.address().clone();
    let workflow_service = service.clone();
    audited_mutation::run(
        directory,
        service,
        audit,
        connection_id,
        operation_id,
        |workflow_phase| async move {
            let previous = match &action {
                ZookeeperNativeAction::SetAcl { .. } => {
                    let NativeResourceInfo::ZookeeperAcl {
                        acl_version,
                        entries,
                        ..
                    } = workflow_service
                        .inspect_native(connection_id, address.clone())
                        .await?
                    else {
                        return Err(RegistryError::invalid_response(
                            "ZooKeeper ACL inspection returned the wrong native resource type",
                        ));
                    };
                    Some(native_state_snapshot(
                        &entries,
                        Some(acl_version.to_string()),
                    )?)
                }
                ZookeeperNativeAction::Create { .. } => None,
            };
            audit
                .record_native_started_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    Some(&address),
                    expected_version.as_deref(),
                    previous.as_ref(),
                    Some(&target),
                )
                .await?;
            let result = workflow_service
                .execute_zookeeper_native_action_with_phase(
                    connection_id,
                    action,
                    workflow_phase.clone(),
                )
                .await?;
            workflow_phase.mark_finalizing();
            let (applied_address, applied_previous, applied_current, consistency) = match &result {
                ZookeeperNativeActionResult::SetAcl {
                    previous_acl_version,
                    current_acl_version,
                    previous_entries,
                    current_entries,
                    consistency,
                    ..
                } => (
                    &address,
                    Some(native_state_snapshot(
                        previous_entries,
                        Some(previous_acl_version.to_string()),
                    )?),
                    Some(native_state_snapshot(
                        current_entries,
                        Some(current_acl_version.to_string()),
                    )?),
                    *consistency,
                ),
                ZookeeperNativeActionResult::Create {
                    address,
                    current,
                    consistency,
                    ..
                } => (address, None, Some(current.clone()), *consistency),
            };
            audit
                .record_native_applied_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    Some(applied_address),
                    applied_previous.as_ref(),
                    applied_current.as_ref(),
                    consistency,
                    Some(&target),
                )
                .await
                .map_err(|error| {
                    RegistryError::audit_incomplete(format!(
                        "ZooKeeper native mutation succeeded, but the audit completion record failed: {}",
                        error.message
                    ))
                })?;
            Ok(result)
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosOperationRequest {
    connection_id: String,
    operation_id: String,
}

#[tauri::command]
async fn list_nacos_namespaces(
    service: State<'_, RegistryService>,
    request: NacosOperationRequest,
) -> Result<Vec<NacosNamespace>, RegistryError> {
    service
        .list_nacos_namespaces_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListNacosServicesRequest {
    connection_id: String,
    operation_id: String,
    group: String,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[tauri::command]
async fn list_nacos_services(
    service: State<'_, RegistryService>,
    request: ListNacosServicesRequest,
) -> Result<NacosServicePage, RegistryError> {
    service
        .list_nacos_services_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.group,
            request.cursor,
            request.limit.unwrap_or(50),
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadNacosServiceRequest {
    connection_id: String,
    operation_id: String,
    group: String,
    service_name: String,
}

#[tauri::command]
async fn read_nacos_service(
    service: State<'_, RegistryService>,
    request: ReadNacosServiceRequest,
) -> Result<NacosService, RegistryError> {
    service
        .read_nacos_service_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.group,
            request.service_name,
        )
        .await
}

#[tauri::command]
async fn list_nacos_instances(
    service: State<'_, RegistryService>,
    request: ReadNacosServiceRequest,
) -> Result<Vec<NacosInstance>, RegistryError> {
    service
        .list_nacos_instances_cancellable(
            OperationId::new(request.operation_id)?,
            request.connection_id,
            request.group,
            request.service_name,
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteNacosNativeActionRequest {
    connection_id: String,
    operation_id: String,
    native_action: NacosNativeAction,
    confirmed: bool,
}

#[tauri::command]
async fn execute_nacos_native_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    request: ExecuteNacosNativeActionRequest,
) -> Result<NacosNativeActionResult, RegistryError> {
    if !request.confirmed {
        return Err(RegistryError::validation(
            "Nacos native mutation requires explicit confirmation",
        ));
    }
    let directory = app_config_directory(&app)?;
    execute_audited_nacos_native_action(
        &directory,
        service.inner(),
        audit.inner(),
        &request.connection_id,
        &request.operation_id,
        request.native_action,
    )
    .await
}

async fn execute_audited_nacos_native_action(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    mut action: NacosNativeAction,
) -> Result<NacosNativeActionResult, RegistryError> {
    action.validate()?;
    if service.connection_adapter(connection_id).await? != AdapterId::Nacos {
        return Err(RegistryError::unsupported(
            "Nacos native actions require a Nacos connection",
        ));
    }
    let operation = match action.operation() {
        NacosNativeOperation::CreateNamespace => audit::NativeAuditOperation::NacosCreateNamespace,
        NacosNativeOperation::UpdateNamespace => audit::NativeAuditOperation::NacosUpdateNamespace,
        NacosNativeOperation::DeleteNamespace => audit::NativeAuditOperation::NacosDeleteNamespace,
        NacosNativeOperation::CreateService => audit::NativeAuditOperation::NacosCreateService,
        NacosNativeOperation::UpdateService => audit::NativeAuditOperation::NacosUpdateService,
        NacosNativeOperation::DeleteService => audit::NativeAuditOperation::NacosDeleteService,
        NacosNativeOperation::RegisterInstance => {
            audit::NativeAuditOperation::NacosRegisterInstance
        }
        NacosNativeOperation::UpdateInstance => audit::NativeAuditOperation::NacosUpdateInstance,
        NacosNativeOperation::DeregisterInstance => {
            audit::NativeAuditOperation::NacosDeregisterInstance
        }
    };
    let target = match &action {
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
        } => {
            format!("service:{group}@@{service_name}")
        }
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
    };
    let expected_fingerprint = action.expected_fingerprint().map(str::to_owned);
    let workflow_service = service.clone();
    let audit_action = action.clone();
    audited_mutation::run(
        directory,
        service,
        audit,
        connection_id,
        operation_id,
        |workflow_phase| async move {
            let previous = nacos_native_snapshot(
                &workflow_service,
                connection_id,
                &audit_action,
                false,
            )
            .await?;
            audit
                .record_native_started_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    None,
                    expected_fingerprint.as_deref(),
                    previous.as_ref(),
                    Some(&target),
                )
                .await?;
            let result = workflow_service
                .execute_nacos_native_action_with_phase(
                    connection_id,
                    action,
                    workflow_phase.clone(),
                )
                .await?;
            workflow_phase.mark_finalizing();
            let current = nacos_native_snapshot(
                &workflow_service,
                connection_id,
                &audit_action,
                true,
            )
            .await
            .map_err(|error| {
                RegistryError::audit_incomplete(format!(
                    "Nacos native mutation succeeded, but its current state could not be summarized for audit: {}",
                    error.message
                ))
            })?;
            audit
                .record_native_applied_in(
                    directory,
                    connection_id,
                    operation_id,
                    operation,
                    None,
                    previous.as_ref(),
                    current.as_ref(),
                    result.consistency,
                    Some(&target),
                )
                .await
                .map_err(|error| {
                    RegistryError::audit_incomplete(format!(
                        "Nacos native mutation succeeded, but the audit completion record failed: {}",
                        error.message
                    ))
                })?;
            Ok(result)
        },
    )
    .await
}

fn native_state_snapshot<T: Serialize>(
    state: &T,
    version: Option<String>,
) -> Result<ResourceSnapshot, RegistryError> {
    let bytes = serde_json::to_vec(state).map_err(|error| {
        RegistryError::invalid_response(format!("cannot serialize native state for audit: {error}"))
    })?;
    Ok(ResourceSnapshot::from_bytes(&bytes, version))
}

async fn nacos_native_snapshot(
    service: &RegistryService,
    connection_id: &str,
    action: &NacosNativeAction,
    after: bool,
) -> Result<Option<ResourceSnapshot>, RegistryError> {
    match action {
        NacosNativeAction::CreateNamespace { .. } if !after => Ok(None),
        NacosNativeAction::DeleteNamespace { .. } if after => Ok(None),
        NacosNativeAction::CreateNamespace { namespace_id, .. }
        | NacosNativeAction::UpdateNamespace { namespace_id, .. }
        | NacosNativeAction::DeleteNamespace { namespace_id, .. } => {
            let state = service
                .list_nacos_namespaces(connection_id)
                .await?
                .into_iter()
                .find(|item| item.id == *namespace_id)
                .ok_or_else(|| {
                    RegistryError::not_found("Nacos namespace state is unavailable for audit")
                })?;
            let version = Some(state.fingerprint.clone());
            native_state_snapshot(&state, version).map(Some)
        }
        NacosNativeAction::CreateService { .. } if !after => Ok(None),
        NacosNativeAction::DeleteService { .. } if after => Ok(None),
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
        } => {
            let state = service
                .read_nacos_service(connection_id, group.clone(), service_name.clone())
                .await?;
            let version = Some(state.fingerprint.clone());
            native_state_snapshot(&state, version).map(Some)
        }
        NacosNativeAction::RegisterInstance { .. } if !after => Ok(None),
        NacosNativeAction::DeregisterInstance { .. } if after => Ok(None),
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
            let state = service
                .list_nacos_instances(connection_id, group.clone(), service_name.clone())
                .await?
                .into_iter()
                .find(|item| item.cluster == *cluster && item.ip == *ip && item.port == *port)
                .ok_or_else(|| {
                    RegistryError::not_found("Nacos instance state is unavailable for audit")
                })?;
            let version = Some(state.fingerprint.clone());
            native_state_snapshot(&state, version).map(Some)
        }
    }
}

async fn execute_audited_mutation(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    mutation: ResourceMutation,
) -> Result<MutationResult, RegistryError> {
    mutation.validate()?;
    let workflow_service = service.clone();
    audited_mutation::run(
        directory,
        service,
        audit,
        connection_id,
        operation_id,
        |workflow_phase| async move {
            let previous = match mutation.expected_version() {
                Some(expected_version) => {
                    let document = workflow_service
                        .read(connection_id, mutation.address().clone())
                        .await?;
                    if document.version.as_deref() != Some(expected_version.trim()) {
                        return Err(RegistryError::conflict(format!(
                            "resource version changed: expected {}, current {}",
                            expected_version.trim(),
                            document.version.as_deref().unwrap_or("unversioned")
                        )));
                    }
                    Some(document.snapshot()?)
                }
                None => None,
            };
            audit
                .record_started_in(
                    directory,
                    connection_id,
                    operation_id,
                    &mutation,
                    previous.as_ref(),
                )
                .await?;
            let result = workflow_service
                .mutate_with_phase(connection_id, mutation.clone(), workflow_phase.clone())
                .await?;
            workflow_phase.mark_finalizing();
            audit
                .record_applied_in(directory, connection_id, operation_id, &result)
                .await
                .map_err(|error| {
                    RegistryError::audit_incomplete(format!(
                        "mutation succeeded, but the audit completion record failed: {}",
                        error.message
                    ))
                })?;
            Ok(result)
        },
    )
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportResourceRequest {
    connection_id: String,
    address: ResourceAddress,
    include_value: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportReceipt {
    file_name: String,
    include_value: bool,
    snapshot: registry::ResourceSnapshot,
}

#[tauri::command]
async fn export_resource<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    request: ExportResourceRequest,
) -> Result<Option<ExportReceipt>, RegistryError> {
    let document = service
        .read(&request.connection_id, request.address)
        .await?;
    let snapshot = document.snapshot()?;
    let bytes = transfer::build_export_file(&document, request.include_value, transfer::now_ms()?)?;
    let suggested_name = transfer::suggested_export_file_name(&document);
    let dialog_app = app.clone();
    let chosen_path = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Export Atlas Registry resource")
            .set_file_name(&suggested_name)
            .add_filter("Atlas Registry export", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|error| RegistryError::storage(format!("export dialog failed: {error}")))?;
    let Some(chosen_path) = chosen_path else {
        return Ok(None);
    };
    let path = chosen_path.into_path().map_err(|error| {
        RegistryError::storage(format!("export destination is not a local file: {error}"))
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot create export file: {error}")))?;
    use tokio::io::AsyncWriteExt as _;
    file.write_all(&bytes)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot write export file: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| RegistryError::storage(format!("cannot sync export file: {error}")))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.json".to_owned());
    Ok(Some(ExportReceipt {
        file_name,
        include_value: request.include_value,
        snapshot,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChooseImportRequest {
    connection_id: String,
}

#[tauri::command]
async fn choose_import<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    transfers: State<'_, transfer::TransferService>,
    request: ChooseImportRequest,
) -> Result<Option<transfer::ImportPreview>, RegistryError> {
    let dialog_app = app.clone();
    let chosen_path = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Import Atlas Registry export")
            .add_filter("Atlas Registry export", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| RegistryError::storage(format!("import dialog failed: {error}")))?;
    let Some(chosen_path) = chosen_path else {
        return Ok(None);
    };
    let path = chosen_path.into_path().map_err(|error| {
        RegistryError::storage(format!("import source is not a local file: {error}"))
    })?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot open import file: {error}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| RegistryError::storage(format!("cannot inspect import file: {error}")))?;
    if metadata.len() > transfer::MAX_IMPORT_BYTES as u64 {
        return Err(RegistryError::validation(format!(
            "import file is larger than {} MiB",
            transfer::MAX_IMPORT_BYTES / (1024 * 1024)
        )));
    }
    use tokio::io::AsyncReadExt as _;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(transfer::MAX_IMPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| RegistryError::storage(format!("cannot read import file: {error}")))?;
    if bytes.len() > transfer::MAX_IMPORT_BYTES {
        return Err(RegistryError::validation(format!(
            "import file is larger than {} MiB",
            transfer::MAX_IMPORT_BYTES / (1024 * 1024)
        )));
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "import.json".to_owned());
    transfers
        .prepare_import(service.inner(), &request.connection_id, file_name, &bytes)
        .await
        .map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplyImportRequest {
    connection_id: String,
    plan_id: String,
    operation_id: String,
    confirmed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportAppliedItem {
    item: transfer::ImportPreviewItem,
    consistency: registry::MutationConsistency,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportFailure {
    item: transfer::ImportPreviewItem,
    error: RegistryError,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportApplyResult {
    applied: Vec<ImportAppliedItem>,
    failed: Option<ImportFailure>,
    remaining: usize,
}

#[tauri::command]
async fn apply_import<R: tauri::Runtime>(
    app: AppHandle<R>,
    service: State<'_, RegistryService>,
    audit: State<'_, audit::AuditLog>,
    transfers: State<'_, transfer::TransferService>,
    request: ApplyImportRequest,
) -> Result<ImportApplyResult, RegistryError> {
    if !request.confirmed {
        return Err(RegistryError::validation(
            "import requires explicit confirmation",
        ));
    }
    OperationId::new(request.operation_id.clone())?;
    let directory = app_config_directory(&app)?;
    let plan = transfers
        .take_plan(&request.plan_id, &request.connection_id)
        .await?;
    let total = plan.entries.len();
    let mut applied = Vec::with_capacity(total);
    for (index, entry) in plan.entries.into_iter().enumerate() {
        match execute_audited_mutation(
            &directory,
            service.inner(),
            audit.inner(),
            &request.connection_id,
            &request.operation_id,
            entry.mutation,
        )
        .await
        {
            Ok(result) => applied.push(ImportAppliedItem {
                item: entry.preview,
                consistency: result.consistency,
            }),
            Err(error) => {
                return Ok(ImportApplyResult {
                    applied,
                    failed: Some(ImportFailure {
                        item: entry.preview,
                        error,
                    }),
                    remaining: total - index - 1,
                });
            }
        }
    }
    Ok(ImportApplyResult {
        applied,
        failed: None,
        remaining: 0,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadAuditHistoryRequest {
    connection_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[tauri::command]
async fn load_audit_history<R: tauri::Runtime>(
    app: AppHandle<R>,
    audit: State<'_, audit::AuditLog>,
    request: LoadAuditHistoryRequest,
) -> Result<audit::AuditHistoryPage, RegistryError> {
    let directory = app_config_directory(&app)?;
    let connection_id = request
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|connection_id| !connection_id.is_empty());
    audit
        .load_recent_in(
            &directory,
            connection_id,
            request.cursor,
            request.limit.unwrap_or(50),
        )
        .await
}

#[tauri::command]
async fn cancel_operation(
    service: State<'_, RegistryService>,
    operation_id: String,
) -> Result<bool, RegistryError> {
    Ok(service.cancel(&OperationId::new(operation_id)?).await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartWatchCommandRequest {
    connection_id: String,
    subscription_id: String,
    watch: WatchRequest,
}

#[tauri::command]
async fn start_watch(
    service: State<'_, RegistryService>,
    request: StartWatchCommandRequest,
    on_event: Channel<WatchEvent>,
) -> Result<(), RegistryError> {
    let subscription_id = SubscriptionId::new(request.subscription_id)?;
    let mut events = service
        .start_watch(
            subscription_id.clone(),
            request.connection_id,
            request.watch,
        )
        .await?;
    let watch_service = service.inner().clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            if on_event.send(event).is_err() {
                watch_service.stop_watch(&subscription_id).await;
                break;
            }
        }
    });
    Ok(())
}

#[tauri::command]
async fn stop_watch(
    service: State<'_, RegistryService>,
    subscription_id: String,
) -> Result<bool, RegistryError> {
    Ok(service
        .stop_watch(&SubscriptionId::new(subscription_id)?)
        .await)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configured_builder(
        tauri::Builder::default().plugin(tauri_plugin_updater::Builder::new().build()),
    )
    .run(tauri::generate_context!())
    .expect("error while running Atlas Registry");
}

fn configured_builder<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder
        .plugin(tauri_plugin_dialog::init())
        .manage(RegistryService::default())
        .manage(audit::AuditLog::default())
        .manage(CredentialVault::system())
        .manage(transfer::TransferService::default())
        .manage(updates::PendingAppUpdate::default())
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            export_diagnostic_bundle,
            probe_connection,
            load_connection_profiles,
            upsert_connection_profile,
            delete_connection_profile,
            open_connection,
            close_connection,
            list_resources,
            read_resource,
            search_resources,
            list_resource_history,
            read_resource_history,
            inspect_native_resource,
            mutate_resource,
            execute_etcd_transaction,
            execute_etcd_lease_action,
            execute_zookeeper_native_action,
            list_nacos_namespaces,
            list_nacos_services,
            read_nacos_service,
            list_nacos_instances,
            execute_nacos_native_action,
            export_resource,
            choose_import,
            apply_import,
            load_audit_history,
            cancel_operation,
            start_watch,
            stop_watch,
            updates::check_for_app_update,
            updates::install_app_update
        ])
}

#[cfg(test)]
mod command_tests {
    use serde_json::{Value, json};
    use tauri::{WebviewWindowBuilder, test, webview::InvokeRequest};

    use super::configured_builder;

    #[test]
    fn public_commands_expose_capabilities_and_reject_an_invalid_mutation_contract() {
        let app = configured_builder(test::mock_builder())
            .build(test::mock_context(test::noop_assets()))
            .expect("mock app should build");
        let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock webview should build");

        let capabilities =
            test::get_ipc_response(&webview, request("registry_capabilities", Value::Null))
                .expect("capability command should succeed")
                .deserialize::<Value>()
                .expect("capabilities should be JSON");
        assert_eq!(capabilities.as_array().map(Vec::len), Some(3));
        let capabilities = capabilities.as_array().expect("adapter array");
        assert_eq!(
            capabilities[0]["capabilities"].as_array().map(Vec::len),
            Some(10)
        );
        assert!(
            capabilities[0]["capabilities"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == "transaction"))
        );
        assert_eq!(
            capabilities[1]["capabilities"].as_array().map(Vec::len),
            Some(10)
        );
        assert_eq!(
            capabilities[2]["capabilities"].as_array().map(Vec::len),
            Some(12)
        );

        let watch_error = test::get_ipc_response(
            &webview,
            request(
                "start_watch",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "subscriptionId": "invalid-root-watch",
                        "watch": {
                            "address": { "type": "root" },
                            "startVersion": null
                        }
                    },
                    "onEvent": "__CHANNEL__:42"
                }),
            ),
        )
        .expect_err("root watch should be rejected by the public command");
        assert_eq!(watch_error["code"], "validation");

        let search_error = test::get_ipc_response(
            &webview,
            request(
                "search_resources",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "blank-search",
                        "search": {
                            "scope": { "type": "root" },
                            "query": "   ",
                            "cursor": null,
                            "limit": 100
                        }
                    }
                }),
            ),
        )
        .expect_err("blank search should be rejected by the public command");
        assert_eq!(search_error["code"], "validation");

        let history_error = test::get_ipc_response(
            &webview,
            request(
                "list_resource_history",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "invalid-history",
                        "history": {
                            "address": { "type": "root" },
                            "cursor": null,
                            "limit": 50
                        }
                    }
                }),
            ),
        )
        .expect_err("non-Nacos server history should be rejected");
        assert_eq!(history_error["code"], "unsupported");

        let native_info_error = test::get_ipc_response(
            &webview,
            request(
                "inspect_native_resource",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "invalid-native-info",
                        "address": { "type": "root" }
                    }
                }),
            ),
        )
        .expect_err("root native inspection should be rejected before session lookup");
        assert_eq!(native_info_error["code"], "unsupported");

        let import_error = test::get_ipc_response(
            &webview,
            request(
                "apply_import",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "planId": "missing",
                        "operationId": "unconfirmed-import",
                        "confirmed": false
                    }
                }),
            ),
        )
        .expect_err("unconfirmed import should be rejected by the public command");
        assert_eq!(import_error["code"], "validation");

        let transaction_error = test::get_ipc_response(
            &webview,
            request(
                "execute_etcd_transaction",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "undersized-transaction",
                        "confirmed": true,
                        "transaction": {
                            "mutations": [{
                                "operation": "create",
                                "address": { "type": "etcd", "keyBase64": "L2F0bGFzL29ubHk=" },
                                "value": { "content": "only", "encoding": "utf8" }
                            }]
                        }
                    }
                }),
            ),
        )
        .expect_err("undersized transaction should be rejected by the public command");
        assert_eq!(transaction_error["code"], "validation");

        let lease_error = test::get_ipc_response(
            &webview,
            request(
                "execute_etcd_lease_action",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "unconfirmed-lease",
                        "confirmed": false,
                        "leaseAction": {
                            "action": "revoke",
                            "address": { "type": "etcd", "keyBase64": "L2F0bGFzL2tleQ==" },
                            "expectedVersion": "42",
                            "leaseId": "99"
                        }
                    }
                }),
            ),
        )
        .expect_err("unconfirmed lease mutation should be rejected by the public command");
        assert_eq!(lease_error["code"], "validation");

        let zookeeper_error = test::get_ipc_response(
            &webview,
            request(
                "execute_zookeeper_native_action",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "unconfirmed-zookeeper-native",
                        "confirmed": false,
                        "nativeAction": {
                            "action": "create",
                            "address": { "type": "zookeeper", "path": "/atlas/member-" },
                            "value": { "content": "online", "encoding": "utf8" },
                            "mode": "ephemeralSequential"
                        }
                    }
                }),
            ),
        )
        .expect_err("unconfirmed ZooKeeper native mutation should be rejected");
        assert_eq!(zookeeper_error["code"], "validation");

        let nacos_error = test::get_ipc_response(
            &webview,
            request(
                "execute_nacos_native_action",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "unconfirmed-nacos-native",
                        "confirmed": false,
                        "nativeAction": {
                            "action": "createService",
                            "group": "DEFAULT_GROUP",
                            "serviceName": "payments",
                            "protectThreshold": 0.0,
                            "ephemeral": false,
                            "metadata": {}
                        }
                    }
                }),
            ),
        )
        .expect_err("unconfirmed Nacos native mutation should be rejected");
        assert_eq!(nacos_error["code"], "validation");

        let error = test::get_ipc_response(
            &webview,
            request(
                "mutate_resource",
                json!({
                    "request": {
                        "connectionId": "missing",
                        "operationId": "invalid-root-create",
                        "mutation": {
                            "operation": "create",
                            "address": { "type": "root" },
                            "value": { "content": "must-not-be-audited", "encoding": "utf8" }
                        }
                    }
                }),
            ),
        )
        .expect_err("root mutation should be rejected");
        assert_eq!(error["code"], "validation");

        let connection_error = test::get_ipc_response(
            &webview,
            request(
                "upsert_connection_profile",
                json!({
                    "profile": {
                        "id": "",
                        "name": "Invalid",
                        "adapter": "etcd",
                        "endpoint": "127.0.0.1:2379",
                        "namespace": "",
                        "nacosApiVersion": "v2",
                        "environment": "unspecified",
                        "auth": { "mode": "none", "username": "", "customKey": "" },
                        "tls": {
                            "enabled": false,
                            "caCertificatePath": "",
                            "clientCertificatePath": "",
                            "clientKeyPath": "",
                            "serverName": ""
                        }
                    },
                    "credentialUpdate": { "operation": "preserve" }
                }),
            ),
        )
        .expect_err("invalid connection command should be rejected");
        assert_eq!(connection_error["code"], "validation");
    }

    fn request(command: &str, body: Value) -> InvokeRequest {
        InvokeRequest {
            cmd: command.to_owned(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("test URL should parse"),
            body: body.into(),
            headers: Default::default(),
            invoke_key: test::INVOKE_KEY.to_owned(),
        }
    }
}
