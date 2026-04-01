//! Versioned on-disk session state for Phase 5 (resume + compaction metadata).
//!
//! Interactive CLI persists to a JSON file; schema bumps allow migrations. Compaction archives
//! live under `~/.zeroclaw/sessions/archives/` as JSONL lines of [`ChatMessage`].

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::ChatMessage;

/// Current on-disk schema for [`SessionRecord`].
pub const SESSION_RECORD_VERSION: u32 = 2;

/// Metadata for compaction: pointers to archived segments (full message JSONL) + latest summary hint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCompactionMeta {
    /// Paths relative to `~/.zeroclaw/sessions/` (e.g. `archives/<uuid>.jsonl`).
    #[serde(default)]
    pub archive_paths: Vec<String>,
    /// Short excerpt of the last compaction summary (for tooling; full text remains in `history`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_summary_excerpt: Option<String>,
}

/// Persistent session record (interactive CLI and future resume paths).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub version: u32,
    pub history: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<SessionCompactionMeta>,
}

#[derive(Deserialize)]
struct LegacySessionV1 {
    #[allow(dead_code)]
    version: u32,
    history: Vec<ChatMessage>,
}

/// `~/.zeroclaw/sessions` or None if home is unavailable.
#[must_use]
pub fn sessions_root_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.home_dir().join(".zeroclaw").join("sessions"))
}

fn ensure_system_prompt(history: &mut Vec<ChatMessage>, system_prompt: &str) {
    if history.is_empty() {
        history.push(ChatMessage::system(system_prompt));
    } else if history.first().map(|m| m.role.as_str()) != Some("system") {
        history.insert(0, ChatMessage::system(system_prompt));
    }
}

/// Load session file; migrate legacy v1; ensure leading system message.
pub fn load_session_record(path: &Path, system_prompt: &str) -> Result<SessionRecord> {
    if !path.exists() {
        return Ok(SessionRecord {
            version: SESSION_RECORD_VERSION,
            history: vec![ChatMessage::system(system_prompt)],
            compaction: None,
        });
    }

    let raw = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;

    let ver = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(2) as u32;

    let mut record = if ver == 1 {
        let legacy: LegacySessionV1 = serde_json::from_value(value)
            .with_context(|| format!("legacy session {}", path.display()))?;
        SessionRecord {
            version: SESSION_RECORD_VERSION,
            history: legacy.history,
            compaction: None,
        }
    } else {
        serde_json::from_value(value).with_context(|| format!("session {}", path.display()))?
    };

    record.version = SESSION_RECORD_VERSION;
    ensure_system_prompt(&mut record.history, system_prompt);
    Ok(record)
}

/// Write pretty JSON session file (v2).
pub fn save_session_record(path: &Path, record: &SessionRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string_pretty(record)?;
    std::fs::write(path, payload).with_context(|| path.display().to_string())?;
    Ok(())
}

/// Append one compaction segment as JSONL under `sessions/archives/`. Returns relative path from `sessions/`.
pub fn write_compaction_archive(messages: &[ChatMessage]) -> Result<Option<String>> {
    let Some(root) = sessions_root_dir() else {
        tracing::warn!("compaction archive: no home directory; skip");
        return Ok(None);
    };
    let arch = root.join("archives");
    std::fs::create_dir_all(&arch).with_context(|| arch.display().to_string())?;
    let name = format!("{}.jsonl", Uuid::new_v4());
    let full = arch.join(&name);
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&full)
        .with_context(|| full.display().to_string())?;
    for m in messages {
        let line = serde_json::to_string(m).context("serialize ChatMessage for archive")?;
        writeln!(f, "{line}")?;
    }
    f.sync_all()?;
    Ok(Some(format!("archives/{name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn v1_round_trips_to_v2_fields() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let v1 = r#"{"version":1,"history":[{"role":"user","content":"hi"}]}"#;
        std::fs::write(&path, v1).unwrap();

        let r = load_session_record(&path, "sys").unwrap();
        assert_eq!(r.version, SESSION_RECORD_VERSION);
        assert_eq!(r.history.len(), 2);
        assert_eq!(r.history[0].role, "system");
        assert_eq!(r.history[0].content, "sys");
        assert_eq!(r.history[1].content, "hi");
        assert!(r.compaction.is_none());
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session.json");
        let rec = SessionRecord {
            version: SESSION_RECORD_VERSION,
            history: vec![ChatMessage::system("s"), ChatMessage::user("u")],
            compaction: Some(SessionCompactionMeta {
                archive_paths: vec!["archives/x.jsonl".into()],
                last_summary_excerpt: Some("bullets".into()),
            }),
        };
        save_session_record(&path, &rec).unwrap();
        let loaded = load_session_record(&path, "fallback").unwrap();
        assert_eq!(loaded.history.len(), rec.history.len());
        assert_eq!(
            loaded.compaction.as_ref().unwrap().archive_paths,
            rec.compaction.as_ref().unwrap().archive_paths
        );
    }
}
