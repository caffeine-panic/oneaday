use atlas_registry_lib::{
    credentials::ConnectionSecret,
    registry::{
        AdapterId, AuthenticationMode, ConnectionAuth, ConnectionProfile, EtcdLeaseAction,
        EtcdLeaseActionResult, EtcdTransaction, MutationValue, NacosApiVersion, NacosNativeAction,
        NacosNativeOperation, NativeResourceInfo, OperationId, RegistryService, ResourceAddress,
        ResourceHistoryRequest, ResourceMutation, ResourceSearchRequest, SubscriptionId,
        TlsProfile, ValueEncoding, WatchEvent, WatchRequest, WatchStatusState, ZookeeperCreateMode,
        ZookeeperNativeAction, ZookeeperNativeActionResult,
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn profile(adapter: AdapterId, endpoint: String) -> ConnectionProfile {
    ConnectionProfile {
        id: format!("live-{adapter:?}").to_lowercase(),
        name: format!("Live {adapter:?}"),
        adapter,
        endpoint,
        namespace: String::new(),
        nacos_api_version: NacosApiVersion::V2,
        environment: Default::default(),
        auth: Default::default(),
        tls: Default::default(),
    }
}

fn secured_profile(
    adapter: AdapterId,
    endpoint: String,
) -> (ConnectionProfile, Option<ConnectionSecret>) {
    let mut profile = profile(adapter, endpoint);
    let prefix = match adapter {
        AdapterId::Etcd => "ATLAS_TEST_ETCD",
        AdapterId::Zookeeper => "ATLAS_TEST_ZOOKEEPER",
        AdapterId::Nacos => "ATLAS_TEST_NACOS",
    };
    let username = std::env::var(format!("{prefix}_USERNAME")).ok();
    let secret = username.map(|username| {
        profile.auth = ConnectionAuth {
            mode: if adapter == AdapterId::Zookeeper {
                AuthenticationMode::Digest
            } else {
                AuthenticationMode::UsernamePassword
            },
            username,
            custom_key: String::new(),
        };
        ConnectionSecret::new(
            std::env::var(format!("{prefix}_PASSWORD"))
                .unwrap_or_else(|_| panic!("set {prefix}_PASSWORD with {prefix}_USERNAME")),
        )
    });
    if std::env::var(format!("{prefix}_TLS")).as_deref() == Ok("1") {
        profile.tls = TlsProfile {
            enabled: true,
            ca_certificate_path: std::env::var(format!("{prefix}_TLS_CA")).unwrap_or_default(),
            client_certificate_path: std::env::var(format!("{prefix}_TLS_CERT"))
                .unwrap_or_default(),
            client_key_path: std::env::var(format!("{prefix}_TLS_KEY")).unwrap_or_default(),
            server_name: std::env::var(format!("{prefix}_TLS_SERVER_NAME")).unwrap_or_default(),
        };
    }
    (profile, secret)
}

fn mutations_enabled() -> bool {
    std::env::var("ATLAS_TEST_ENABLE_MUTATIONS").as_deref() == Ok("1")
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    )
}

fn text_value(content: &str) -> MutationValue {
    MutationValue {
        content: content.to_owned(),
        encoding: ValueEncoding::Utf8,
    }
}

#[test]
#[ignore = "requires ATLAS_TEST_ETCD_ENDPOINT"]
fn etcd_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ETCD_ENDPOINT")
        .expect("set ATLAS_TEST_ETCD_ENDPOINT before running ignored tests");
    let service = RegistryService::default();
    let (connection, secret) = secured_profile(AdapterId::Etcd, endpoint);

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("etcd session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("etcd root should be listable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same etcd session should be reusable");
        service
            .search(
                &session.id,
                ResourceSearchRequest {
                    scope: ResourceAddress::Root,
                    query: "atlas".to_owned(),
                    cursor: None,
                    limit: Some(25),
                },
            )
            .await
            .expect("etcd identifiers should be searchable without reading values");
        if let Ok(key) = std::env::var("ATLAS_TEST_ETCD_KEY") {
            let address = ResourceAddress::Etcd {
                key_base64: STANDARD.encode(key),
            };
            let document = service
                .read(&session.id, address.clone())
                .await
                .expect("configured etcd fixture should be readable");
            assert!(document.metadata.contains_key("modRevision"));
            if document
                .metadata
                .get("lease")
                .is_some_and(|lease| lease != "0")
            {
                let info = service
                    .inspect_native_cancellable(
                        OperationId::new(format!("live-lease-{}", unique_suffix())).unwrap(),
                        session.id.clone(),
                        address,
                    )
                    .await
                    .expect("configured etcd lease should be inspectable");
                assert!(matches!(info, NativeResourceInfo::EtcdLease { .. }));
            }
        }
        if mutations_enabled() {
            let prefix = std::env::var("ATLAS_TEST_ETCD_MUTATION_PREFIX")
                .unwrap_or_else(|_| "/atlas-registry-live-test".to_owned());
            let address = ResourceAddress::Etcd {
                key_base64: STANDARD.encode(format!("{prefix}/{}", unique_suffix())),
            };
            assert_create_update_delete(&service, &session.id, address).await;
            assert_etcd_transaction(&service, &session.id, &prefix).await;
            assert_etcd_lease_lifecycle(&service, &session.id, &prefix).await;
        }
    });
}

