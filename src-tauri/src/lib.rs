pub mod registry;

use registry::{
    AdapterDescriptor, AdapterId, ConnectionProbe, ConnectionProfile, ConnectionSession,
    OperationId, RegistryCatalog, RegistryError, RegistryService, ResourceAddress,
    ResourceDocument, ResourcePage, ResourcePageRequest,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
fn registry_capabilities() -> Vec<AdapterDescriptor> {
    RegistryCatalog.descriptors()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeConnectionRequest {
    adapter: AdapterId,
    endpoint: String,
}

#[tauri::command]
async fn probe_connection(
    request: ProbeConnectionRequest,
) -> Result<ConnectionProbe, RegistryError> {
    RegistryCatalog
        .probe(request.adapter, &request.endpoint)
        .await
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnectionProfiles {
    version: u32,
    profiles: Vec<ConnectionProfile>,
}

#[tauri::command]
async fn load_connection_profiles(app: AppHandle) -> Result<Vec<ConnectionProfile>, RegistryError> {
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
async fn save_connection_profiles(
    app: AppHandle,
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

fn connection_profiles_path(app: &AppHandle) -> Result<std::path::PathBuf, RegistryError> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("connections.json"))
        .map_err(|error| {
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

#[tauri::command]
async fn cancel_operation(
    service: State<'_, RegistryService>,
    operation_id: String,
) -> Result<bool, RegistryError> {
    Ok(service.cancel(&OperationId::new(operation_id)?).await)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RegistryService::default())
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            probe_connection,
            load_connection_profiles,
            save_connection_profiles,
            open_connection,
            close_connection,
            list_resources,
            read_resource,
            cancel_operation
        ])
        .run(tauri::generate_context!())
        .expect("error while running Atlas Registry");
}
