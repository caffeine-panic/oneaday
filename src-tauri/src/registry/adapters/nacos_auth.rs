use std::{
    collections::HashMap,
    sync::{Arc, RwLock, Weak},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use nacos_sdk::api::{
    config::ConfigServiceBuilder,
    plugin::{AuthContext, AuthPlugin, LoginIdentityContext, RequestResource},
};
use ring::hmac;
use serde::Deserialize;

use crate::{
    credentials::ConnectionSecret,
    registry::{AuthenticationMode, ConnectionProfile, NacosApiVersion, RegistryError},
};

#[derive(Clone)]
pub(super) enum NacosRequestAuth {
    None,
    Token {
        token: Arc<RwLock<Option<ConnectionSecret>>>,
        transport: TokenTransport,
    },
    Static {
        key: String,
        secret: Arc<ConnectionSecret>,
    },
    MseAccessKey {
        access_key_id: String,
        access_key_secret: Arc<ConnectionSecret>,
    },
}

impl NacosRequestAuth {
    pub(super) fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.apply_with_resource_at(request, "", current_timestamp_millis())
    }

    pub(super) fn apply_for_config(
        &self,
        request: reqwest::RequestBuilder,
        namespace: &str,
        group: &str,
    ) -> reqwest::RequestBuilder {
        self.apply_with_resource_at(
            request,
            &mse_config_resource(namespace, group),
            current_timestamp_millis(),
        )
    }

    fn apply_with_resource_at(
        &self,
        request: reqwest::RequestBuilder,
        resource: &str,
        timestamp_millis: u128,
    ) -> reqwest::RequestBuilder {
        match self {
            Self::None => request,
            Self::Token { token, transport } => {
                let token = token.read().unwrap_or_else(|error| error.into_inner());
                match token.as_ref() {
                    Some(token) if *transport == TokenTransport::Bearer => {
                        request.bearer_auth(token.expose())
                    }
                    Some(token) => request.query(&[("accessToken", token.expose())]),
                    None => request,
                }
            }
            Self::Static { key, secret } => request.query(&[(key.as_str(), secret.expose())]),
            Self::MseAccessKey {
                access_key_id,
                access_key_secret,
            } => {
                let timestamp = timestamp_millis.to_string();
                let signature = mse_signature(access_key_secret, resource, &timestamp);
                request
                    .header("Spas-AccessKey", access_key_id)
                    .header("Timestamp", timestamp)
                    .header("Spas-Signature", signature)
            }
        }
    }
}

fn mse_config_resource(namespace: &str, group: &str) -> String {
    match (namespace.is_empty(), group.is_empty()) {
        (false, false) => format!("{namespace}+{group}"),
        (false, true) => namespace.to_owned(),
        (true, false) => group.to_owned(),
        (true, true) => String::new(),
    }
}

fn current_timestamp_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TokenTransport {
    Query,
    Bearer,
}

pub(super) struct NacosAuthConfiguration {
    pub(super) config_builder: ConfigServiceBuilder,
    pub(super) request_auth: NacosRequestAuth,
    pub(super) sdk_auth: Option<Arc<dyn AuthPlugin>>,
}

