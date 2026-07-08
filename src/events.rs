use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::util::home_dir;

/// One session-routing telemetry event, produced by a watch pass. This is the
/// in-memory shape; persistence goes to Counterspell's own dedicated stream
/// (`~/.counterspell/events.jsonl`), never the shared fleet feed dir — that
/// separation is the fix for the high-frequency telemetry flooding
/// `~/.factory-lanes/feed/*.jsonl` (counterspell-910).
#[derive(Debug, Clone)]
pub(crate) struct FeedEvent {
    pub(crate) session_id: String,
    pub(crate) pane: String,
    pub(crate) from_model: String,
    pub(crate) to_model: String,
    pub(crate) gate: String,
    pub(crate) action: String,
    pub(crate) action_taken: String,
    pub(crate) origin: String,
}

/// The durable on-disk record for one activation event. Kept flat and
/// self-describing so the desktop app (and any future consumer) can tail the
/// stream without the weave envelope's fleet-feed ceremony.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActivationRecord {
    pub(crate) occurred_at: String,
    pub(crate) occurred_at_unix: i64,
    pub(crate) session_id: String,
    pub(crate) pane: String,
    pub(crate) from_model: String,
    pub(crate) to_model: String,
    pub(crate) gate: String,
    pub(crate) action: String,
    pub(crate) action_taken: String,
    pub(crate) origin: String,
}

/// Rotate the active stream once it crosses this size, so a long-lived install
/// never grows an unbounded JSONL file (the 16MB day-file that flooded the
/// shared feed is exactly what this prevents recurring in the dedicated
/// stream). One rotated generation is kept; older data ages out.
const MAX_STREAM_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn append_activation_events(events: &[FeedEvent], now: DateTime<Utc>) -> Result<()> {
    append_events_to(&events_path()?, events, now)
}

fn append_events_to(
    path: &std::path::Path,
    events: &[FeedEvent],
    now: DateTime<Utc>,
) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create events dir {}", parent.display()))?;
    }
    rotate_if_needed(path)?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open events stream {}", path.display()))?;

    for event in events {
        serde_json::to_writer(&mut file, &record_from(event, now))
            .with_context(|| format!("write event {}", path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("write event newline {}", path.display()))?;
    }

    Ok(())
}

/// Read the most recent activation records in chronological (oldest-first)
/// order, drawing from the rotated generation and the active stream. `limit`
/// caps the returned count to the newest N. Malformed lines are skipped rather
/// than failing the whole read — telemetry must never be able to break the
/// surface that displays it.
pub(crate) fn read_recent_records(limit: usize) -> Result<Vec<ActivationRecord>> {
    read_records_from(&events_path()?, limit)
}

fn read_records_from(path: &std::path::Path, limit: usize) -> Result<Vec<ActivationRecord>> {
    let mut records = Vec::new();
    for candidate in [rotated_path(path), path.to_path_buf()] {
        if let Ok(contents) = fs::read_to_string(&candidate) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(record) = serde_json::from_str::<ActivationRecord>(line) {
                    records.push(record);
                }
            }
        }
    }
    records.sort_by_key(|record| record.occurred_at_unix);
    if records.len() > limit {
        records.drain(0..records.len() - limit);
    }
    Ok(records)
}

fn rotate_if_needed(path: &std::path::Path) -> Result<()> {
    let too_big = fs::metadata(path)
        .map(|meta| meta.len() >= MAX_STREAM_BYTES)
        .unwrap_or(false);
    if too_big {
        let rotated = rotated_path(path);
        fs::rename(path, &rotated)
            .with_context(|| format!("rotate events stream to {}", rotated.display()))?;
    }
    Ok(())
}

fn rotated_path(path: &std::path::Path) -> PathBuf {
    let mut rotated = path.to_path_buf();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "events.jsonl".to_string());
    rotated.set_file_name(format!("{name}.1"));
    rotated
}

fn events_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("COUNTERSPELL_EVENTS_PATH") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".counterspell").join("events.jsonl"))
}

fn record_from(event: &FeedEvent, now: DateTime<Utc>) -> ActivationRecord {
    ActivationRecord {
        occurred_at: now.to_rfc3339(),
        occurred_at_unix: now.timestamp(),
        session_id: event.session_id.clone(),
        pane: event.pane.clone(),
        from_model: event.from_model.clone(),
        to_model: event.to_model.clone(),
        gate: event.gate.clone(),
        action: event.action.clone(),
        action_taken: event.action_taken.clone(),
        origin: event.origin.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(action: &str, action_taken: &str) -> FeedEvent {
        FeedEvent {
            session_id: "sess-123456".to_string(),
            pane: "w30:p1".to_string(),
            from_model: "claude-fable-5".to_string(),
            to_model: "claude-opus-4-8".to_string(),
            gate: "allow".to_string(),
            action: action.to_string(),
            action_taken: action_taken.to_string(),
            origin: "downgraded-from-fable".to_string(),
        }
    }

    #[test]
    fn append_then_read_round_trips_in_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("events.jsonl");

        let t0 = Utc::now();
        append_events_to(
            &path,
            &[sample("model_drift_detected", "remediation-started")],
            t0,
        )
        .expect("append first");
        let t1 = t0 + chrono::Duration::seconds(5);
        append_events_to(
            &path,
            &[sample("model_switched", "model_switched:claude-fable-5")],
            t1,
        )
        .expect("append second");

        let records = read_records_from(&path, 10).expect("read");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].action, "model_drift_detected");
        assert_eq!(records[1].action, "model_switched");
    }

    #[test]
    fn read_limit_returns_newest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("events.jsonl");

        let base = Utc::now();
        for index in 0..5 {
            let at = base + chrono::Duration::seconds(index);
            append_events_to(&path, &[sample(&format!("action_{index}"), "none")], at)
                .expect("append");
        }
        let records = read_records_from(&path, 2).expect("read");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].action, "action_3");
        assert_eq!(records[1].action, "action_4");
    }
}
