use std::sync::{Arc, RwLock, Weak};

use nacos_sdk::api::{
    config::ConfigServiceBuilder,
    plugin::{AuthContext, AuthPlugin, LoginIdentityContext, RequestResource},
};
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
}

impl NacosRequestAuth {
    pub(super) fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
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
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TokenTransport {
    Query,
    Bearer,
}

pub(super) fn configure(
    mut builder: ConfigServiceBuilder,
    http: reqwest::Client,
    profile: &ConnectionProfile,
    secret: Option<&Arc<ConnectionSecret>>,
) -> Result<(ConfigServiceBuilder, NacosRequestAuth), RegistryError> {
    let request_auth = match profile.auth.mode {
        AuthenticationMode::None => NacosRequestAuth::None,
        AuthenticationMode::UsernamePassword => {
            let secret = required_secret(profile, secret)?;
            let token = Arc::new(RwLock::new(None));
            let (login_path, form_encoded, transport) = login_settings(profile.nacos_api_version);
            builder = builder.with_auth_plugin(Arc::new(NacosHttpAuthPlugin {
                http,
                login_base: nacos_server_base(first_endpoint(&profile.endpoint)),
                login_path,
                form_encoded,
                username: profile.auth.username.clone(),
                password: Arc::downgrade(secret),
                token: Arc::downgrade(&token),
            }));
            NacosRequestAuth::Token { token, transport }
        }
        AuthenticationMode::Custom => {
            let secret = required_secret(profile, secret)?.clone();
            builder = builder.with_auth_plugin(Arc::new(NacosStaticAuthPlugin {
                key: profile.auth.custom_key.clone(),
                secret: Arc::downgrade(&secret),
            }));
            NacosRequestAuth::Static {
                key: profile.auth.custom_key.clone(),
                secret,
            }
        }
        AuthenticationMode::Digest => {
            return Err(RegistryError::validation(
                "Nacos does not support ZooKeeper digest authentication",
            ));
        }
    };
    Ok((builder, request_auth))
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

    use crate::credentials::ConnectionSecret;

    use crate::registry::NacosApiVersion;

    use super::{NacosRequestAuth, TokenTransport, login_settings};

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
}
