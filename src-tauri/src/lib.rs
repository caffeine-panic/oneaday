pub mod audit;
pub mod connections;
pub mod credentials;
pub mod registry;

use registry::{
    AdapterDescriptor, ConnectionProbe, ConnectionProfile, ConnectionSession, MutationPhase,
    MutationResult, OperationId, RegistryCatalog, RegistryError, RegistryService, ResourceAddress,
    ResourceDocument, ResourceMutation, ResourcePage, ResourcePageRequest, SubscriptionId,
    WatchEvent, WatchRequest,
};
use serde::Deserialize;
use tauri::{AppHandle, Manager, State, ipc::Channel};

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
    request.mutation.validate()?;
    let operation_id = OperationId::new(request.operation_id.clone())?;
    let directory = app_config_directory(&app)?;
    let phase = MutationPhase::default();
    let workflow_phase = phase.clone();
    let workflow_service = service.inner().clone();
    let result = service
        .run_mutation_workflow(operation_id, phase, async {
            let previous = match request.mutation.expected_version() {
                Some(expected_version) => {
                    let document = workflow_service
                        .read(&request.connection_id, request.mutation.address().clone())
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
                    &directory,
                    &request.connection_id,
                    &request.operation_id,
                    &request.mutation,
                    previous.as_ref(),
                )
                .await?;
            let result = workflow_service
                .mutate_with_phase(
                    &request.connection_id,
                    request.mutation.clone(),
                    workflow_phase.clone(),
                )
                .await?;
            workflow_phase.mark_finalizing();
            audit
                .record_applied_in(
                    &directory,
                    &request.connection_id,
                    &request.operation_id,
                    &result,
                )
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
                    .record_outcome_unknown_in(
                        &directory,
                        &request.connection_id,
                        &request.operation_id,
                    )
                    .await;
            } else if error.code != registry::RegistryErrorCode::AuditIncomplete {
                let _ = audit
                    .record_failed_in(
                        &directory,
                        &request.connection_id,
                        &request.operation_id,
                        error.code,
                    )
                    .await;
            }
            Err(error)
        }
    }
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
        .manage(RegistryService::default())
        .manage(audit::AuditLog::default())
        .manage(CredentialVault::system())
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
            mutate_resource,
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
                .all(|adapter| adapter["capabilities"].as_array().map(Vec::len) == Some(7))
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