#[test]
#[ignore = "requires ATLAS_TEST_ZOOKEEPER_ENDPOINT"]
fn zookeeper_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ZOOKEEPER_ENDPOINT")
        .expect("set ATLAS_TEST_ZOOKEEPER_ENDPOINT before running ignored tests");
    let service = RegistryService::default();
    let (connection, secret) = secured_profile(AdapterId::Zookeeper, endpoint);

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("ZooKeeper session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("ZooKeeper root should be listable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same ZooKeeper session should be reusable");
        service
            .search(
                &session.id,
                ResourceSearchRequest {
                    scope: ResourceAddress::Root,
                    query: "atlas".to_owned(),
                    cursor: None,
                    limit: Some(25),
                },
            )
            .await
            .expect("ZooKeeper child identifiers should be searchable without reading data");
        if let Ok(path) = std::env::var("ATLAS_TEST_ZOOKEEPER_PATH") {
            let address = ResourceAddress::Zookeeper { path };
            let document = service
                .read(&session.id, address.clone())
                .await
                .expect("configured ZooKeeper fixture should be readable");
            assert!(document.metadata.contains_key("modifiedZxid"));
            let info = service
                .inspect_native_cancellable(
                    OperationId::new(format!("live-acl-{}", unique_suffix())).unwrap(),
                    session.id.clone(),
                    address,
                )
                .await
                .expect("configured ZooKeeper ACL should be inspectable");
            assert!(matches!(info, NativeResourceInfo::ZookeeperAcl { .. }));
        }
        if mutations_enabled() {
            let parent = std::env::var("ATLAS_TEST_ZOOKEEPER_MUTATION_PARENT")
                .unwrap_or_else(|_| "/".to_owned());
            let address = ResourceAddress::Zookeeper {
                path: format!(
                    "{}/atlas-registry-live-test-{}",
                    parent.trim_end_matches('/'),
                    unique_suffix()
                ),
            };
            assert_create_update_delete(&service, &session.id, address).await;
            assert_zookeeper_native_actions(&service, &session.id, &parent).await;
        }
    });
}