pub(super) fn configure(
    mut builder: ConfigServiceBuilder,
    http: reqwest::Client,
    profile: &ConnectionProfile,
    secret: Option<&Arc<ConnectionSecret>>,
) -> Result<NacosAuthConfiguration, RegistryError> {
    let (request_auth, sdk_auth) = match profile.auth.mode {
        AuthenticationMode::None => (NacosRequestAuth::None, None),
        AuthenticationMode::UsernamePassword => {
            let secret = required_secret(profile, secret)?;
            let token = Arc::new(RwLock::new(None));
            let (login_path, form_encoded, transport) = login_settings(profile.nacos_api_version);
            let plugin: Arc<dyn AuthPlugin> = Arc::new(NacosHttpAuthPlugin {
                http,
                login_base: nacos_server_base(first_endpoint(&profile.endpoint)),
                login_path,
                form_encoded,
                username: profile.auth.username.clone(),
                password: Arc::downgrade(secret),
                token: Arc::downgrade(&token),
            });
            builder = builder.with_auth_plugin(plugin.clone());
            (NacosRequestAuth::Token { token, transport }, Some(plugin))
        }
        AuthenticationMode::Custom => {
            let secret = required_secret(profile, secret)?.clone();
            let plugin: Arc<dyn AuthPlugin> = Arc::new(NacosStaticAuthPlugin {
                key: profile.auth.custom_key.clone(),
                secret: Arc::downgrade(&secret),
            });
            builder = builder.with_auth_plugin(plugin.clone());
            (
                NacosRequestAuth::Static {
                    key: profile.auth.custom_key.clone(),
                    secret,
                },
                Some(plugin),
            )
        }
        AuthenticationMode::MseAccessKey => {
            let access_key_secret = required_secret(profile, secret)?.clone();
            let plugin: Arc<dyn AuthPlugin> = Arc::new(NacosMseAuthPlugin {
                access_key_id: profile.auth.username.clone(),
                access_key_secret: Arc::downgrade(&access_key_secret),
            });
            builder = builder.with_auth_plugin(plugin.clone());
            (
                NacosRequestAuth::MseAccessKey {
                    access_key_id: profile.auth.username.clone(),
                    access_key_secret,
                },
                Some(plugin),
            )
        }
        AuthenticationMode::Digest => {
            return Err(RegistryError::validation(
                "Nacos does not support ZooKeeper digest authentication",
            ));
        }
    };
    Ok(NacosAuthConfiguration {
        config_builder: builder,
        request_auth,
        sdk_auth,
    })
}

struct NacosHttpAuthPlugin {
    http: reqwest::Client,
    login_base: String,
    login_path: &'static str,
    form_encoded: bool,
    username: String,
    password: Weak<ConnectionSecret>,
    token: Weak<RwLock<Option<ConnectionSecret>>>,
}

#[async_trait::async_trait]
impl AuthPlugin for NacosHttpAuthPlugin {
    async fn login(&self, _server_list: Arc<Vec<String>>, _auth_context: Arc<AuthContext>) {
        let (Some(password), Some(token_state)) = (self.password.upgrade(), self.token.upgrade())
        else {
            return;
        };
        let request = self
            .http
            .post(format!("{}{}", self.login_base, self.login_path));
        let credentials = [
            ("username", self.username.as_str()),
            ("password", password.expose()),
        ];
        let request = if self.form_encoded {
            request.form(&credentials)
        } else {
            request.query(&credentials)
        };
        let response = request.send().await;
        let Ok(response) = response.and_then(reqwest::Response::error_for_status) else {
            return;
        };
        let Ok(login) = response.json::<NacosLoginResponse>().await else {
            return;
        };
        let mut token = token_state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *token = Some(ConnectionSecret::new(login.access_token));
    }

    fn get_login_identity(&self, _resource: RequestResource) -> LoginIdentityContext {
        let Some(token_state) = self.token.upgrade() else {
            return LoginIdentityContext::default();
        };
        let token = token_state
            .read()
            .unwrap_or_else(|error| error.into_inner());
        match token.as_ref() {
            Some(token) => LoginIdentityContext::default()
                .add_context("accessToken", token.expose().to_owned()),
            None => LoginIdentityContext::default(),
        }
    }
}

struct NacosStaticAuthPlugin {
    key: String,
    secret: Weak<ConnectionSecret>,
}

struct NacosMseAuthPlugin {
    access_key_id: String,
    access_key_secret: Weak<ConnectionSecret>,
}

#[async_trait::async_trait]
impl AuthPlugin for NacosMseAuthPlugin {
    async fn login(&self, _server_list: Arc<Vec<String>>, _auth_context: Arc<AuthContext>) {}

    fn get_login_identity(&self, resource: RequestResource) -> LoginIdentityContext {
        let Some(access_key_secret) = self.access_key_secret.upgrade() else {
            return LoginIdentityContext::default();
        };
        mse_sdk_identity_at(
            &self.access_key_id,
            &access_key_secret,
            resource,
            current_timestamp_millis(),
        )
        .into_iter()
        .fold(LoginIdentityContext::default(), |identity, (key, value)| {
            identity.add_context(key, value)
        })
    }
}

