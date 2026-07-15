use atlas_registry_lib::registry::{
    AdapterId, AdapterStatus, AuthenticationMode, Capability, ConnectionAuth, ConnectionProfile,
    EncodedValue, MutationValue, NacosApiVersion, OperationId, RegistryCatalog, RegistryErrorCode,
    RegistryService, ResourceAddress, ResourceMutation, ResourceSnapshot, TlsProfile,
    ValueEncoding,
};

#[test]
fn catalog_reports_read_watch_and_safe_mutation_capabilities_for_each_native_adapter() {
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
                == vec![
                    Capability::Probe,
                    Capability::Browse,
                    Capability::Read,
                    Capability::Watch,
                    Capability::Create,
                    Capability::Update,
                    Capability::Delete,
                ]
    }));
}

#[test]
fn mutation_contract_requires_versions_and_preserves_binary_values() {
    let address = ResourceAddress::Etcd {
        key_base64: "L3NlcnZpY2VzL3BheW1lbnQ=".to_owned(),
    };
    let update = ResourceMutation::Update {
        address: address.clone(),
        value: MutationValue {
            content: "/wAv".to_owned(),
            encoding: ValueEncoding::Base64,
        },
        content_type: None,
        expected_version: "42".to_owned(),
    };

    update.validate().expect("valid conditional update");
    assert_eq!(
        update.decoded_value().expect("binary value"),
        [0xff, 0x00, 0x2f]
    );

    let missing_version = ResourceMutation::Delete {
        address,
        expected_version: "   ".to_owned(),
    };
    let error = missing_version
        .validate()
        .expect_err("delete must be conditional");
    assert_eq!(error.code, RegistryErrorCode::Validation);

    let root_create = ResourceMutation::Create {
        address: ResourceAddress::Root,
        value: MutationValue {
            content: "value".to_owned(),
            encoding: ValueEncoding::Utf8,
        },
        content_type: None,
    };
    assert_eq!(
        root_create
            .validate()
            .expect_err("root cannot be written")
            .code,
        RegistryErrorCode::Validation
    );

    for path in ["relative/path", "/trailing/", "/double//slash", "/./dot"] {
        let invalid_zookeeper_create = ResourceMutation::Create {
            address: ResourceAddress::Zookeeper {
                path: path.to_owned(),
            },
            value: MutationValue {
                content: "value".to_owned(),
                encoding: ValueEncoding::Utf8,
            },
            content_type: None,
        };
        assert_eq!(
            invalid_zookeeper_create
                .validate()
                .expect_err("ZooKeeper mutation path must be canonical")
                .code,
            RegistryErrorCode::Validation
        );
    }
}

#[test]
fn mutation_snapshots_are_stable_and_do_not_contain_resource_values() {
    let snapshot = ResourceSnapshot::from_bytes(b"secret", Some("7".to_owned()));

    assert_eq!(
        snapshot.sha256,
        "2bb80d537b1da3e38bd30361aa855686bde0eacd7162fef6a25fe97bf527a25b"
    );
    assert_eq!(snapshot.size_bytes, 6);
    assert_eq!(snapshot.encoding, ValueEncoding::Utf8);
    assert_eq!(snapshot.version.as_deref(), Some("7"));
    assert!(
        !serde_json::to_string(&snapshot)
            .expect("snapshot should serialize")
            .contains("secret")
    );
}

#[test]
fn mutation_commands_require_an_open_registry_session() {
    let mutation = ResourceMutation::Delete {
        address: ResourceAddress::Zookeeper {
            path: "/atlas/missing".to_owned(),
        },
        expected_version: "0".to_owned(),
    };
    let error = tauri::async_runtime::block_on(
        RegistryService::default().mutate_cancellable(
            OperationId::new("missing-session-mutation".to_owned())
                .expect("operation id should be valid"),
            "missing-session".to_owned(),
            mutation,
        ),
    )
    .expect_err("mutation requires an open session");

    assert_eq!(error.code, RegistryErrorCode::NotConnected);
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
            environment: Default::default(),
            auth: Default::default(),
            tls: Default::default(),
        },
    ))
    .expect_err("a blank endpoint must be rejected");

    assert_eq!(error.code, RegistryErrorCode::Validation);
    assert_eq!(error.message, "endpoint cannot be blank");
}

#[test]
fn authenticated_probe_requires_a_secret_before_using_a_protocol_client() {
    let error = tauri::async_runtime::block_on(
        RegistryService::default().probe_with_credentials_cancellable(
            OperationId::new("missing-credential-probe".to_owned())
                .expect("operation id should be valid"),
            ConnectionProfile {
                id: "secured-etcd".to_owned(),
                name: "Secured etcd".to_owned(),
                adapter: AdapterId::Etcd,
                endpoint: "127.0.0.1:2379".to_owned(),
                namespace: String::new(),
                nacos_api_version: NacosApiVersion::V2,
                environment: Default::default(),
                auth: ConnectionAuth {
                    mode: AuthenticationMode::UsernamePassword,
                    username: "operator".to_owned(),
                    custom_key: String::new(),
                },
                tls: Default::default(),
            },
            None,
        ),
    )
    .expect_err("authenticated probes must not fall back to anonymous access");

    assert_eq!(error.code, RegistryErrorCode::CredentialMissing);
}

#[test]
fn legacy_connection_profiles_gain_safe_authentication_and_tls_defaults() {
    let profile = serde_json::from_value::<ConnectionProfile>(serde_json::json!({
        "id": "legacy-etcd",
        "name": "Legacy etcd",
        "adapter": "etcd",
        "endpoint": "127.0.0.1:2379",
        "namespace": "",
        "nacosApiVersion": "v2"
    }))
    .expect("version 1 profiles should migrate through serde defaults");

    assert_eq!(profile.auth, ConnectionAuth::default());
    assert_eq!(profile.auth.mode, AuthenticationMode::None);
    assert_eq!(profile.tls, TlsProfile::default());
    assert!(!profile.tls.enabled);
}
