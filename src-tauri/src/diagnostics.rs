use std::collections::BTreeMap;

use serde::Serialize;

use crate::registry::{
    AdapterDescriptor, AdapterId, ConnectionEnvironment, ConnectionProfile, RegistryError,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticBundle<'a> {
    schema_version: u8,
    application_version: &'static str,
    generated_at_ms: u64,
    runtime: RuntimeSummary,
    adapters: &'a [AdapterDescriptor],
    connections: ConnectionSummary,
    privacy: PrivacyDeclaration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSummary {
    os: &'static str,
    architecture: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionSummary {
    total: usize,
    by_adapter: BTreeMap<&'static str, usize>,
    by_environment: BTreeMap<&'static str, usize>,
    tls_enabled: usize,
    authenticated: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrivacyDeclaration {
    contains_resource_values: bool,
    contains_credentials: bool,
    contains_endpoints: bool,
    contains_connection_names: bool,
}

pub fn build(
    profiles: &[ConnectionProfile],
    adapters: &[AdapterDescriptor],
    generated_at_ms: u64,
) -> Result<Vec<u8>, RegistryError> {
    let mut by_adapter = BTreeMap::new();
    let mut by_environment = BTreeMap::new();
    for profile in profiles {
        *by_adapter
            .entry(adapter_label(profile.adapter))
            .or_insert(0) += 1;
        *by_environment
            .entry(environment_label(profile.environment))
            .or_insert(0) += 1;
    }
    let bundle = DiagnosticBundle {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION"),
        generated_at_ms,
        runtime: RuntimeSummary {
            os: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        },
        adapters,
        connections: ConnectionSummary {
            total: profiles.len(),
            by_adapter,
            by_environment,
            tls_enabled: profiles
                .iter()
                .filter(|profile| profile.tls.enabled)
                .count(),
            authenticated: profiles
                .iter()
                .filter(|profile| profile.auth.mode != Default::default())
                .count(),
        },
        privacy: PrivacyDeclaration {
            contains_resource_values: false,
            contains_credentials: false,
            contains_endpoints: false,
            contains_connection_names: false,
        },
    };
    serde_json::to_vec_pretty(&bundle).map_err(|error| {
        RegistryError::invalid_response(format!("cannot build diagnostic bundle: {error}"))
    })
}

fn adapter_label(adapter: AdapterId) -> &'static str {
    match adapter {
        AdapterId::Etcd => "etcd",
        AdapterId::Zookeeper => "zookeeper",
        AdapterId::Nacos => "nacos",
    }
}

fn environment_label(environment: ConnectionEnvironment) -> &'static str {
    match environment {
        ConnectionEnvironment::Unspecified => "unspecified",
        ConnectionEnvironment::Development => "development",
        ConnectionEnvironment::Testing => "testing",
        ConnectionEnvironment::Staging => "staging",
        ConnectionEnvironment::Production => "production",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{
        AuthenticationMode, ConnectionAuth, NacosApiVersion, RegistryCatalog, TlsProfile,
    };

    #[test]
    fn diagnostic_bundle_contains_counts_but_no_connection_identifiers_or_credentials() {
        let sentinel = "must-not-cross-the-diagnostic-boundary";
        let profiles = vec![ConnectionProfile {
            id: sentinel.to_owned(),
            name: sentinel.to_owned(),
            adapter: AdapterId::Nacos,
            endpoint: format!("https://{sentinel}.example"),
            namespace: sentinel.to_owned(),
            nacos_api_version: NacosApiVersion::V3,
            environment: ConnectionEnvironment::Production,
            auth: ConnectionAuth {
                mode: AuthenticationMode::UsernamePassword,
                username: sentinel.to_owned(),
                custom_key: sentinel.to_owned(),
            },
            tls: TlsProfile {
                enabled: true,
                ca_certificate_path: sentinel.to_owned(),
                client_certificate_path: sentinel.to_owned(),
                client_key_path: sentinel.to_owned(),
                server_name: sentinel.to_owned(),
            },
        }];

        let bytes = build(&profiles, &RegistryCatalog.descriptors(), 1)
            .expect("diagnostic summary should serialize");
        let json = String::from_utf8(bytes).expect("diagnostics are UTF-8 JSON");
        assert!(!json.contains(sentinel));
        assert!(json.contains("\"nacos\": 1"));
        assert!(json.contains("\"production\": 1"));
        assert!(json.contains("\"containsCredentials\": false"));
    }
}
