pub mod audit;
pub mod registry;

use registry::{
    AdapterDescriptor, ConnectionProbe, ConnectionProfile, ConnectionSession, MutationPhase,
    MutationResult, OperationId, RegistryCatalog, RegistryError, RegistryService, ResourceAddress,
    ResourceDocument, ResourceMutation, ResourcePage, ResourcePageRequest,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
fn registry_capabilities() -> Vec<AdapterDescriptor> {
    RegistryCatalog.descriptors()
}

#[tauri::command]
async fn probe_connection(
    service: State<'_, RegistryService>,
    profile: ConnectionProfile,
    operation_id: String,
) -> Result<ConnectionProbe, RegistryError> {
    service
        .probe_cancellable(OperationId::new(operation_id)?, profile)
        .await
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnectionProfiles {
    version: u32,
    profiles: Vec<ConnectionProfile>,
}

#[tauri::command]
async fn load_connection_profiles<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<ConnectionProfile>, RegistryError> {
    let path = connection_profiles_path(&app)?;
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(RegistryError::storage(format!(
                "cannot read connection profiles: {error}"
            )));
        }
    };
    let mut stored =
        serde_json::from_slice::<StoredConnectionProfiles>(&bytes).map_err(|error| {
            RegistryError::storage(format!("connection profile file is invalid: {error}"))
        })?;
    if stored.version != 1 {
        return Err(RegistryError::storage(format!(
            "unsupported connection profile version {}",
            stored.version
        )));
    }
    for profile in &mut stored.profiles {
        profile.validate()?;
    }
    Ok(stored.profiles)
}

#[tauri::command]
async fn save_connection_profiles<R: tauri::Runtime>(
    app: AppHandle<R>,
    mut profiles: Vec<ConnectionProfile>,
) -> Result<(), RegistryError> {
    for profile in &mut profiles {
        profile.validate()?;
    }
    let path = connection_profiles_path(&app)?;
    let directory = path
        .parent()
        .ok_or_else(|| RegistryError::storage("connection profile path has no parent"))?;
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| {
            RegistryError::storage(format!(
                "cannot create application config directory: {error}"
            ))
        })?;
    let bytes = serde_json::to_vec_pretty(&StoredConnectionProfiles {
        version: 1,
        profiles,
    })
    .map_err(|error| {
        RegistryError::storage(format!("cannot serialize connection profiles: {error}"))
    })?;
    tokio::fs::write(path, bytes).await.map_err(|error| {
        RegistryError::storage(format!("cannot save connection profiles: {error}"))
    })
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
    profile: ConnectionProfile,
    operation_id: String,
) -> Result<ConnectionSession, RegistryError> {
    service
        .open_cancellable(OperationId::new(operation_id)?, profile)
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
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            probe_connection,
            load_connection_profiles,
            save_connection_profiles,
            open_connection,
            close_connection,
            list_resources,
            read_resource,
            mutate_resource,
            cancel_operation
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
                .all(|adapter| adapter["capabilities"].as_array().map(Vec::len) == Some(6))
        );

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
