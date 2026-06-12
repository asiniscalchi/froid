//! Transport-agnostic export/import of raw journal entries.
//!
//! The JSON envelope (versioned, with one record per stored message) is the
//! data-portability format: users pull their journal out via the Telegram
//! `/export` command and load it back with `/import`. Import is
//! all-or-nothing — a single collision with an existing message aborts the
//! whole batch.

use std::{error::Error, fmt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::journal::registry::JournalServiceRegistry;
use crate::journal::repository::{BulkImportError, JournalEntryRecord, JournalRepository};

pub const EXPORT_FORMAT_VERSION: u32 = 2;
pub const MIN_SUPPORTED_IMPORT_VERSION: u32 = 1;

#[derive(Debug)]
pub enum TransferError {
    Storage(String),
    Serialization(String),
    InvalidPayload(String),
    UnsupportedVersion {
        version: u32,
    },
    Conflict {
        source: String,
        source_conversation_id: String,
        source_message_id: String,
    },
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(message) => write!(f, "storage error: {message}"),
            Self::Serialization(message) => write!(f, "failed to serialize export: {message}"),
            Self::InvalidPayload(message) => write!(f, "invalid export file: {message}"),
            Self::UnsupportedVersion { version } => write!(
                f,
                "unsupported export version {version} (supported {MIN_SUPPORTED_IMPORT_VERSION}..={EXPORT_FORMAT_VERSION})"
            ),
            Self::Conflict {
                source,
                source_conversation_id,
                source_message_id,
            } => write!(
                f,
                "import aborted: entry ({source}, {source_conversation_id}, {source_message_id}) collides with an existing message"
            ),
        }
    }
}

impl Error for TransferError {}

#[derive(Serialize)]
struct ExportedMessage {
    id: String,
    source: String,
    source_conversation_id: String,
    source_message_id: String,
    text: String,
    received_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct ExportEnvelope {
    version: u32,
    exported_at: DateTime<Utc>,
    messages: Vec<ExportedMessage>,
}

#[derive(Deserialize)]
struct ImportedMessage {
    #[serde(default)]
    id: Option<String>,
    source: String,
    source_conversation_id: String,
    source_message_id: String,
    text: String,
    received_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ImportEnvelope {
    version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    exported_at: Option<DateTime<Utc>>,
    messages: Vec<ImportedMessage>,
}

/// A rendered export: suggested file name plus the JSON payload.
pub struct ExportFile {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub message_count: usize,
}

/// Serialize every stored message of `repo` into the export envelope.
pub async fn export_json(repo: &JournalRepository) -> Result<ExportFile, TransferError> {
    let records = repo
        .fetch_all_for_export()
        .await
        .map_err(|err| TransferError::Storage(err.to_string()))?;

    let envelope = ExportEnvelope {
        version: EXPORT_FORMAT_VERSION,
        exported_at: Utc::now(),
        messages: records
            .into_iter()
            .map(|r| ExportedMessage {
                id: r.id,
                source: r.source,
                source_conversation_id: r.source_conversation_id,
                source_message_id: r.source_message_id,
                text: r.text,
                received_at: r.received_at,
            })
            .collect(),
    };

    let bytes = serde_json::to_vec(&envelope)
        .map_err(|err| TransferError::Serialization(err.to_string()))?;
    let filename = format!(
        "froid-messages-{}.json",
        envelope.exported_at.format("%Y-%m-%d")
    );

    Ok(ExportFile {
        filename,
        bytes,
        message_count: envelope.messages.len(),
    })
}

/// Parse an export envelope and bulk-insert its messages into `repo`.
/// Returns the number of imported messages.
pub async fn import_json(repo: &JournalRepository, payload: &[u8]) -> Result<usize, TransferError> {
    let envelope: ImportEnvelope = serde_json::from_slice(payload)
        .map_err(|err| TransferError::InvalidPayload(err.to_string()))?;

    if envelope.version < MIN_SUPPORTED_IMPORT_VERSION || envelope.version > EXPORT_FORMAT_VERSION {
        return Err(TransferError::UnsupportedVersion {
            version: envelope.version,
        });
    }

    if envelope.messages.is_empty() {
        return Ok(0);
    }

    let records: Vec<JournalEntryRecord> = envelope
        .messages
        .into_iter()
        .map(|m| JournalEntryRecord {
            id: m.id.unwrap_or_default(),
            source: m.source,
            source_conversation_id: m.source_conversation_id,
            source_message_id: m.source_message_id,
            text: m.text,
            received_at: m.received_at,
        })
        .collect();

    repo.bulk_import(&records).await.map_err(|err| match err {
        BulkImportError::Conflict {
            source,
            source_conversation_id,
            source_message_id,
        } => TransferError::Conflict {
            source,
            source_conversation_id,
            source_message_id,
        },
        BulkImportError::Database(err) => TransferError::Storage(err.to_string()),
    })
}

/// Per-tenant export/import on top of the registry's isolated databases.
#[derive(Clone)]
pub struct TransferService {
    registry: JournalServiceRegistry,
}

impl TransferService {
    pub fn new(registry: JournalServiceRegistry) -> Self {
        Self { registry }
    }

