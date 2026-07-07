use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::Config;
use crate::sessions::discover_recent_sessions;
use crate::util::normalize_path;

/// Matches the `source` herdr's own `claude` SessionStart hook
/// (`~/.claude/hooks/herdr-agent-state.sh`) reports itself as, so a rebind
/// looks identical to an ordinary hook-driven report on the herdr side.
const HERDR_REPORT_SOURCE: &str = "herdr:claude";

pub(crate) struct PaneEnv {
    pub(crate) pane_id: String,
    pub(crate) socket_path: PathBuf,
}

/// Reads the herdr-managed-pane environment (`HERDR_ENV`, `HERDR_PANE_ID`,
/// `HERDR_SOCKET_PATH`) that herdr exports into every pane it manages. A
/// clear error here is the whole point of `rebind` refusing to guess: sending
/// a report with a wrong or empty pane id would silently mis-bind a pane.
pub(crate) fn resolve_pane_env() -> Result<PaneEnv> {
    let herdr_env = env::var("HERDR_ENV").unwrap_or_default();
    if herdr_env != "1" {
        bail!(
            "HERDR_ENV is not \"1\" (found {herdr_env:?}); `counterspell rebind` must run inside \
             a herdr-managed pane"
        );
    }
    let pane_id = env::var("HERDR_PANE_ID").context(
        "HERDR_PANE_ID is not set; `counterspell rebind` must run inside a herdr-managed pane",
    )?;
    let socket_path = env::var_os("HERDR_SOCKET_PATH").context(
        "HERDR_SOCKET_PATH is not set; `counterspell rebind` must run inside a herdr-managed pane",
    )?;
    Ok(PaneEnv {
        pane_id,
        socket_path: PathBuf::from(socket_path),
    })
}

/// Resolves the session id (and transcript path, when known) a rebind should
/// report. Explicit overrides win outright; otherwise this reuses the same
/// transcript discovery `status`/`watch` already rely on
/// (`sessions::discover_recent_sessions`) and picks the most recent session
/// whose transcript `cwd` matches the caller's cwd — the newest *.jsonl for
/// this pane's project, not a reinvented scan.
pub(crate) fn resolve_target_session(
    config: &Config,
    session_id_override: Option<&str>,
    transcript_path_override: Option<&Path>,
    cwd: &Path,
    now: DateTime<Utc>,
) -> Result<(String, Option<String>)> {
    if let Some(session_id) = session_id_override {
        let transcript_path =
            transcript_path_override.map(|path| path.to_string_lossy().into_owned());
        return Ok((session_id.to_string(), transcript_path));
    }

    if let Some(path) = transcript_path_override {
        let session_id = session_id_from_transcript_path(path)?;
        return Ok((session_id, Some(path.to_string_lossy().into_owned())));
    }

    let sessions = discover_recent_sessions(config, now)
        .context("discover recent Claude transcript sessions")?;
    let normalized_cwd = normalize_path(cwd);
    let session = sessions
        .iter()
        .find(|session| session.cwd.as_deref().map(normalize_path) == Some(normalized_cwd.clone()))
        .with_context(|| {
            format!(
                "no recent Claude transcript session found for cwd {}; pass --session-id or \
                 --transcript-path",
                cwd.display()
            )
        })?;

    let transcript_path = config
        .projects_dir
        .join(&session.project)
        .join(format!("{}.jsonl", session.session_id));
    Ok((
        session.session_id.clone(),
        Some(transcript_path.to_string_lossy().into_owned()),
    ))
}

fn session_id_from_transcript_path(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
        .with_context(|| format!("transcript path {} has no file stem", path.display()))
}

/// Builds the `pane.report_agent_session` request, mirroring the shape and
/// field names `~/.claude/hooks/herdr-agent-state.sh` sends on every
/// SessionStart so a rebind reads identically to an ordinary hook report on
/// the herdr side.
pub(crate) fn build_report_request(
    pane_id: &str,
    session_id: &str,
    transcript_path: Option<&str>,
    seq: u64,
) -> Value {
    let mut params = serde_json::json!({
        "pane_id": pane_id,
        "source": HERDR_REPORT_SOURCE,
        "agent": "claude",
        "seq": seq,
        "agent_session_id": session_id,
    });
    if let Some(path) = transcript_path {
        params["agent_session_path"] = serde_json::json!(path);
    }
    serde_json::json!({
        "id": format!("{HERDR_REPORT_SOURCE}:{seq}"),
        "method": "pane.report_agent_session",
        "params": params,
    })
}

/// Sends one newline-delimited JSON request over herdr's unix socket and
/// best-effort reads one newline-delimited JSON response line back — the
/// same protocol `herdr-agent-state.sh` speaks. A missing or unparsable
/// response is not fatal (the hook script ignores it too); only a failed
/// connect or write is an error worth surfacing.
pub(crate) fn send_report_request(socket_path: &Path, request: &Value) -> Result<Option<Value>> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connect Herdr socket {}", socket_path.display()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .context("set Herdr socket write timeout")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .context("set Herdr socket read timeout")?;

    let mut payload =
        serde_json::to_vec(request).context("encode pane.report_agent_session request")?;
    payload.push(b'\n');
    stream
        .write_all(&payload)
        .context("write pane.report_agent_session request to Herdr socket")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => Ok(None),
        Ok(_) => {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            serde_json::from_str(trimmed)
                .map(Some)
                .context("parse Herdr socket response as JSON")
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error).context("read Herdr socket response"),
    }
}
