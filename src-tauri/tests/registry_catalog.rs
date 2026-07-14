use atlas_registry_lib::registry::{AdapterId, AdapterStatus, Capability, RegistryCatalog};

#[test]
fn catalog_reports_only_the_native_capability_that_is_implemented() {
    let descriptors = RegistryCatalog.descriptors();

    assert_eq!(
        descriptors
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<Vec<_>>(),
        vec![AdapterId::Etcd, AdapterId::Zookeeper, AdapterId::Nacos]
    );
    assert!(descriptors.iter().all(|descriptor| {
        descriptor.status == AdapterStatus::Available
            && descriptor.capabilities == vec![Capability::Probe]
    }));
}

#[test]
fn connection_probe_rejects_a_blank_endpoint_before_using_a_protocol_client() {
    let error = tauri::async_runtime::block_on(RegistryCatalog.probe(AdapterId::Etcd, "   "))
        .expect_err("a blank endpoint must be rejected");

    assert_eq!(error, "endpoint cannot be blank");
}
