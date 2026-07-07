use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::util::normalize_path;

const DEFAULT_HERDR_TIMEOUT: Duration = Duration::from_secs(10);
const HERDR_POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Deserialize)]
struct HerdrPaneList {
    result: HerdrPaneListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrWorkspaceList {
    result: HerdrWorkspaceListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrTabList {
    result: HerdrTabListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrPaneListResult {
    #[serde(default)]
    panes: Vec<HerdrPane>,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrWorkspaceListResult {
    #[serde(default)]
    workspaces: Vec<HerdrWorkspace>,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrTabListResult {
    #[serde(default)]
    tabs: Vec<HerdrTab>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrPane {
    #[serde(default)]
    pub(crate) pane_id: String,
    #[serde(default)]
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) tab_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) foreground_cwd: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) agent_status: Option<String>,
    #[serde(default)]
    pub(crate) focused: bool,
    pub(crate) title: Option<String>,
    pub(crate) custom_status: Option<String>,
    pub(crate) agent_session: Option<HerdrAgentSession>,
}

/// Durable session identity an agent reported for its pane via Herdr's
/// `pane.report_agent_session` (installed by `herdr integration install
/// claude` as a SessionStart hook).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrAgentSession {
    pub(crate) kind: Option<String>,
    pub(crate) value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrWorkspace {
    #[serde(default)]
    pub(crate) workspace_id: String,
    pub(crate) label: Option<String>,
    pub(crate) number: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrTab {
    #[serde(default)]
    pub(crate) tab_id: String,
    pub(crate) label: Option<String>,
    pub(crate) number: Option<u64>,
}

pub(crate) fn load_herdr_panes() -> Result<Vec<HerdrPane>> {
    let output = run_herdr_args(&["pane", "list"])?;
    parse_herdr_panes(&output.stdout)
}

pub(crate) fn load_herdr_workspaces() -> Result<Vec<HerdrWorkspace>> {
    let output = run_herdr_args(&["workspace", "list"])?;
    parse_herdr_workspaces(&output.stdout)
}

pub(crate) fn load_herdr_tabs(workspace_id: &str) -> Result<Vec<HerdrTab>> {
    let output = run_herdr_args(&["tab", "list", "--workspace", workspace_id])?;
    parse_herdr_tabs(&output.stdout)
}

pub(crate) fn run_herdr_args(args: &[&str]) -> Result<std::process::Output> {
    run_herdr_args_with_timeout(args, herdr_timeout())
}

/// `herdr wait ...` legitimately blocks for as long as its own `--timeout`
/// asks — the caller must size the subprocess timeout above it. The default
/// 10s kill silently truncated a 20s interrupt wait on 2026-07-04 and would
/// truncate every 180s compact wait.
pub(crate) fn run_herdr_args_with_timeout(
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output> {
    let herdr_bin =
        env::var_os("COUNTERSPELL_HERDR_BIN").unwrap_or_else(|| OsString::from("herdr"));
    let mut child = ProcessCommand::new(&herdr_bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "spawn {}; Herdr must be installed and running for pane discovery/injection",
                PathBuf::from(&herdr_bin).display()
            )
        })?;

    // Drain stdout/stderr on background threads so a chatty herdr can't fill
    // its pipe buffer and deadlock while we poll for exit below.
    let stdout_reader = spawn_pipe_reader(child.stdout.take());
    let stderr_reader = spawn_pipe_reader(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("poll {}", PathBuf::from(&herdr_bin).display()))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} {:?} timed out after {:?}; Herdr must be installed, running, and responsive",
                PathBuf::from(&herdr_bin).display(),
                args,
                timeout
            );
        }
        thread::sleep(HERDR_POLL_INTERVAL);
    };

    let output = std::process::Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    };

    if !output.status.success() {
        bail!(
            "{} {:?} exited with {}; Herdr must be running and reachable",
            PathBuf::from(&herdr_bin).display(),
            args,
            output.status
        );
    }

    Ok(output)
}