async fn assert_zookeeper_native_actions(
    service: &RegistryService,
    connection_id: &str,
    parent: &str,
) {
    let prefix = format!(
        "{}/atlas-native-{}",
        parent.trim_end_matches('/'),
        unique_suffix()
    );
    let acl_address = ResourceAddress::Zookeeper {
        path: format!("{prefix}-acl"),
    };
    service
        .mutate(
            connection_id,
            ResourceMutation::Create {
                address: acl_address.clone(),
                value: text_value("acl-native"),
                content_type: None,
            },
        )
        .await
        .expect("ACL fixture should be created");
    let (acl_version, entries) = match service
        .inspect_native_cancellable(
            OperationId::new(format!("native-acl-{}", unique_suffix())).unwrap(),
            connection_id.to_owned(),
            acl_address.clone(),
        )
        .await
        .expect("ACL fixture should be inspectable")
    {
        NativeResourceInfo::ZookeeperAcl {
            acl_version,
            entries,
            ..
        } => (acl_version, entries),
        other => panic!("unexpected ZooKeeper native info: {other:?}"),
    };
    let updated = service
        .execute_zookeeper_native_action(
            connection_id,
            ZookeeperNativeAction::SetAcl {
                address: acl_address.clone(),
                expected_acl_version: acl_version,
                entries: entries.clone(),
            },
        )
        .await
        .expect("matching aversion should atomically apply the ACL");
    let current_acl_version = match updated {
        ZookeeperNativeActionResult::SetAcl {
            previous_acl_version,
            current_acl_version,
            current_entries,
            ..
        } => {
            assert_eq!(previous_acl_version, acl_version);
            assert_eq!(current_entries, entries);
            assert_eq!(current_acl_version, acl_version + 1);
            current_acl_version
        }
        other => panic!("unexpected ACL action result: {other:?}"),
    };
    let stale = service
        .execute_zookeeper_native_action(
            connection_id,
            ZookeeperNativeAction::SetAcl {
                address: acl_address.clone(),
                expected_acl_version: acl_version,
                entries,
            },
        )
        .await
        .expect_err("stale aversion must be rejected");
    assert_eq!(
        stale.code,
        atlas_registry_lib::registry::RegistryErrorCode::Conflict
    );

    let created = service
        .execute_zookeeper_native_action(
            connection_id,
            ZookeeperNativeAction::Create {
                address: ResourceAddress::Zookeeper {
                    path: format!("{prefix}-member-"),
                },
                value: text_value("online"),
                mode: ZookeeperCreateMode::EphemeralSequential,
            },
        )
        .await
        .expect("ephemeral sequential node should inherit its parent ACL");
    let created_address = match created {
        ZookeeperNativeActionResult::Create {
            requested_address,
            address,
            sequence,
            mode,
            ..
        } => {
            assert_eq!(mode, ZookeeperCreateMode::EphemeralSequential);
            assert!(sequence.as_deref().is_some_and(|value| value.len() >= 10));
            assert_ne!(address, requested_address);
            address
        }
        other => panic!("unexpected native create result: {other:?}"),
    };
    let ephemeral = service
        .read(connection_id, created_address.clone())
        .await
        .expect("ephemeral sequential node should remain while the session is open");
    assert_ne!(
        ephemeral.metadata.get("ephemeralOwner").map(String::as_str),
        Some("0")
    );

    service
        .mutate(
            connection_id,
            ResourceMutation::Delete {
                address: created_address,
                expected_version: ephemeral.version.unwrap(),
            },
        )
        .await
        .expect("ephemeral fixture should be removable explicitly");
    let acl_document = service
        .read(connection_id, acl_address.clone())
        .await
        .expect("ACL fixture should remain readable after ACL update");
    assert_eq!(current_acl_version, acl_version + 1);
    service
        .mutate(
            connection_id,
            ResourceMutation::Delete {
                address: acl_address,
                expected_version: acl_document.version.unwrap(),
            },
        )
        .await
        .expect("ACL fixture should be cleaned up");
}

