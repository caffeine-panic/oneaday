use serde::{Deserialize, Serialize};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcResponse {
    result: Capabilities,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    protocol_version: String,
    adapters: Vec<String>,
}

#[tauri::command]
async fn sidecar_capabilities(app: tauri::AppHandle) -> Result<Capabilities, String> {
    let sidecar = app
        .shell()
        .sidecar("registry-core")
        .map_err(|error| error.to_string())?;
    let (mut events, child) = sidecar.spawn().map_err(|error| error.to_string())?;

    child
        .write(b"{\"jsonrpc\":\"2.0\",\"id\":\"desktop-startup\",\"method\":\"system.capabilities\"}\n")
        .map_err(|error| error.to_string())?;

    while let Some(event) = events.recv().await {
        match event {
            CommandEvent::Stdout(bytes) => {
                let response: RpcResponse = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("invalid sidecar response: {error}"))?;
                return Ok(response.result);
            }
            CommandEvent::Error(message) => return Err(message),
            CommandEvent::Terminated(payload) => {
                return Err(format!("sidecar terminated before responding: {payload:?}"));
            }
            _ => {}
        }
    }

    Err("sidecar closed without a response".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![sidecar_capabilities])
        .run(tauri::generate_context!())
        .expect("error while running Atlas Registry");
}
