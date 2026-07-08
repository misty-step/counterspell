use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::model::{Config, TranscriptSession};
use crate::util::{parse_rfc3339_utc, project_label, system_time_to_utc};

pub(crate) fn discover_recent_sessions(
    config: &Config,
    now: DateTime<Utc>,
) -> Result<Vec<TranscriptSession>> {
    let cutoff = now - Duration::hours(config.recent_hours as i64);
    let mut sessions = Vec::new();

    if !config.projects_dir.exists() {
        return Ok(sessions);
    }

    for project_entry in fs::read_dir(&config.projects_dir)
        .with_context(|| format!("read projects dir {}", config.projects_dir.display()))?
    {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project = project_label(&project_path);

        for session_entry in fs::read_dir(&project_path)
            .with_context(|| format!("read project dir {}", project_path.display()))?
        {
            let session_entry = session_entry?;
            let path = session_entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let metadata = session_entry
                .metadata()
                .with_context(|| format!("read metadata {}", path.display()))?;
            let modified_at = system_time_to_utc(metadata.modified()?);
            if modified_at < cutoff {
                continue;
            }

            match parse_transcript_file(&path, project.clone(), modified_at) {
                Ok(session) => sessions.push(session),
                Err(error) => eprintln!("warning: skipped {}: {error:#}", path.display()),
            }
        }
    }

    sessions.sort_by(|left, right| {
        right
            .last_event_at
            .cmp(&left.last_event_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(sessions)
}

pub(crate) fn parse_transcript_file(
    path: &Path,
    project: String,
    file_modified_at: DateTime<Utc>,
) -> Result<TranscriptSession> {
    let file = File::open(path).with_context(|| format!("open transcript {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut cwd = None;
    let mut last_event_at = None;
    let mut latest_model = None;
    let mut latest_model_at = None;
    let mut latest_compact_at = None;
    let mut model_history = Vec::new();

    for line in reader.lines() {
        let line = line.with_context(|| format!("read transcript {}", path.display()))?;
        let value: Value =
            serde_json::from_str(&line).with_context(|| format!("parse {}", path.display()))?;

        if let Some(value_session_id) = value.get("sessionId").and_then(Value::as_str) {
            session_id = value_session_id.to_string();
        }
        if let Some(value_cwd) = value.get("cwd").and_then(Value::as_str) {
            cwd = Some(value_cwd.to_string());
        }
        let event_at = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc);
        if let Some(timestamp) = event_at {
            last_event_at = Some(timestamp);
        }
        if let Some(model) = transcript_model(&value) {
            if model_history.last() != Some(&model) {
                model_history.push(model.clone());
            }
            latest_model_at = event_at.or(last_event_at);
            latest_model = Some(model);
        }
        if is_compact_summary(&value) {
            latest_compact_at = event_at.or(last_event_at);
        }
    }

    Ok(TranscriptSession {
        session_id,
        project,
        cwd,
        last_event_at: last_event_at.unwrap_or(file_modified_at),
        latest_model,
        latest_model_at,
        latest_compact_at,
        model_history,
    })
}

fn transcript_model(value: &Value) -> Option<String> {
    value
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/message/model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_model_sentinel(value))
        .map(str::to_string)
}

pub(crate) fn is_model_sentinel(model: &str) -> bool {
    let model = model.trim();
    model.len() >= 2 && model.starts_with('<') && model.ends_with('>')
}

fn is_compact_summary(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("summary")
        || value.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
        || value.get("summary").is_some()
        || value.get("compactMetadata").is_some()
        || value
            .get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || value.pointer("/message/type").and_then(Value::as_str) == Some("summary")
        || value.pointer("/message/summary").is_some()
}