#[test]
#[ignore = "requires ATLAS_TEST_NACOS_ENDPOINT"]
fn nacos_live_session_can_browse_the_config_list() {
    let endpoint = std::env::var("ATLAS_TEST_NACOS_ENDPOINT")
        .expect("set ATLAS_TEST_NACOS_ENDPOINT before running ignored tests");
    let version = match std::env::var("ATLAS_TEST_NACOS_VERSION").as_deref() {
        Ok("3") | Ok("v3") => NacosApiVersion::V3,
        _ => NacosApiVersion::V2,
    };
    let service = RegistryService::default();
    let (mut connection, secret) = secured_profile(AdapterId::Nacos, endpoint);
    connection.nacos_api_version = version;
    connection.namespace = std::env::var("ATLAS_TEST_NACOS_NAMESPACE").unwrap_or_default();

    tauri::async_runtime::block_on(async {
        let session = service
            .open_with_credentials(connection, secret)
            .await
            .expect("Nacos session should open");
        let page = service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("Nacos config list should be browsable");
        service
            .list(&session.id, page.parent, None, 100)
            .await
            .expect("the same Nacos session should be reusable");
        service
            .search(
                &session.id,
                ResourceSearchRequest {
                    scope: ResourceAddress::Root,
                    query: "atlas".to_owned(),
                    cursor: None,
                    limit: Some(25),
                },
            )
            .await
            .expect("Nacos dataIds should be searchable without reading config content");
        if let (Ok(group), Ok(data_id)) = (
            std::env::var("ATLAS_TEST_NACOS_GROUP"),
            std::env::var("ATLAS_TEST_NACOS_DATA_ID"),
        ) {
            let address = ResourceAddress::NacosConfig { group, data_id };
            let document = service
                .read(&session.id, address.clone())
                .await
                .expect("configured Nacos fixture should be readable");
            assert!(document.metadata.contains_key("md5"));
            let history = service
                .history(
                    &session.id,
                    ResourceHistoryRequest {
                        address: address.clone(),
                        cursor: None,
                        limit: Some(25),
                    },
                )
                .await
                .expect("configured Nacos fixture history should be listable");
            if let Some(entry) = history.items.first() {
                service
                    .read_history_cancellable(
                        OperationId::new(format!("live-history-{}", unique_suffix())).unwrap(),
                        session.id.clone(),
                        address,
                        entry.revision_id.clone(),
                    )
                    .await
                    .expect("configured Nacos fixture history detail should be readable");
            }
        }
        if mutations_enabled() {
            let group = std::env::var("ATLAS_TEST_NACOS_MUTATION_GROUP")
                .unwrap_or_else(|_| "ATLAS_REGISTRY_TEST".to_owned());
            let address = ResourceAddress::NacosConfig {
                group: group.clone(),
                data_id: format!("atlas-registry-live-test-{}", unique_suffix()),
            };
            assert_create_update_delete(&service, &session.id, address).await;
            assert_nacos_native_actions(&service, &session.id, &group).await;
        }
    });
}

