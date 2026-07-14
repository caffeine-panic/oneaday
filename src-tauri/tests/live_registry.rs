use atlas_registry_lib::registry::{
    AdapterId, ConnectionProfile, NacosApiVersion, RegistryService, ResourceAddress,
};

fn profile(adapter: AdapterId, endpoint: String) -> ConnectionProfile {
    ConnectionProfile {
        id: format!("live-{adapter:?}").to_lowercase(),
        name: format!("Live {adapter:?}"),
        adapter,
        endpoint,
        namespace: String::new(),
        nacos_api_version: NacosApiVersion::V2,
    }
}

#[test]
#[ignore = "requires ATLAS_TEST_ETCD_ENDPOINT"]
fn etcd_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ETCD_ENDPOINT")
        .expect("set ATLAS_TEST_ETCD_ENDPOINT before running ignored tests");
    let service = RegistryService::default();

    tauri::async_runtime::block_on(async {
        let session = service
            .open(profile(AdapterId::Etcd, endpoint))
            .await
            .expect("etcd session should open");
        service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("etcd root should be listable");
    });
}

#[test]
#[ignore = "requires ATLAS_TEST_ZOOKEEPER_ENDPOINT"]
fn zookeeper_live_session_can_browse_the_root() {
    let endpoint = std::env::var("ATLAS_TEST_ZOOKEEPER_ENDPOINT")
        .expect("set ATLAS_TEST_ZOOKEEPER_ENDPOINT before running ignored tests");
    let service = RegistryService::default();

    tauri::async_runtime::block_on(async {
        let session = service
            .open(profile(AdapterId::Zookeeper, endpoint))
            .await
            .expect("ZooKeeper session should open");
        service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("ZooKeeper root should be listable");
    });
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
    let mut connection = profile(AdapterId::Nacos, endpoint);
    connection.nacos_api_version = version;
    connection.namespace = std::env::var("ATLAS_TEST_NACOS_NAMESPACE").unwrap_or_default();

    tauri::async_runtime::block_on(async {
        let session = service
            .open(connection)
            .await
            .expect("Nacos session should open");
        service
            .list(&session.id, ResourceAddress::Root, None, 100)
            .await
            .expect("Nacos config list should be browsable");
    });
}
