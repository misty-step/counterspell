use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

pub(crate) fn parse_env_u64(key: &str) -> Option<u64> {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

pub(crate) fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub(crate) fn system_time_to_utc(value: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(value)
}

pub(crate) fn project_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-project")
        .to_string()
}

pub(crate) fn normalize_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalized.to_string_lossy().into_owned()
}

pub(crate) fn unix_to_utc(value: u64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value as i64, 0)
}

pub(crate) fn join_or_dash<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut unique = values
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    unique.sort();
    unique.dedup();

    if unique.is_empty() {
        "-".to_string()
    } else {
        unique.join(",")
    }
}

pub(crate) fn short_session(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

pub(crate) fn human_age(value: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = now - value;
    if age.num_seconds() < 60 {
        return format!("{}s ago", age.num_seconds().max(0));
    }
    if age.num_minutes() < 60 {
        return format!("{}m ago", age.num_minutes());
    }
    if age.num_hours() < 48 {
        return format!("{}h ago", age.num_hours());
    }
    format!("{}d ago", age.num_days())
}

pub(crate) fn shell_param_default(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

pub(crate) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
