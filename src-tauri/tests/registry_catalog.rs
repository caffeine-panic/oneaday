use atlas_registry_lib::registry::{AdapterDescriptor, AdapterId, RegistryCatalog};

#[test]
fn catalog_reports_the_three_native_adapters_and_their_distinct_capabilities() {
    let descriptors = RegistryCatalog.descriptors();

    assert_eq!(
        descriptors,
        vec![
            AdapterDescriptor {
                id: AdapterId::Etcd,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "watch",
                    "lease",
                    "transaction"
                ],
            },
            AdapterDescriptor {
                id: AdapterId::Zookeeper,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "watch",
                    "acl",
                    "ephemeral"
                ],
            },
            AdapterDescriptor {
                id: AdapterId::Nacos,
                status: "available",
                capabilities: vec![
                    "browse",
                    "read",
                    "write",
                    "delete",
                    "listen",
                    "namespace",
                    "service"
                ],
            },
        ]
    );
}

#[test]
fn connection_probe_rejects_a_blank_endpoint_before_using_a_protocol_client() {
    let error = tauri::async_runtime::block_on(RegistryCatalog.probe(AdapterId::Etcd, "   "))
        .expect_err("a blank endpoint must be rejected");

    assert_eq!(error, "endpoint cannot be blank");
}
