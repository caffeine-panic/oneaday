use atlas_registry_lib::registry::{
    AdapterId, AdapterStatus, Capability, EncodedValue, RegistryCatalog, ValueEncoding,
};

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
            && descriptor.capabilities
                == vec![Capability::Probe, Capability::Browse, Capability::Read]
    }));
}

#[test]
fn binary_resource_values_cross_the_tauri_boundary_without_data_loss() {
    let value = EncodedValue::from_bytes(&[0xff, 0x00, 0x2f]);

    assert_eq!(value.encoding, ValueEncoding::Base64);
    assert_eq!(value.content, "/wAv");
    assert_eq!(value.size_bytes, 3);
}

#[test]
fn utf8_resource_values_remain_editable_text() {
    let value = EncodedValue::from_bytes("支付服务".as_bytes());

    assert_eq!(value.encoding, ValueEncoding::Utf8);
    assert_eq!(value.content, "支付服务");
    assert_eq!(value.size_bytes, 12);
}

#[test]
fn connection_probe_rejects_a_blank_endpoint_before_using_a_protocol_client() {
    let error = tauri::async_runtime::block_on(RegistryCatalog.probe(AdapterId::Etcd, "   "))
        .expect_err("a blank endpoint must be rejected");

    assert_eq!(error, "endpoint cannot be blank");
}
