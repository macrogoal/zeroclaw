//! Append-only JSONL transcript of user and assistant lines (Phase 5 roadmap slice).

use crate::config::SessionTranscriptConfig;
use serde::Serialize;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One line in `~/.zeroclaw/sessions/transcripts/<stem>.jsonl`.
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptRecordV1 {
    pub v: u32,
    pub ts: String,
    /// `"user"` or `"assistant"`.
    pub role: String,
    pub channel: String,
    pub provider: String,
    pub model: String,
    pub content: String,
}

#[derive(Clone)]
pub struct SessionTranscriptScope {
    pub cfg: SessionTranscriptConfig,
    pub session_key: String,
}

tokio::task_local! {
    pub static SESSION_TRANSCRIPT_CONTEXT: Option<SessionTranscriptScope>;
}

pub(crate) fn default_transcript_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| {
        b.home_dir()
            .join(".zeroclaw")
            .join("sessions")
            .join("transcripts")
    })
}

fn transcript_file_stem(session_key: &str) -> String {
    let safe: String = session_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('_');
    let head: String = if safe.is_empty() {
        "session".to_string()
    } else {
        safe.chars().take(120).collect()
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    session_key.hash(&mut h);
    format!("{head}_{:x}", h.finish())
}

fn maybe_truncate_content(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return s.to_string();
    }
    let n = s.chars().count();
    if n <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}\n… [truncated: {n} chars > {max_chars}]\n")
}

fn append_transcript_line_for_config(
    cfg: &SessionTranscriptConfig,
    session_key: &str,
    role: &str,
    channel: &str,
    provider: &str,
    model: &str,
    text: &str,
) {
    if !cfg.enabled || session_key.trim().is_empty() {
        return;
    }
    let Some(base) = default_transcript_dir() else {
        tracing::warn!("session transcript: no home directory; skip");
        return;
    };
    let content = maybe_truncate_content(text, cfg.max_content_chars);
    let record = TranscriptRecordV1 {
        v: 1,
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        role: role.to_string(),
        channel: channel.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        content,
    };
    if let Err(e) = append_record_jsonl(&base, session_key, &record) {
        tracing::warn!(error = %e, "session transcript append failed");
    }
}

/// Append one user line when `cfg.enabled` and paths succeed.
pub(crate) fn append_user_for_config(
    cfg: &SessionTranscriptConfig,
    session_key: &str,
    channel: &str,
    provider: &str,
    model: &str,
    text: &str,
) {
    append_transcript_line_for_config(cfg, session_key, "user", channel, provider, model, text);
}

/// Append one assistant line when `cfg.enabled` and paths succeed.
pub(crate) fn append_assistant_for_config(
    cfg: &SessionTranscriptConfig,
    session_key: &str,
    channel: &str,
    provider: &str,
    model: &str,
    text: &str,
) {
    append_transcript_line_for_config(
        cfg,
        session_key,
        "assistant",
        channel,
        provider,
        model,
        text,
    );
}

fn append_record_jsonl(
    base: &Path,
    session_key: &str,
    record: &TranscriptRecordV1,
) -> std::io::Result<()> {
    std::fs::create_dir_all(base)?;
    let path = base.join(format!("{}.jsonl", transcript_file_stem(session_key)));
    let line = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    f.sync_all()?;
    Ok(())
}

/// Read task-local scope (if any) and append assistant finals.
pub(crate) fn record_assistant_from_task_local(
    display_text: &str,
    ctx_channel: &str,
    ctx_provider: &str,
    ctx_model: &str,
) {
    let Ok(maybe) = SESSION_TRANSCRIPT_CONTEXT.try_with(Clone::clone) else {
        return;
    };
    let Some(scope) = maybe else {
        return;
    };
    append_assistant_for_config(
        &scope.cfg,
        &scope.session_key,
        ctx_channel,
        ctx_provider,
        ctx_model,
        display_text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn transcript_stem_is_stable_for_same_key() {
        let a = transcript_file_stem("tg_user_123");
        let b = transcript_file_stem("tg_user_123");
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn append_writes_jsonl_line() {
        let dir = tempdir().unwrap();
        let record = TranscriptRecordV1 {
            v: 1,
            ts: "2020-01-01T00:00:00Z".to_string(),
            role: "assistant".into(),
            channel: "c".into(),
            provider: "p".into(),
            model: "m".into(),
            content: "x".into(),
        };
        append_record_jsonl(dir.path(), "sess_test", &record).unwrap();
        let stem = transcript_file_stem("sess_test");
        let p = dir.path().join(format!("{stem}.jsonl"));
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("\"role\":\"assistant\""));
        assert!(raw.trim().starts_with('{'));
    }

    #[test]
    fn truncate_respects_max_chars() {
        let s = "abcdefghij".chars().take(100).collect::<String>();
        let out = maybe_truncate_content(&s, 5);
        assert!(out.contains("truncated"));
        assert!(out.chars().count() < 100);
    }
}
