use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::util::home_dir;

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

pub(crate) fn append_feed_events(events: &[FeedEvent], now: DateTime<Utc>) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    let dir = feed_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("create feed dir {}", dir.display()))?;
    let path = dir.join(format!("{}.jsonl", now.format("%Y-%m-%d")));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open feed {}", path.display()))?;

    for event in events {
        serde_json::to_writer(&mut file, &event_envelope(event, now))
            .with_context(|| format!("write feed event {}", path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("write feed newline {}", path.display()))?;
    }

    Ok(())
}

fn feed_dir() -> Result<PathBuf> {
    Ok(env::var_os("COUNTERSPELL_FEED_DIR")
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".factory-lanes").join("feed")))
}

fn event_envelope(event: &FeedEvent, now: DateTime<Utc>) -> Value {
    let occurred_at = now.to_rfc3339();
    let event_id = format!(
        "counterspell-{}-{}-{}",
        now.timestamp_millis(),
        sanitize(&event.session_id),
        sanitize(&event.action)
    );
    let idempotency_key = format!(
        "counterspell:{}:{}:{}:{}:{}:{}",
        event.session_id,
        event.action,
        event.from_model,
        event.to_model,
        event.gate,
        event.action_taken
    );

    json!({
        "schema_version": "weave.remote_event.v1",
        "id": event_id,
        "producer": {
            "name": "counterspell",
            "version": env!("CARGO_PKG_VERSION")
        },
        "produced_at": occurred_at,
        "occurred_at": occurred_at,
        "correlation_id": event.session_id,
        "source": {
            "kind": "harness-kit",
            "host": "counterspell.local",
            "external_id": event_id
        },
        "repository": {
            "id": "misty-step/counterspell",
            "full_name": "misty-step/counterspell",
            "default_branch": "main",
            "html_url": "https://github.com/misty-step/counterspell"
        },
        "subject": {
            "kind": "run",
            "id": event.session_id,
            "url": "https://github.com/misty-step/counterspell"
        },
        "actor": {
            "id": "counterspell",
            "login": "counterspell",
            "kind": "system"
        },
        "action": event.action,
        "idempotency_key": idempotency_key,
        "host_payload": {
            "event_name": "counterspell",
            "delivery_id": event_id,
            "links": [
                {
                    "rel": "html",
                    "href": "https://github.com/misty-step/counterspell"
                }
            ]
        },
        "payload": {
            "session_id": event.session_id,
            "pane": event.pane,
            "from_model": event.from_model,
            "to_model": event.to_model,
            "gate": event.gate,
            "action_taken": event.action_taken,
            "origin": event.origin
        }
    })
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect()
}
