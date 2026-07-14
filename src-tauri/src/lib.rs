pub mod registry;

use registry::{
    AdapterDescriptor, AdapterId, ConnectionProbe, ConnectionProfile, ConnectionSession,
    RegistryCatalog, RegistryError, RegistryService, ResourceAddress, ResourceDocument,
    ResourcePage,
};
use serde::Deserialize;
use tauri::State;

#[tauri::command]
fn registry_capabilities() -> Vec<AdapterDescriptor> {
    RegistryCatalog::default().descriptors()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProbeConnectionRequest {
    adapter: AdapterId,
    endpoint: String,
}

#[tauri::command]
async fn probe_connection(request: ProbeConnectionRequest) -> Result<ConnectionProbe, String> {
    RegistryCatalog
        .probe(request.adapter, &request.endpoint)
        .await
}

#[tauri::command]
async fn open_connection(
    service: State<'_, RegistryService>,
    profile: ConnectionProfile,
) -> Result<ConnectionSession, RegistryError> {
    service.open(profile).await
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
    parent: ResourceAddress,
    cursor: Option<String>,
    limit: Option<usize>,
}

#[tauri::command]
async fn list_resources(
    service: State<'_, RegistryService>,
    request: ListResourcesRequest,
) -> Result<ResourcePage, RegistryError> {
    service
        .list(
            &request.connection_id,
            request.parent,
            request.cursor,
            request.limit.unwrap_or(100),
        )
        .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadResourceRequest {
    connection_id: String,
    address: ResourceAddress,
}

#[tauri::command]
async fn read_resource(
    service: State<'_, RegistryService>,
    request: ReadResourceRequest,
) -> Result<ResourceDocument, RegistryError> {
    service.read(&request.connection_id, request.address).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(RegistryService::default())
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            probe_connection,
            open_connection,
            close_connection,
            list_resources,
            read_resource
        ])
        .run(tauri::generate_context!())
        .expect("error while running Atlas Registry");
}