async fn assert_nacos_native_actions(service: &RegistryService, connection_id: &str, group: &str) {
    let suffix = unique_suffix();
    let namespace_id = format!("atlas-{suffix}");
    let created_namespace = service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::CreateNamespace {
                namespace_id: namespace_id.clone(),
                name: format!("Atlas {suffix}"),
                description: "Atlas compatibility fixture".to_owned(),
            },
        )
        .await
        .expect("Nacos namespace should be creatable through the selected API generation");
    assert_eq!(
        created_namespace.operation,
        NacosNativeOperation::CreateNamespace
    );
    let namespace = service
        .list_nacos_namespaces(connection_id)
        .await
        .expect("Nacos namespaces should be listable")
        .into_iter()
        .find(|item| item.id == namespace_id)
        .expect("created namespace should be visible");

    let service_name = format!("atlas-native-{suffix}");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::CreateService {
                group: group.to_owned(),
                service_name: service_name.clone(),
                protect_threshold: 0.0,
                ephemeral: false,
                metadata: std::collections::BTreeMap::from([(
                    "owner".to_owned(),
                    "atlas".to_owned(),
                )]),
            },
        )
        .await
        .expect("Nacos persistent service should be creatable");
    let created_service = service
        .read_nacos_service(connection_id, group.to_owned(), service_name.clone())
        .await
        .expect("created Nacos service should be readable");

    let port = 10_000 + (std::process::id() % 50_000) as u16;
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::RegisterInstance {
                group: group.to_owned(),
                service_name: service_name.clone(),
                cluster: "DEFAULT".to_owned(),
                ip: "127.0.0.1".to_owned(),
                port,
                weight: 1.0,
                enabled: true,
                ephemeral: false,
                metadata: std::collections::BTreeMap::new(),
            },
        )
        .await
        .expect("Nacos persistent instance should be registerable");
    let instance = service
        .list_nacos_instances(connection_id, group.to_owned(), service_name.clone())
        .await
        .expect("Nacos instances should be listable")
        .into_iter()
        .find(|item| item.ip == "127.0.0.1" && item.port == port)
        .expect("registered Nacos instance should be visible");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::UpdateInstance {
                group: group.to_owned(),
                service_name: service_name.clone(),
                cluster: instance.cluster.clone(),
                ip: instance.ip.clone(),
                port: instance.port,
                weight: 2.0,
                enabled: true,
                ephemeral: false,
                metadata: std::collections::BTreeMap::from([(
                    "zone".to_owned(),
                    "test".to_owned(),
                )]),
                expected_fingerprint: instance.fingerprint,
            },
        )
        .await
        .expect("Nacos instance should update after a matching fingerprint preflight");
    let updated_instance = service
        .list_nacos_instances(connection_id, group.to_owned(), service_name.clone())
        .await
        .expect("updated Nacos instance should be listable")
        .into_iter()
        .find(|item| item.ip == "127.0.0.1" && item.port == port)
        .expect("updated Nacos instance should remain visible");
    assert_eq!(updated_instance.weight, 2.0);
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::DeregisterInstance {
                group: group.to_owned(),
                service_name: service_name.clone(),
                cluster: updated_instance.cluster,
                ip: updated_instance.ip,
                port: updated_instance.port,
                ephemeral: updated_instance.ephemeral,
                expected_fingerprint: updated_instance.fingerprint,
            },
        )
        .await
        .expect("Nacos instance should deregister after a matching fingerprint preflight");

    let ephemeral_service_name = format!("{service_name}-ephemeral");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::CreateService {
                group: group.to_owned(),
                service_name: ephemeral_service_name.clone(),
                protect_threshold: 0.0,
                ephemeral: true,
                metadata: std::collections::BTreeMap::from([(
                    "lifecycle".to_owned(),
                    "sdk-heartbeat".to_owned(),
                )]),
            },
        )
        .await
        .expect("Nacos ephemeral service should be creatable");
    let ephemeral_port = port + 1;
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::RegisterInstance {
                group: group.to_owned(),
                service_name: ephemeral_service_name.clone(),
                cluster: "DEFAULT".to_owned(),
                ip: "127.0.0.1".to_owned(),
                port: ephemeral_port,
                weight: 1.0,
                enabled: true,
                ephemeral: true,
                metadata: std::collections::BTreeMap::from([(
                    "lifecycle".to_owned(),
                    "sdk-heartbeat".to_owned(),
                )]),
            },
        )
        .await
        .expect("Nacos ephemeral instance should be registered by the Naming SDK");
    let ephemeral_instance = service
        .list_nacos_instances(
            connection_id,
            group.to_owned(),
            ephemeral_service_name.clone(),
        )
        .await
        .expect("Nacos ephemeral instance should be listable")
        .into_iter()
        .find(|item| item.ip == "127.0.0.1" && item.port == ephemeral_port)
        .expect("SDK-managed Nacos instance should be visible");
    assert!(ephemeral_instance.ephemeral);
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::UpdateInstance {
                group: group.to_owned(),
                service_name: ephemeral_service_name.clone(),
                cluster: ephemeral_instance.cluster.clone(),
                ip: ephemeral_instance.ip.clone(),
                port: ephemeral_instance.port,
                weight: 1.5,
                enabled: true,
                ephemeral: true,
                metadata: std::collections::BTreeMap::from([(
                    "lifecycle".to_owned(),
                    "sdk-heartbeat-updated".to_owned(),
                )]),
                expected_fingerprint: ephemeral_instance.fingerprint,
            },
        )
        .await
        .expect("Nacos ephemeral instance should update through the Naming SDK");
    let updated_ephemeral = service
        .list_nacos_instances(
            connection_id,
            group.to_owned(),
            ephemeral_service_name.clone(),
        )
        .await
        .expect("updated Nacos ephemeral instance should be listable")
        .into_iter()
        .find(|item| item.ip == "127.0.0.1" && item.port == ephemeral_port)
        .expect("updated SDK-managed Nacos instance should remain visible");
    assert_eq!(updated_ephemeral.weight, 1.5);
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::DeregisterInstance {
                group: group.to_owned(),
                service_name: ephemeral_service_name.clone(),
                cluster: updated_ephemeral.cluster,
                ip: updated_ephemeral.ip,
                port: updated_ephemeral.port,
                ephemeral: true,
                expected_fingerprint: updated_ephemeral.fingerprint,
            },
        )
        .await
        .expect("Nacos ephemeral instance should deregister through the Naming SDK");
    let ephemeral_service = service
        .read_nacos_service(
            connection_id,
            group.to_owned(),
            ephemeral_service_name.clone(),
        )
        .await
        .expect("empty Nacos ephemeral service should remain before deletion");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::DeleteService {
                group: group.to_owned(),
                service_name: ephemeral_service_name,
                expected_fingerprint: ephemeral_service.fingerprint,
            },
        )
        .await
        .expect("empty Nacos ephemeral service should be deletable");
    let service_before_delete = service
        .read_nacos_service(connection_id, group.to_owned(), service_name.clone())
        .await
        .expect("empty Nacos service should remain before deletion");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::DeleteService {
                group: group.to_owned(),
                service_name,
                expected_fingerprint: service_before_delete.fingerprint,
            },
        )
        .await
        .expect("empty Nacos service should be deletable");
    service
        .execute_nacos_native_action(
            connection_id,
            NacosNativeAction::DeleteNamespace {
                namespace_id,
                expected_fingerprint: namespace.fingerprint,
            },
        )
        .await
        .expect("empty Nacos namespace should be deletable");
    assert_eq!(created_service.name, format!("atlas-native-{suffix}"));
}

