use atlas_registry_lib::registry::{
    AdapterId, AdapterStatus, Capability, ConnectionProfile, EncodedValue, NacosApiVersion,
    OperationId, RegistryCatalog, RegistryErrorCode, RegistryService, ValueEncoding,
};

#[test]
fn catalog_reports_probe_browse_and_read_for_each_native_adapter() {
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
fn values_larger_than_the_inline_limit_are_kept_out_of_the_webview() {
    let oversized = vec![b'a'; EncodedValue::MAX_INLINE_BYTES + 1];
    let error = EncodedValue::try_from_inline_bytes(&oversized)
        .expect_err("oversized values must not cross the Tauri boundary");

    assert_eq!(error.code, RegistryErrorCode::ValueTooLarge);
    assert!(!error.retryable);
}

#[test]
fn connection_probe_rejects_a_blank_endpoint_before_using_a_protocol_client() {
    let error = tauri::async_runtime::block_on(RegistryService::default().probe_cancellable(
        OperationId::new("blank-probe".to_owned()).expect("operation id should be valid"),
        ConnectionProfile {
            id: "blank-endpoint".to_owned(),
            name: "Blank endpoint".to_owned(),
            adapter: AdapterId::Etcd,
            endpoint: "   ".to_owned(),
            namespace: String::new(),
            nacos_api_version: NacosApiVersion::V2,
        },
    ))
    .expect_err("a blank endpoint must be rejected");

    assert_eq!(error.code, RegistryErrorCode::Validation);
    assert_eq!(error.message, "endpoint cannot be blank");
}