    async fn repository(&self, chat_id: &str) -> Result<JournalRepository, TransferError> {
        let pool = self
            .registry
            .pool(chat_id)
            .await
            .map_err(|err| TransferError::Storage(err.to_string()))?;
        Ok(JournalRepository::new(pool))
    }

    pub async fn export(&self, chat_id: &str) -> Result<ExportFile, TransferError> {
        export_json(&self.repository(chat_id).await?).await
    }

    pub async fn import(&self, chat_id: &str, payload: &[u8]) -> Result<usize, TransferError> {
        import_json(&self.repository(chat_id).await?, payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::messages::{IncomingMessage, MessageSource};
    use serde_json::{Value, json};

    async fn repo() -> JournalRepository {
        let pool = crate::database::test_pool().await;
        JournalRepository::new(pool)
    }

    fn incoming(message_id: &str, text: &str) -> IncomingMessage {
        IncomingMessage {
            source: MessageSource::Telegram,
            source_conversation_id: "42".to_string(),
            source_message_id: message_id.to_string(),
            text: text.to_string(),
            received_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn export_produces_versioned_envelope_with_all_messages() {
        let repo = repo().await;
        repo.store(&incoming("m1", "first")).await.unwrap();
        repo.store(&incoming("m2", "second")).await.unwrap();

        let export = export_json(&repo).await.unwrap();

        assert_eq!(export.message_count, 2);
        assert!(export.filename.starts_with("froid-messages-"));
        assert!(export.filename.ends_with(".json"));
        let payload: Value = serde_json::from_slice(&export.bytes).unwrap();
        assert_eq!(payload["version"], EXPORT_FORMAT_VERSION);
        let messages = payload["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        let mut texts: Vec<&str> = messages
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        texts.sort_unstable();
        assert_eq!(texts, vec!["first", "second"]);
        assert!(messages.iter().all(|m| m["source_conversation_id"] == "42"));
    }

    #[tokio::test]
    async fn export_then_import_roundtrips_into_empty_journal() {
        let source = repo().await;
        source.store(&incoming("m1", "roundtrip")).await.unwrap();
        let export = export_json(&source).await.unwrap();

        let target = repo().await;
        let imported = import_json(&target, &export.bytes).await.unwrap();

        assert_eq!(imported, 1);
        let records = target.fetch_all_for_export().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, "roundtrip");
    }

    #[tokio::test]
    async fn import_rejects_invalid_json() {
        let repo = repo().await;

        let error = import_json(&repo, b"not json").await.unwrap_err();

        assert!(matches!(error, TransferError::InvalidPayload(_)));
    }

    #[tokio::test]
    async fn import_rejects_unsupported_version() {
        let repo = repo().await;
        let payload = json!({"version": 99, "messages": []}).to_string();

        let error = import_json(&repo, payload.as_bytes()).await.unwrap_err();

        assert!(matches!(
            error,
            TransferError::UnsupportedVersion { version: 99 }
        ));
    }

    #[tokio::test]
    async fn import_aborts_on_conflict_with_existing_message() {
        let repo = repo().await;
        repo.store(&incoming("m1", "already here")).await.unwrap();
        let export = export_json(&repo).await.unwrap();

        let error = import_json(&repo, &export.bytes).await.unwrap_err();

        assert!(matches!(error, TransferError::Conflict { .. }));
        assert_eq!(repo.fetch_all_for_export().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn import_of_empty_envelope_is_a_noop() {
        let repo = repo().await;
        let payload = json!({"version": 2, "messages": []}).to_string();

        assert_eq!(import_json(&repo, payload.as_bytes()).await.unwrap(), 0);
    }
}