async fn assert_etcd_lease_lifecycle(service: &RegistryService, connection_id: &str, prefix: &str) {
    let address = ResourceAddress::Etcd {
        key_base64: STANDARD.encode(format!("{prefix}/lease-{}", unique_suffix())),
    };
    let created = service
        .mutate(
            connection_id,
            ResourceMutation::Create {
                address: address.clone(),
                value: text_value("lease-lifecycle"),
                content_type: None,
            },
        )
        .await
        .expect("lease fixture should be created");
    let created_version = created.current.unwrap().version.unwrap();

    let attached = service
        .execute_etcd_lease_action(
            connection_id,
            EtcdLeaseAction::GrantAndAttach {
                address: address.clone(),
                expected_version: created_version,
                ttl_seconds: 120,
            },
        )
        .await
        .expect("a new lease should be granted and attached");
    let (lease_id, attached_version) = match attached {
        EtcdLeaseActionResult::GrantAndAttach {
            lease_id,
            current,
            remaining_ttl_seconds,
            ..
        } => {
            assert!(remaining_ttl_seconds > 0);
            (lease_id, current.version.unwrap())
        }
        other => panic!("unexpected grant result: {other:?}"),
    };
    service
        .execute_etcd_lease_action(
            connection_id,
            EtcdLeaseAction::KeepAlive {
                address: address.clone(),
                lease_id: lease_id.clone(),
            },
        )
        .await
        .expect("the selected lease should accept a one-shot keep alive");

    let detached = service
        .execute_etcd_lease_action(
            connection_id,
            EtcdLeaseAction::Detach {
                address: address.clone(),
                expected_version: attached_version,
            },
        )
        .await
        .expect("lease should detach with a matching revision");
    let detached_version = match detached {
        EtcdLeaseActionResult::Detach { current, .. } => current.version.unwrap(),
        other => panic!("unexpected detach result: {other:?}"),
    };

    let reattached = service
        .execute_etcd_lease_action(
            connection_id,
            EtcdLeaseAction::Attach {
                address: address.clone(),
                expected_version: detached_version,
                lease_id: lease_id.clone(),
            },
        )
        .await
        .expect("the existing lease should reattach");
    let reattached_version = match reattached {
        EtcdLeaseActionResult::Attach { current, .. } => current.version.unwrap(),
        other => panic!("unexpected attach result: {other:?}"),
    };

    service
        .execute_etcd_lease_action(
            connection_id,
            EtcdLeaseAction::Revoke {
                address: address.clone(),
                expected_version: reattached_version,
                lease_id,
            },
        )
        .await
        .expect("revoke should expire the selected key and lease");
    assert_eq!(
        service.read(connection_id, address).await.unwrap_err().code,
        atlas_registry_lib::registry::RegistryErrorCode::NotFound
    );
}

