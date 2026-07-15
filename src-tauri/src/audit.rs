use std::{path::Path, sync::Arc, time::SystemTime};

use serde::{Deserialize, Serialize};
use tokio::{
    fs::OpenOptions,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};

use crate::registry::{
    MutationOperation, MutationResult, RegistryError, RegistryErrorCode, ResourceAddress,
    ResourceMutation, ResourceSnapshot,
};

const AUDIT_FILE_NAME: &str = "mutation-audit.jsonl";
const HISTORY_SCAN_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditHistoryKind {
    Started,
    Applied,
    Failed,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditHistoryItem {
    pub kind: AuditHistoryKind,
    pub timestamp_ms: u64,
    pub connection_id: String,
    pub operation_id: String,
    pub operation: Option<MutationOperation>,
    pub address: Option<ResourceAddress>,
    pub expected_version: Option<String>,
    pub previous: Option<ResourceSnapshot>,
    pub current: Option<ResourceSnapshot>,
    pub consistency: Option<crate::registry::MutationConsistency>,
    pub error_code: Option<RegistryErrorCode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditHistoryPage {
    pub items: Vec<AuditHistoryItem>,
    pub next_cursor: Option<String>,
    pub scanned_bytes: usize,
    pub exhaustive: bool,
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
enum StoredAuditEvent {
    #[serde(rename = "mutationStarted")]
    Started {
        timestamp_ms: u64,
        connection_id: String,
        operation_id: String,
        operation: MutationOperation,
        address: ResourceAddress,
        expected_version: Option<String>,
        previous: Option<ResourceSnapshot>,
    },
    #[serde(rename = "mutationApplied")]
    Applied {
        timestamp_ms: u64,
        connection_id: String,
        operation_id: String,
        result: MutationResult,
    },
    #[serde(rename = "mutationFailed")]
    Failed {
        timestamp_ms: u64,
        connection_id: String,
        operation_id: String,
        code: RegistryErrorCode,
    },
    #[serde(rename = "mutationOutcomeUnknown")]
    OutcomeUnknown {
        timestamp_ms: u64,
        connection_id: String,
        operation_id: String,
    },
}

impl StoredAuditEvent {
    fn connection_id(&self) -> &str {
        match self {
            Self::Started { connection_id, .. }
            | Self::Applied { connection_id, .. }
            | Self::Failed { connection_id, .. }
            | Self::OutcomeUnknown { connection_id, .. } => connection_id,
        }
    }
}

impl From<StoredAuditEvent> for AuditHistoryItem {
    fn from(event: StoredAuditEvent) -> Self {
        match event {
            StoredAuditEvent::Started {
                timestamp_ms,
                connection_id,
                operation_id,
                operation,
                address,
                expected_version,
                previous,
            } => Self {
                kind: AuditHistoryKind::Started,
                timestamp_ms,
                connection_id,
                operation_id,
                operation: Some(operation),
                address: Some(address),
                expected_version,
                previous,
                current: None,
                consistency: None,
                error_code: None,
            },
            StoredAuditEvent::Applied {
                timestamp_ms,
                connection_id,
                operation_id,
                result,
            } => Self {
                kind: AuditHistoryKind::Applied,
                timestamp_ms,
                connection_id,
                operation_id,
                operation: Some(result.operation),
                address: Some(result.address),
                expected_version: None,
                previous: result.previous,
                current: result.current,
                consistency: Some(result.consistency),
                error_code: None,
            },
            StoredAuditEvent::Failed {
                timestamp_ms,
                connection_id,
                operation_id,
                code,
            } => Self {
                kind: AuditHistoryKind::Failed,
                timestamp_ms,
                connection_id,
                operation_id,
                operation: None,
                address: None,
                expected_version: None,
                previous: None,
                current: None,
                consistency: None,
                error_code: Some(code),
            },
            StoredAuditEvent::OutcomeUnknown {
                timestamp_ms,
                connection_id,
                operation_id,
            } => Self {
                kind: AuditHistoryKind::OutcomeUnknown,
                timestamp_ms,
                connection_id,
                operation_id,
                operation: None,
                address: None,
                expected_version: None,
                previous: None,
                current: None,
                consistency: None,
                error_code: None,
            },
        }
    }
}

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
    pub async fn load_recent_in(
        &self,
        directory: &Path,
        connection_id: Option<&str>,
        cursor: Option<String>,
        limit: usize,
    ) -> Result<AuditHistoryPage, RegistryError> {
        let _writer = self.writer.clone().lock_owned().await;
        let path = directory.join(AUDIT_FILE_NAME);
        let mut file = match tokio::fs::File::open(path).await {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(AuditHistoryPage {
                    items: Vec::new(),
                    next_cursor: None,
                    scanned_bytes: 0,
                    exhaustive: true,
                });
            }
            Err(error) => {
                return Err(RegistryError::storage(format!(
                    "cannot open audit history: {error}"
                )));
            }
        };
        let file_length = file
            .metadata()
            .await
            .map_err(|error| {
                RegistryError::storage(format!("cannot inspect audit history: {error}"))
            })?
            .len();
        let end = parse_history_cursor(cursor, file_length)?;
        if end == 0 {
            return Ok(AuditHistoryPage {
                items: Vec::new(),
                next_cursor: None,
                scanned_bytes: 0,
                exhaustive: true,
            });
        }
        let start = end.saturating_sub(HISTORY_SCAN_BYTES);
        let length = usize::try_from(end - start)
            .map_err(|_| RegistryError::storage("audit history page is too large"))?;
        let mut bytes = vec![0; length];
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|error| {
                RegistryError::storage(format!("cannot seek audit history: {error}"))
            })?;
        file.read_exact(&mut bytes).await.map_err(|error| {
            RegistryError::storage(format!("cannot read audit history: {error}"))
        })?;

        let first_complete = if start == 0 {
            0
        } else {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|position| position + 1)
                .unwrap_or(bytes.len())
        };
        let mut line_ranges = Vec::new();
        let mut line_start = first_complete;
        for (index, byte) in bytes.iter().enumerate().skip(first_complete) {
            if *byte == b'\n' {
                if index > line_start {
                    line_ranges.push((line_start, index));
                }
                line_start = index + 1;
            }
        }
        if line_start < bytes.len() {
            line_ranges.push((line_start, bytes.len()));
        }

        let limit = limit.clamp(1, 100);
        let mut items = Vec::with_capacity(limit);
        let mut oldest_processed = None;
        for (line_start, line_end) in line_ranges.into_iter().rev() {
            let absolute_start = start + line_start as u64;
            oldest_processed = Some(absolute_start);
            let Ok(event) =
                serde_json::from_slice::<StoredAuditEvent>(&bytes[line_start..line_end])
            else {
                continue;
            };
            if connection_id.is_some_and(|filter| event.connection_id() != filter) {
                continue;
            }
            items.push(event.into());
            if items.len() == limit {
                break;
            }
        }

        let next_offset = if items.len() == limit {
            oldest_processed.filter(|offset| *offset > 0)
        } else if start > 0 {
            let complete_boundary = start + first_complete as u64;
            Some(if complete_boundary < end {
                complete_boundary
            } else {
                start
            })
        } else {
            None
        };
        let next_cursor = next_offset.map(|offset| offset.to_string());
        Ok(AuditHistoryPage {
            items,
            next_cursor: next_cursor.clone(),
            scanned_bytes: bytes.len(),
            exhaustive: next_cursor.is_none(),
        })
    }

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

fn parse_history_cursor(cursor: Option<String>, file_length: u64) -> Result<u64, RegistryError> {
    let offset = cursor
        .map(|cursor| {
            cursor
                .parse::<u64>()
                .map_err(|_| RegistryError::validation("audit history cursor is invalid"))
        })
        .transpose()?
        .unwrap_or(file_length);
    if offset > file_length {
        return Err(RegistryError::validation(
            "audit history cursor is beyond the current log",
        ));
    }
    Ok(offset)
}

fn timestamp_ms() -> Result<u64, RegistryError> {
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| RegistryError::storage(format!("system clock is invalid: {error}")))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| RegistryError::storage("system timestamp does not fit in an audit record"))
}
