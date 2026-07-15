fn main() {
    const COMMANDS: &[&str] = &[
        "registry_capabilities",
        "probe_connection",
        "load_connection_profiles",
        "upsert_connection_profile",
        "delete_connection_profile",
        "open_connection",
        "close_connection",
        "list_resources",
        "read_resource",
        "search_resources",
        "list_resource_history",
        "read_resource_history",
        "inspect_native_resource",
        "mutate_resource",
        "export_resource",
        "choose_import",
        "apply_import",
        "load_audit_history",
        "cancel_operation",
        "start_watch",
        "stop_watch",
    ];

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to build Atlas Registry Tauri manifest");
}