async fn assert_etcd_transaction(service: &RegistryService, connection_id: &str, prefix: &str) {
    let suffix = unique_suffix();
    let first = ResourceAddress::Etcd {
        key_base64: STANDARD.encode(format!("{prefix}/{suffix}-first")),
    };
    let second = ResourceAddress::Etcd {
        key_base64: STANDARD.encode(format!("{prefix}/{suffix}-second")),
    };
    let created = service
        .execute_etcd_transaction(
            connection_id,
            EtcdTransaction {
                mutations: vec![
                    ResourceMutation::Create {
                        address: first.clone(),
                        value: text_value("transaction-first"),
                        content_type: None,
                    },
                    ResourceMutation::Create {
                        address: second.clone(),
                        value: text_value("transaction-second"),
                        content_type: None,
                    },
                ],
            },
        )
        .await
        .expect("etcd transaction should create both keys atomically");
    assert_eq!(created.results.len(), 2);
    assert!(created.results.iter().all(|result| {
        result
            .current
            .as_ref()
            .and_then(|item| item.version.as_deref())
            == Some(created.revision.as_str())
    }));

    let first_version = created.results[0]
        .current
        .as_ref()
        .and_then(|snapshot| snapshot.version.clone())
        .expect("transaction create should expose the shared revision");
    let second_version = created.results[1]
        .current
        .as_ref()
        .and_then(|snapshot| snapshot.version.clone())
        .expect("transaction create should expose the shared revision");
    let conflict = service
        .execute_etcd_transaction(
            connection_id,
            EtcdTransaction {
                mutations: vec![
                    ResourceMutation::Update {
                        address: first.clone(),
                        value: text_value("must-not-apply"),
                        content_type: None,
                        expected_version: (first_version.parse::<i64>().unwrap() + 1).to_string(),
                    },
                    ResourceMutation::Delete {
                        address: second.clone(),
                        expected_version: second_version.clone(),
                    },
                ],
            },
        )
        .await
        .expect_err("one stale compare must reject the entire transaction");
    assert_eq!(
        conflict.code,
        atlas_registry_lib::registry::RegistryErrorCode::Conflict
    );
    assert_eq!(
        service
            .read(connection_id, first.clone())
            .await
            .unwrap()
            .value
            .content,
        "transaction-first"
    );
    service
        .read(connection_id, second.clone())
        .await
        .expect("failed transaction must not delete the second key");

    let applied = service
        .execute_etcd_transaction(
            connection_id,
            EtcdTransaction {
                mutations: vec![
                    ResourceMutation::Update {
                        address: first.clone(),
                        value: text_value("transaction-updated"),
                        content_type: None,
                        expected_version: first_version,
                    },
                    ResourceMutation::Delete {
                        address: second.clone(),
                        expected_version: second_version,
                    },
                ],
            },
        )
        .await
        .expect("matching compares should apply the entire transaction");
    assert_eq!(applied.results.len(), 2);
    assert_eq!(
        service
            .read(connection_id, first.clone())
            .await
            .unwrap()
            .value
            .content,
        "transaction-updated"
    );
    assert_eq!(
        service.read(connection_id, second).await.unwrap_err().code,
        atlas_registry_lib::registry::RegistryErrorCode::NotFound
    );

    let updated_version = applied.results[0]
        .current
        .as_ref()
        .and_then(|snapshot| snapshot.version.clone())
        .unwrap();
    service
        .mutate(
            connection_id,
            ResourceMutation::Delete {
                address: first,
                expected_version: updated_version,
            },
        )
        .await
        .expect("transaction test should clean up the remaining key");
}

