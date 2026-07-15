use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use atlas_registry_lib::{
    connections::ConnectionStore,
    credentials::{
        CredentialBackend, CredentialBackendError, CredentialUpdate, CredentialVault,
        TransientCredential,
    },
    registry::{
        AdapterId, AuthenticationMode, ConnectionAuth, ConnectionEnvironment, ConnectionProfile,
        NacosApiVersion, RegistryErrorCode, TlsProfile,
    },
};

#[derive(Default)]
struct MemoryBackend {
    values: Mutex<BTreeMap<String, String>>,
}

#[derive(Default)]
struct FailOnceBackend {
    values: Mutex<BTreeMap<String, String>>,
    fail_next_write: AtomicBool,
}

impl CredentialBackend for FailOnceBackend {
    fn read(&self, key: &str) -> Result<Option<String>, CredentialBackendError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn write(&self, key: &str, secret: &str) -> Result<(), CredentialBackendError> {
        if self.fail_next_write.swap(false, Ordering::SeqCst) {
            return Err(CredentialBackendError::new("injected credential failure"));
        }
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialBackendError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

impl CredentialBackend for MemoryBackend {
    fn read(&self, key: &str) -> Result<Option<String>, CredentialBackendError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn write(&self, key: &str, secret: &str) -> Result<(), CredentialBackendError> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialBackendError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

#[test]
fn credential_vault_replaces_preserves_and_clears_system_secrets() {
    tauri::async_runtime::block_on(async {
        let vault = CredentialVault::new(Arc::new(MemoryBackend::default()));

        vault
            .apply("connection-a", CredentialUpdate::replace("first-secret"))
            .await
            .expect("replacement should be stored");
        vault
            .apply("connection-a", CredentialUpdate::preserve())
            .await
            .expect("preserve should not overwrite the stored secret");
        assert_eq!(
            vault
                .required("connection-a")
                .await
                .expect("stored secret should resolve")
                .expose(),
            "first-secret"
        );

        vault
            .apply("connection-a", CredentialUpdate::clear())
            .await
            .expect("clear should remove the stored secret");
        let error = match vault.required("connection-a").await {
            Ok(_) => panic!("cleared credentials must not silently authenticate"),
            Err(error) => error,
        };
        assert_eq!(error.code, RegistryErrorCode::CredentialMissing);
    });
}

#[test]
fn transient_connection_credentials_override_without_being_persisted() {
    tauri::async_runtime::block_on(async {
        let backend = Arc::new(MemoryBackend::default());
        let vault = CredentialVault::new(backend.clone());
        let profile = authenticated_etcd_profile("transient");

        let resolved = vault
            .resolve(&profile, Some(TransientCredential::new("one-shot-secret")))
            .await
            .expect("transient credential should resolve")
            .expect("authenticated connection should have a secret");

        assert_eq!(resolved.expose(), "one-shot-secret");
        assert!(backend.values.lock().unwrap().is_empty());
    });
}

#[test]
fn connection_store_persists_only_non_secret_profile_data() {
    tauri::async_runtime::block_on(async {
        let backend = Arc::new(MemoryBackend::default());
        let vault = CredentialVault::new(backend.clone());
        let directory = std::env::temp_dir().join(format!(
            "atlas-connection-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("connections.json");
        let store = ConnectionStore::new(path.clone(), vault.clone());
        let mut profile = authenticated_etcd_profile("secure-etcd");
        profile.environment = ConnectionEnvironment::Production;
        profile.tls = TlsProfile {
            enabled: true,
            ca_certificate_path: "/certs/ca.pem".to_owned(),
            client_certificate_path: "/certs/client.pem".to_owned(),
            client_key_path: "/certs/client-key.pem".to_owned(),
            server_name: "etcd.internal".to_owned(),
        };

        store
            .upsert(
                profile.clone(),
                CredentialUpdate::replace("TOP_SECRET_PASSWORD"),
            )
            .await
            .expect("profile and credential should save together");

        let json = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(json.contains("atlas-user"));
        assert!(json.contains("/certs/client-key.pem"));
        assert!(!json.contains("TOP_SECRET_PASSWORD"));
        assert_eq!(store.load().await.unwrap(), vec![profile]);
        assert_eq!(
            vault.required("secure-etcd").await.unwrap().expose(),
            "TOP_SECRET_PASSWORD"
        );

        store.delete("secure-etcd").await.unwrap();
        assert!(store.load().await.unwrap().is_empty());
        assert!(backend.values.lock().unwrap().is_empty());

        tokio::fs::remove_dir_all(directory).await.unwrap();
    });
}

#[test]
fn connection_store_rolls_back_profile_when_credential_update_fails() {
    tauri::async_runtime::block_on(async {
        let backend = Arc::new(FailOnceBackend::default());
        let vault = CredentialVault::new(backend.clone());
        let directory = std::env::temp_dir().join(format!(
            "atlas-connection-rollback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = ConnectionStore::new(directory.join("connections.json"), vault.clone());
        let original = authenticated_etcd_profile("rollback-etcd");
        store
            .upsert(original.clone(), CredentialUpdate::replace("old-secret"))
            .await
            .unwrap();

        let mut replacement = original.clone();
        replacement.name = "Must be rolled back".to_owned();
        backend.fail_next_write.store(true, Ordering::SeqCst);
        let error = store
            .upsert(replacement, CredentialUpdate::replace("new-secret"))
            .await
            .expect_err("credential failure should reject the whole update");

        assert_eq!(error.code, RegistryErrorCode::CredentialStore);
        assert_eq!(store.load().await.unwrap(), vec![original]);
        assert_eq!(
            vault.required("rollback-etcd").await.unwrap().expose(),
            "old-secret"
        );
        tokio::fs::remove_dir_all(directory).await.unwrap();
    });
}

fn authenticated_etcd_profile(id: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.to_owned(),
        name: "Secure etcd".to_owned(),
        adapter: AdapterId::Etcd,
        endpoint: "https://etcd.internal:2379".to_owned(),
        namespace: String::new(),
        nacos_api_version: NacosApiVersion::V2,
        environment: ConnectionEnvironment::default(),
        auth: ConnectionAuth {
            mode: AuthenticationMode::UsernamePassword,
            username: "atlas-user".to_owned(),
            custom_key: String::new(),
        },
        tls: TlsProfile::default(),
    }
}
