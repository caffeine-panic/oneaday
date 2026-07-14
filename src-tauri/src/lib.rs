pub mod registry;

use registry::{AdapterDescriptor, AdapterId, ConnectionProbe, RegistryCatalog};
use serde::Deserialize;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            registry_capabilities,
            probe_connection
        ])
        .run(tauri::generate_context!())
        .expect("error while running Atlas Registry");
}