fn mse_sdk_identity_at(
    access_key_id: &str,
    access_key_secret: &ConnectionSecret,
    resource: RequestResource,
    timestamp_millis: u128,
) -> HashMap<String, String> {
    let timestamp = timestamp_millis.to_string();
    match resource.request_type.as_str() {
        "Config" => {
            let signature = mse_signature(
                access_key_secret,
                &mse_config_resource(
                    resource.namespace.as_deref().unwrap_or_default(),
                    resource.group.as_deref().unwrap_or_default(),
                ),
                &timestamp,
            );
            HashMap::from([
                ("Spas-AccessKey".to_owned(), access_key_id.to_owned()),
                ("Timestamp".to_owned(), timestamp),
                ("Spas-Signature".to_owned(), signature),
            ])
        }
        "Naming" => {
            let Some(service_name) = resource.resource else {
                return HashMap::new();
            };
            let grouped_name = if service_name.contains("@@") {
                service_name
            } else if let Some(group) = resource.group.filter(|group| !group.is_empty()) {
                format!("{group}@@{service_name}")
            } else {
                service_name
            };
            let data = if grouped_name.is_empty() {
                timestamp
            } else {
                format!("{timestamp}@@{grouped_name}")
            };
            let signature = sign_hmac_sha1(access_key_secret, &data);
            HashMap::from([
                ("ak".to_owned(), access_key_id.to_owned()),
                ("data".to_owned(), data),
                ("signature".to_owned(), signature),
            ])
        }
        _ => HashMap::new(),
    }
}

fn mse_signature(access_key_secret: &ConnectionSecret, resource: &str, timestamp: &str) -> String {
    let sign_data = if resource.is_empty() {
        timestamp.to_owned()
    } else {
        format!("{resource}+{timestamp}")
    };
    sign_hmac_sha1(access_key_secret, &sign_data)
}

fn sign_hmac_sha1(access_key_secret: &ConnectionSecret, data: &str) -> String {
    let key = hmac::Key::new(
        hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        access_key_secret.expose().as_bytes(),
    );
    STANDARD.encode(hmac::sign(&key, data.as_bytes()).as_ref())
}

#[async_trait::async_trait]
impl AuthPlugin for NacosStaticAuthPlugin {
    async fn login(&self, _server_list: Arc<Vec<String>>, _auth_context: Arc<AuthContext>) {}

    fn get_login_identity(&self, _resource: RequestResource) -> LoginIdentityContext {
        match self.secret.upgrade() {
            Some(secret) => LoginIdentityContext::default()
                .add_context(self.key.clone(), secret.expose().to_owned()),
            None => LoginIdentityContext::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosLoginResponse {
    access_token: String,
}

fn required_secret<'a>(
    profile: &ConnectionProfile,
    secret: Option<&'a Arc<ConnectionSecret>>,
) -> Result<&'a Arc<ConnectionSecret>, RegistryError> {
    secret.ok_or_else(|| {
        RegistryError::credential_missing(format!(
            "connection '{}' requires a credential",
            profile.id
        ))
    })
}

fn first_endpoint(endpoints: &str) -> &str {
    endpoints
        .split(',')
        .map(str::trim)
        .find(|endpoint| !endpoint.is_empty())
        .unwrap_or(endpoints)
}

fn nacos_server_base(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    let endpoint = endpoint.strip_suffix("/nacos").unwrap_or(endpoint);
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_owned()
    } else {
        format!("http://{endpoint}")
    }
}

