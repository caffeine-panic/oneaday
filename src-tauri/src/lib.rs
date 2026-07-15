pub mod audit;
pub mod connections;
pub mod credentials;
pub mod registry;
pub mod transfer;

use registry::{
    AdapterDescriptor, ConnectionProbe, ConnectionProfile, ConnectionSession, MutationPhase,
    MutationResult, OperationId, RegistryCatalog, RegistryError, RegistryService, ResourceAddress,
    ResourceDocument, ResourceMutation, ResourcePage, ResourcePageRequest, ResourceSearchPage,
    ResourceSearchRequest, SubscriptionId, WatchEvent, WatchRequest,
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

async fn execute_audited_mutation(
    directory: &std::path::Path,
    service: &RegistryService,
    audit: &audit::AuditLog,
    connection_id: &str,
    operation_id: &str,
    mutation: ResourceMutation,
) -> Result<MutationResult, RegistryError> {
    mutation.validate()?;
    let registered_operation_id = OperationId::new(operation_id.to_owned())?;
    let phase = MutationPhase::default();
    let workflow_phase = phase.clone();
    let workflow_service = service.clone();
    let result = service
        .run_mutation_workflow(registered_operation_id, phase, async {
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
        })
        .await;
    match result {
        Ok(result) => Ok(result),
        Err(error) => {
            if error.code == registry::RegistryErrorCode::OutcomeUnknown {
                let _ = audit
                    .record_outcome_unknown_in(directory, connection_id, operation_id)
                    .await;
            } else if error.code != registry::RegistryErrorCode::AuditIncomplete {
                let _ = audit
                    .record_failed_in(directory, connection_id, operation_id, error.code)
                    .await;
            }
            Err(error)
        }
    }
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
    configured_builder(tauri::Builder::default())
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
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            probe_connection,
            load_connection_profiles,
            upsert_connection_profile,
            delete_connection_profile,
            open_connection,
            close_connection,
            list_resources,
            read_resource,
            search_resources,
            mutate_resource,
            export_resource,
            choose_import,
            apply_import,
            cancel_operation,
            start_watch,
            stop_watch
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
        assert!(
            capabilities
                .as_array()
                .expect("adapter array")
                .iter()
                .all(|adapter| adapter["capabilities"].as_array().map(Vec::len) == Some(8))
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