async fn assert_create_update_delete(
    service: &RegistryService,
    connection_id: &str,
    address: ResourceAddress,
) {
    let created = service
        .mutate(
            connection_id,
            ResourceMutation::Create {
                address: address.clone(),
                value: text_value("atlas-live-create"),
                content_type: Some("text".to_owned()),
            },
        )
        .await
        .expect("test resource should be created");
    let created_version = created
        .current
        .and_then(|snapshot| snapshot.version)
        .expect("create should return a version");

    let subscription_id = SubscriptionId::new(format!("live-watch-{}", unique_suffix())).unwrap();
    let mut watch_events = service
        .start_watch(
            subscription_id.clone(),
            connection_id.to_owned(),
            WatchRequest {
                address: address.clone(),
                start_version: matches!(&address, ResourceAddress::Etcd { .. })
                    .then(|| created_version.clone()),
            },
        )
        .await
        .expect("live resource watch should start");
    wait_for_live_watch(&mut watch_events).await;
    drain_initial_watch_events(&mut watch_events).await;

    let stale_version = match &address {
        ResourceAddress::NacosConfig { .. } => "stale-md5".to_owned(),
        _ => (created_version
            .parse::<i64>()
            .expect("numeric protocol version")
            + 1)
        .to_string(),
    };
    let conflict = service
        .mutate(
            connection_id,
            ResourceMutation::Update {
                address: address.clone(),
                value: text_value("must-not-overwrite"),
                content_type: Some("text".to_owned()),
                expected_version: stale_version,
            },
        )
        .await
        .expect_err("stale update must be rejected");
    assert_eq!(
        conflict.code,
        atlas_registry_lib::registry::RegistryErrorCode::Conflict
    );

    let updated = service
        .mutate(
            connection_id,
            ResourceMutation::Update {
                address: address.clone(),
                value: text_value("atlas-live-update"),
                content_type: Some("text".to_owned()),
                expected_version: created_version,
            },
        )
        .await
        .expect("test resource should be conditionally updated");
    wait_for_change(&mut watch_events).await;
    let updated_version = updated
        .current
        .and_then(|snapshot| snapshot.version)
        .expect("update should return a version");
    let document = service
        .read(connection_id, address.clone())
        .await
        .expect("updated resource should be readable");
    assert_eq!(document.value.content, "atlas-live-update");

    service
        .mutate(
            connection_id,
            ResourceMutation::Delete {
                address: address.clone(),
                expected_version: updated_version,
            },
        )
        .await
        .expect("test resource should be conditionally deleted");
    assert!(service.stop_watch(&subscription_id).await);
    let missing = service
        .read(connection_id, address)
        .await
        .expect_err("deleted resource should not be readable");
    assert_eq!(
        missing.code,
        atlas_registry_lib::registry::RegistryErrorCode::NotFound
    );
}

async fn wait_for_live_watch(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events
                .recv()
                .await
                .expect("watch event channel should remain open")
            {
                WatchEvent::Status {
                    state: WatchStatusState::Live,
                    ..
                } => return,
                WatchEvent::Status {
                    state: WatchStatusState::Failed,
                    message,
                    ..
                } => panic!("live watch failed: {message:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("watch should become live within 10 seconds");
}

async fn drain_initial_watch_events(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    while matches!(
        tokio::time::timeout(std::time::Duration::from_millis(100), events.recv()).await,
        Ok(Some(_))
    ) {}
}

async fn wait_for_change(events: &mut tokio::sync::mpsc::Receiver<WatchEvent>) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match events
                .recv()
                .await
                .expect("watch event channel should remain open")
            {
                WatchEvent::Change { .. } => return,
                WatchEvent::Status {
                    state: WatchStatusState::Failed,
                    message,
                    ..
                } => panic!("live watch failed: {message:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("updated resource should emit a watch event within 10 seconds");
}