fn login_settings(version: NacosApiVersion) -> (&'static str, bool, TokenTransport) {
    match version {
        NacosApiVersion::V2 => ("/nacos/v1/auth/login", false, TokenTransport::Query),
        NacosApiVersion::V3 => ("/nacos/v3/auth/user/login", true, TokenTransport::Bearer),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use nacos_sdk::api::plugin::RequestResource;

    use crate::credentials::ConnectionSecret;

    use crate::registry::{
        AdapterId, AuthenticationMode, ConnectionAuth, ConnectionProfile, NacosApiVersion,
    };

    use super::{NacosRequestAuth, TokenTransport, configure, login_settings, mse_sdk_identity_at};

    #[test]
    fn login_api_matches_the_selected_nacos_generation() {
        assert_eq!(
            login_settings(NacosApiVersion::V2),
            ("/nacos/v1/auth/login", false, TokenTransport::Query)
        );
        assert_eq!(
            login_settings(NacosApiVersion::V3),
            ("/nacos/v3/auth/user/login", true, TokenTransport::Bearer)
        );
    }

    #[test]
    fn v3_tokens_use_authorization_headers_instead_of_urls() {
        let auth = NacosRequestAuth::Token {
            token: Arc::new(RwLock::new(Some(ConnectionSecret::new("TOP_SECRET")))),
            transport: TokenTransport::Bearer,
        };
        let request = auth
            .apply(reqwest::Client::new().get("http://nacos.test/nacos/v3/admin/cs/config/list"))
            .build()
            .unwrap();

        assert_eq!(request.url().query(), None);
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer TOP_SECRET"
        );
    }

    #[test]
    fn mse_access_keys_sign_authoritative_http_requests_without_leaking_credentials_in_urls() {
        let auth = NacosRequestAuth::MseAccessKey {
            access_key_id: "LTAI_REDACTED".to_owned(),
            access_key_secret: Arc::new(ConnectionSecret::new("SECRET_REDACTED")),
        };
        let request = auth
            .apply_with_resource_at(
                reqwest::Client::new().get("http://mse.test/nacos/v1/cs/configs"),
                "tenant-a+DEFAULT_GROUP",
                1_720_000_000_000,
            )
            .build()
            .unwrap();

        assert_eq!(request.url().query(), None);
        assert_eq!(
            request.headers().get("Spas-AccessKey").unwrap(),
            "LTAI_REDACTED"
        );
        assert_eq!(request.headers().get("Timestamp").unwrap(), "1720000000000");
        assert_eq!(
            request.headers().get("Spas-Signature").unwrap(),
            "O+p5azqJh6vbdus58N3La0NqYZw="
        );
    }

    #[test]
    fn mse_access_key_profile_configures_the_nacos_clients_for_ram_authentication() {
        let profile = ConnectionProfile {
            id: "mse".to_owned(),
            name: "MSE".to_owned(),
            adapter: AdapterId::Nacos,
            endpoint: "mse.test:8848".to_owned(),
            namespace: "tenant-a".to_owned(),
            nacos_api_version: NacosApiVersion::V2,
            environment: Default::default(),
            auth: ConnectionAuth {
                mode: AuthenticationMode::MseAccessKey,
                username: "LTAI_REDACTED".to_owned(),
                custom_key: String::new(),
            },
            tls: Default::default(),
        };
        let secret = Arc::new(ConnectionSecret::new("SECRET_REDACTED"));
        let configuration = configure(
            nacos_sdk::api::config::ConfigServiceBuilder::new(
                nacos_sdk::api::props::ClientProps::new()
                    .server_addr(&profile.endpoint)
                    .namespace(&profile.namespace),
            ),
            reqwest::Client::new(),
            &profile,
            Some(&secret),
        )
        .expect("MSE AccessKey profiles should configure Nacos authentication");

        assert!(configuration.sdk_auth.is_some());
        assert!(matches!(
            configuration.request_auth,
            NacosRequestAuth::MseAccessKey { .. }
        ));
    }

    #[test]
    fn mse_sdk_identity_signs_config_and_naming_resources() {
        let secret = ConnectionSecret::new("SECRET_REDACTED");
        let config = mse_sdk_identity_at(
            "LTAI_REDACTED",
            &secret,
            RequestResource {
                request_type: "Config".to_owned(),
                namespace: Some("tenant-a".to_owned()),
                group: Some("DEFAULT_GROUP".to_owned()),
                resource: Some("application.yaml".to_owned()),
            },
            1_720_000_000_000,
        );
        assert_eq!(config.get("Spas-AccessKey").unwrap(), "LTAI_REDACTED");
        assert_eq!(config.get("Timestamp").unwrap(), "1720000000000");
        assert_eq!(
            config.get("Spas-Signature").unwrap(),
            "O+p5azqJh6vbdus58N3La0NqYZw="
        );

        let naming = mse_sdk_identity_at(
            "LTAI_REDACTED",
            &secret,
            RequestResource {
                request_type: "Naming".to_owned(),
                namespace: Some("tenant-a".to_owned()),
                group: Some("DEFAULT_GROUP".to_owned()),
                resource: Some("payments".to_owned()),
            },
            1_720_000_000_000,
        );
        assert_eq!(naming.get("ak").unwrap(), "LTAI_REDACTED");
        assert_eq!(
            naming.get("data").unwrap(),
            "1720000000000@@DEFAULT_GROUP@@payments"
        );
        assert_eq!(
            naming.get("signature").unwrap(),
            "Utit0XY4tOMVCDkmIGTS3h9DIqc="
        );
    }
}