fn herdr_timeout() -> Duration {
    env::var("COUNTERSPELL_HERDR_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_HERDR_TIMEOUT)
}

fn spawn_pipe_reader<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

pub(crate) fn annotate_herdr_pane(pane_id: &str, title: &str, status: &str) -> Result<()> {
    run_herdr_args(&[
        "pane",
        "report-metadata",
        pane_id,
        "--source",
        "counterspell",
        "--title",
        title,
        "--custom-status",
        status,
        "--ttl-ms",
        "300000",
    ])
    .with_context(|| format!("annotate Herdr pane {pane_id}"))?;
    Ok(())
}

/// Session id a pane's agent has reported, when present. `kind == "id"`
/// carries the id directly; `kind == "path"` carries a transcript path whose
/// file stem is the session id.
pub(crate) fn pane_session_id(pane: &HerdrPane) -> Option<&str> {
    let session = pane.agent_session.as_ref()?;
    let value = session.value.as_deref()?;
    if value.is_empty() {
        return None;
    }
    match session.kind.as_deref() {
        Some("path") => std::path::Path::new(value)
            .file_stem()
            .and_then(|stem| stem.to_str()),
        _ => Some(value),
    }
}

/// True when at least one live `claude` pane exists but *none* of them has
/// ever reported an `agent_session` — the signature of the SessionStart hook
/// (`herdr integration install claude`) being unwired from `settings.json`.
/// When this holds, `matching_panes_for_session` degrades to cwd-only
/// fallback for every session sharing a project directory, so multi-pane
/// projects gate on `ambiguous-pane` forever and remediation never fires
/// (root cause of the 2026-07-07 unremediated drift: a settings.json rewrite
/// during the harness-kit->roster hook cutover dropped the herdr SessionStart
/// entry, and every subsequently started pane lost session reporting).
pub(crate) fn session_reporting_broken(panes: &[HerdrPane]) -> bool {
    let mut claude_panes = panes
        .iter()
        .filter(|pane| pane.agent.as_deref() == Some("claude"))
        .peekable();
    claude_panes.peek().is_some() && claude_panes.all(|pane| pane_session_id(pane).is_none())
}

/// Binds a transcript session to Herdr panes. A pane whose agent reported
/// this exact session id is authoritative. Only when no pane anywhere claims
/// the session do we fall back to cwd matching, and even then panes bound to
/// a *different* session are excluded — two live agents in the same cwd must
/// never be disambiguated by focus or guesswork (that injected keystrokes
/// into the wrong session on 2026-07-04).
pub(crate) fn matching_panes_for_session<'a>(
    session_id: &str,
    cwd: Option<&str>,
    panes: &'a [HerdrPane],
) -> Vec<&'a HerdrPane> {
    let mut exact = panes
        .iter()
        .filter(|pane| pane_session_id(pane) == Some(session_id))
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        exact.sort_by(|left, right| pane_id(left).cmp(pane_id(right)));
        return exact;
    }
    let Some(cwd) = cwd else {
        return Vec::new();
    };
    matching_panes_for_cwd(cwd, panes)
        .into_iter()
        .filter(|pane| pane_session_id(pane).is_none())
        .collect()
}

pub(crate) fn matching_panes_for_cwd<'a>(cwd: &str, panes: &'a [HerdrPane]) -> Vec<&'a HerdrPane> {
    let normalized_cwd = normalize_path(cwd);
    let mut matches = panes
        .iter()
        .filter(|pane| pane.agent.as_deref() == Some("claude"))
        .filter(|pane| pane_cwds(pane).any(|pane_cwd| normalize_path(pane_cwd) == normalized_cwd))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| pane_id(left).cmp(pane_id(right)));
    matches
}

pub(crate) fn pane_id(pane: &HerdrPane) -> &str {
    if pane.pane_id.is_empty() {
        "unknown"
    } else {
        pane.pane_id.as_str()
    }
}

fn parse_herdr_panes(bytes: &[u8]) -> Result<Vec<HerdrPane>> {
    let pane_list: HerdrPaneList =
        serde_json::from_slice(bytes).context("parse `herdr pane list` JSON")?;
    Ok(pane_list.result.panes)
}

fn parse_herdr_workspaces(bytes: &[u8]) -> Result<Vec<HerdrWorkspace>> {
    let workspace_list: HerdrWorkspaceList =
        serde_json::from_slice(bytes).context("parse `herdr workspace list` JSON")?;
    Ok(workspace_list.result.workspaces)
}

fn parse_herdr_tabs(bytes: &[u8]) -> Result<Vec<HerdrTab>> {
    let tab_list: HerdrTabList =
        serde_json::from_slice(bytes).context("parse `herdr tab list` JSON")?;
    Ok(tab_list.result.tabs)
}

fn pane_cwds(pane: &HerdrPane) -> impl Iterator<Item = &str> {
    [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
        .into_iter()
        .flatten()
}
