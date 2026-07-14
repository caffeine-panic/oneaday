use std::{path::Path, sync::Arc, time::SystemTime};

use serde::Serialize;
use tokio::{fs::OpenOptions, io::AsyncWriteExt, sync::Mutex};

use crate::registry::{
    MutationOperation, MutationResult, RegistryError, RegistryErrorCode, ResourceAddress,
    ResourceMutation, ResourceSnapshot,
};

const AUDIT_FILE_NAME: &str = "mutation-audit.jsonl";

#[derive(Clone, Default)]
pub struct AuditLog {
    writer: Arc<Mutex<()>>,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
enum AuditEvent<'a> {
    #[serde(rename = "mutationStarted")]
    Started {
        timestamp_ms: u64,
        connection_id: &'a str,
        operation_id: &'a str,
        operation: MutationOperation,
        address: &'a ResourceAddress,
        expected_version: Option<&'a str>,
        previous: Option<&'a ResourceSnapshot>,
    },
    #[serde(rename = "mutationApplied")]
    Applied {
        timestamp_ms: u64,
        connection_id: &'a str,
        operation_id: &'a str,
        result: &'a MutationResult,
    },
    #[serde(rename = "mutationFailed")]
    Failed {
        timestamp_ms: u64,
        connection_id: &'a str,
        operation_id: &'a str,
        code: RegistryErrorCode,
    },
    #[serde(rename = "mutationOutcomeUnknown")]
    OutcomeUnknown {
        timestamp_ms: u64,
        connection_id: &'a str,
        operation_id: &'a str,
    },
}

impl AuditLog {
    pub async fn record_started_in(
        &self,
        directory: &Path,
        connection_id: &str,
        operation_id: &str,
        mutation: &ResourceMutation,
        previous: Option<&ResourceSnapshot>,
    ) -> Result<(), RegistryError> {
        self.append(
            directory,
            &AuditEvent::Started {
                timestamp_ms: timestamp_ms()?,
                connection_id,
                operation_id,
                operation: mutation.operation(),
                address: mutation.address(),
                expected_version: mutation.expected_version(),
                previous,
            },
        )
        .await
    }

    pub async fn record_applied_in(
        &self,
        directory: &Path,
        connection_id: &str,
        operation_id: &str,
        result: &MutationResult,
    ) -> Result<(), RegistryError> {
        self.append(
            directory,
            &AuditEvent::Applied {
                timestamp_ms: timestamp_ms()?,
                connection_id,
                operation_id,
                result,
            },
        )
        .await
    }

    pub async fn record_failed_in(
        &self,
        directory: &Path,
        connection_id: &str,
        operation_id: &str,
        code: RegistryErrorCode,
    ) -> Result<(), RegistryError> {
        self.append(
            directory,
            &AuditEvent::Failed {
                timestamp_ms: timestamp_ms()?,
                connection_id,
                operation_id,
                code,
            },
        )
        .await
    }

    pub async fn record_outcome_unknown_in(
        &self,
        directory: &Path,
        connection_id: &str,
        operation_id: &str,
    ) -> Result<(), RegistryError> {
        self.append(
            directory,
            &AuditEvent::OutcomeUnknown {
                timestamp_ms: timestamp_ms()?,
                connection_id,
                operation_id,
            },
        )
        .await
    }

    async fn append(&self, directory: &Path, event: &AuditEvent<'_>) -> Result<(), RegistryError> {
        let mut bytes = serde_json::to_vec(event).map_err(|error| {
            RegistryError::storage(format!("cannot encode audit event: {error}"))
        })?;
        bytes.push(b'\n');
        let directory = directory.to_path_buf();
        let writer = self.writer.clone().lock_owned().await;
        tokio::spawn(async move {
            let _writer = writer;
            tokio::fs::create_dir_all(&directory)
                .await
                .map_err(|error| {
                    RegistryError::storage(format!("cannot create audit directory: {error}"))
                })?;
            let path = directory.join(AUDIT_FILE_NAME);
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
                .map_err(|error| {
                    RegistryError::storage(format!("cannot open audit log: {error}"))
                })?;
            file.write_all(&bytes).await.map_err(|error| {
                RegistryError::storage(format!("cannot append audit log: {error}"))
            })?;
            file.sync_data()
                .await
                .map_err(|error| RegistryError::storage(format!("cannot sync audit log: {error}")))
        })
        .await
        .map_err(|error| RegistryError::storage(format!("audit writer task failed: {error}")))?
    }
}

fn timestamp_ms() -> Result<u64, RegistryError> {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| RegistryError::storage(format!("system clock is invalid: {error}")))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| RegistryError::storage("system timestamp does not fit in an audit record"))
}
