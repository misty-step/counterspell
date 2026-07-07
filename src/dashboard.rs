use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command as ProcessCommand;

use crate::cli::{Cli, UiArgs};
use crate::config::{
    add_target_to_config, config_path, ensure_config_file, load_config,
    remove_session_target_from_config, target_rule_from_parts,
};
use crate::defaults::DEFAULT_TARGET_MODEL;
use crate::herdr::{
    load_herdr_panes, load_herdr_tabs, load_herdr_workspaces, HerdrPane, HerdrTab, HerdrWorkspace,
};
use crate::indicators::watch_arm_daemon_status;
use crate::master;
use crate::model::{Config, TargetRule, TranscriptSession, WatchArmDaemonStatus};
use crate::remediation::{format_target_match, is_auto_fable_target, target_for_session};
use crate::sessions::discover_recent_sessions;
use crate::util::{home_dir, human_age, normalize_path, short_session};

pub(crate) use crate::dashboard_render::render_dashboard_html;

pub(crate) struct DashboardSnapshot {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) panes: Vec<ClaudePaneView>,
    pub(crate) summary: DashboardSummary,
    /// Global master switch: true means `watch --arm` is refusing to act
    /// regardless of drift or per-session targets. Rendered as a prominent
    /// banner above everything else on the page — this is a safety control,
    /// never ambiguous about which state it's in.
    pub(crate) master_disarmed: bool,
    /// The OTHER axis: is the watch-arm daemon actually installed and
    /// scheduled? A cleared flag with a `NotScheduled`/`NotInstalled` daemon
    /// means nothing will actually run — that combination must be visible on
    /// the banner, never silent.
    pub(crate) watch_arm_status: WatchArmDaemonStatus,
}

pub(crate) struct DashboardSummary {
    pub(crate) claude_panes: usize,
    pub(crate) enabled_panes: usize,
    pub(crate) enabled_sessions: usize,
    pub(crate) workspaces: usize,
}

pub(crate) struct ClaudePaneView {
    pub(crate) pane_id: String,
    pub(crate) pane_status: String,
    pub(crate) focused: bool,
    pub(crate) cwd: String,
    pub(crate) workspace_id: String,
    pub(crate) workspace_label: String,
    pub(crate) workspace_number: Option<u64>,
    pub(crate) tab_id: String,
    pub(crate) tab_label: String,
    pub(crate) tab_number: Option<u64>,
    pub(crate) title: Option<String>,
    pub(crate) custom_status: Option<String>,
    pub(crate) sessions: Vec<ClaudeSessionView>,
}

pub(crate) struct ClaudeSessionView {
    pub(crate) session_id: String,
    pub(crate) short_session_id: String,
    pub(crate) project: String,
    pub(crate) model: String,
    pub(crate) updated: String,
    pub(crate) enabled: bool,
    pub(crate) direct_target: bool,
    pub(crate) auto_target: bool,
    pub(crate) target: String,
}

struct Request {
    method: String,
    path: String,
    form: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
}

pub(crate) fn serve_dashboard(cli: &Cli, args: &UiArgs) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .with_context(|| format!("bind Counterspell dashboard port {}", args.port))?;
    let url = format!("http://{}", listener.local_addr()?);
    println!("counterspell ui: {url}");

    if !args.no_open {
        if let Err(error) = open_url(&url) {
            eprintln!("warning: could not open browser: {error:#}");
        }
    }

    for stream in listener.incoming() {
        handle_connection(stream.context("accept dashboard connection")?, cli)?;
        if args.once {
            break;
        }
    }

    Ok(())
}

pub(crate) fn load_dashboard_snapshot(cli: &Cli) -> Result<DashboardSnapshot> {
    let config = load_config(cli)?;
    let generated_at = Utc::now();
    let sessions = discover_recent_sessions(&config, generated_at)?;
    let panes = load_herdr_panes().context("load Herdr panes for dashboard")?;
    let workspaces = load_herdr_workspaces().context("load Herdr workspaces for dashboard")?;
    let tabs = load_all_tabs(&workspaces)?;
    let marker = master::marker_path(cli.disarm_marker.clone())?;
    let master_disarmed = master::is_disarmed(&marker);
    // Read-only status query (`launchctl print`) — never a mutation. This is
    // the only launchd interaction any GET route performs.
    let watch_arm_status = watch_arm_daemon_status(&home_dir()?)?;
    Ok(build_dashboard_snapshot(
        generated_at,
        &config,
        &sessions,
        &panes,
        &workspaces,
        &tabs,
        master_disarmed,
        watch_arm_status,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_dashboard_snapshot(
    generated_at: DateTime<Utc>,
    config: &Config,
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    workspaces: &[HerdrWorkspace],
    tabs: &[HerdrTab],
    master_disarmed: bool,
    watch_arm_status: WatchArmDaemonStatus,
) -> DashboardSnapshot {
    let workspaces_by_id = workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace))
        .collect::<BTreeMap<_, _>>();
    let tabs_by_id = tabs
        .iter()
        .map(|tab| (tab.tab_id.as_str(), tab))
        .collect::<BTreeMap<_, _>>();
    let direct_session_targets = direct_session_targets(&config.targets);

    let mut claude_panes = panes
        .iter()
        .filter(|pane| pane.agent.as_deref() == Some("claude"))
        .map(|pane| {
            let workspace = workspaces_by_id.get(pane.workspace_id.as_str()).copied();
            let tab = tabs_by_id.get(pane.tab_id.as_str()).copied();
            let pane_sessions = sessions
                .iter()
                .filter(|session| session_matches_pane(session, pane))
                .take(5)
                .map(|session| {
                    let target = target_for_session(session, config);
                    let enabled = target.is_some();
                    let auto_target = target.as_ref().is_some_and(is_auto_fable_target);
                    ClaudeSessionView {
                        session_id: session.session_id.clone(),
                        short_session_id: short_session(&session.session_id),
                        project: session.project.clone(),
                        model: session
                            .latest_model
                            .clone()
                            .unwrap_or_else(|| "-".to_string()),
                        updated: human_age(session.last_event_at, generated_at),
                        enabled,
                        direct_target: direct_session_targets.contains(&session.session_id),
                        auto_target,
                        target: target
                            .as_ref()
                            .map(format_target_match)
                            .unwrap_or_else(|| "disabled".to_string()),
                    }
                })
                .collect::<Vec<_>>();

            ClaudePaneView {
                pane_id: pane.pane_id.clone(),
                pane_status: pane
                    .agent_status
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                focused: pane.focused,
                cwd: pane
                    .cwd
                    .clone()
                    .or_else(|| pane.foreground_cwd.clone())
                    .unwrap_or_else(|| "-".to_string()),
                workspace_id: pane.workspace_id.clone(),
                workspace_label: workspace_label(workspace, &pane.workspace_id),
                workspace_number: workspace.and_then(|workspace| workspace.number),
                tab_id: pane.tab_id.clone(),
                tab_label: tab_label(tab, &pane.tab_id),
                tab_number: tab.and_then(|tab| tab.number),
                title: pane.title.clone(),
                custom_status: pane.custom_status.clone(),
                sessions: pane_sessions,
            }
        })
        .collect::<Vec<_>>();

    claude_panes.sort_by(|left, right| {
        left.workspace_number
            .cmp(&right.workspace_number)
            .then_with(|| left.workspace_label.cmp(&right.workspace_label))
            .then_with(|| left.tab_number.cmp(&right.tab_number))
            .then_with(|| left.tab_label.cmp(&right.tab_label))
            .then_with(|| left.pane_id.cmp(&right.pane_id))
    });

    let enabled_sessions = claude_panes
        .iter()
        .flat_map(|pane| pane.sessions.iter())
        .filter(|session| session.enabled)
        .map(|session| session.session_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let summary = DashboardSummary {
        claude_panes: claude_panes.len(),
        enabled_panes: claude_panes.iter().filter(|pane| pane.enabled()).count(),
        enabled_sessions,
        workspaces: claude_panes
            .iter()
            .map(|pane| pane.workspace_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
    };

    DashboardSnapshot {
        generated_at,
        panes: claude_panes,
        summary,
        master_disarmed,
        watch_arm_status,
    }
}

impl ClaudePaneView {
    pub(crate) fn enabled(&self) -> bool {
        self.sessions.iter().any(|session| session.enabled)
    }
}

fn load_all_tabs(workspaces: &[HerdrWorkspace]) -> Result<Vec<HerdrTab>> {
    let mut tabs = Vec::new();
    for workspace in workspaces {
        if workspace.workspace_id.is_empty() {
            continue;
        }
        tabs.extend(
            load_herdr_tabs(&workspace.workspace_id)
                .with_context(|| format!("load Herdr tabs for {}", workspace.workspace_id))?,
        );
    }
    Ok(tabs)
}

fn handle_connection(mut stream: TcpStream, cli: &Cli) -> Result<()> {
    let local_addr = stream
        .local_addr()
        .context("resolve dashboard local address")?;
    let read_stream = stream.try_clone().context("clone dashboard stream")?;
    handle_request(read_stream, &mut stream, cli, local_addr)
}

fn handle_request<R, W>(
    read_stream: R,
    stream: &mut W,
    cli: &Cli,
    local_addr: SocketAddr,
) -> Result<()>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(read_stream);
    let request = read_request(&mut reader)?;

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => {
            let snapshot = load_dashboard_snapshot(cli)?;
            respond(
                stream,
                "200 OK",
                "text/html; charset=utf-8",
                render_dashboard_html(&snapshot),
            )
        }
        ("GET", "/status.json") => {
            let snapshot = load_dashboard_snapshot(cli)?;
            respond(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                render_dashboard_json(&snapshot),
            )
        }
        ("POST", "/targets/enable") => {
            if !csrf_allowed(&request.headers, local_addr) {
                return respond_forbidden(stream);
            }
            enable_session_target(cli, &request.form)?;
            redirect_home(stream)
        }
        ("POST", "/targets/disable") => {
            if !csrf_allowed(&request.headers, local_addr) {
                return respond_forbidden(stream);
            }
            disable_session_target(cli, &request.form)?;
            redirect_home(stream)
        }
        ("POST", "/master/enable") => {
            if !csrf_allowed(&request.headers, local_addr) {
                return respond_forbidden(stream);
            }
            enable_master_switch(cli)?;
            redirect_home(stream)
        }
        ("POST", "/master/disable") => {
            if !csrf_allowed(&request.headers, local_addr) {
                return respond_forbidden(stream);
            }
            disable_master_switch(cli)?;
            redirect_home(stream)
        }
        ("GET", "/favicon.ico") => respond(stream, "204 No Content", "text/plain", String::new()),
        _ => respond(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    }
}

/// Reject cross-site form posts. The dashboard binds to loopback only, but
/// without this check any local page (or another browser tab) can silently
/// POST to /targets/enable|disable and rewrite ~/.counterspell/config.toml.
/// Browsers attach `Origin` (all modern browsers, on POST navigations) or at
/// minimum `Referer` for same-origin form submissions; neither present is
/// treated as untrusted.
fn csrf_allowed(headers: &BTreeMap<String, String>, local_addr: SocketAddr) -> bool {
    let allowed_origins = [
        format!("http://127.0.0.1:{}", local_addr.port()),
        format!("http://localhost:{}", local_addr.port()),
    ];

    if let Some(origin) = headers.get("origin") {
        return allowed_origins.iter().any(|allowed| origin == allowed);
    }

    if let Some(referer) = headers.get("referer") {
        return allowed_origins.iter().any(|allowed| {
            referer == allowed || referer.starts_with(format!("{allowed}/").as_str())
        });
    }

    false
}

fn respond_forbidden<W: Write>(stream: &mut W) -> Result<()> {
    respond(
        stream,
        "403 Forbidden",
        "text/plain; charset=utf-8",
        "forbidden: missing or mismatched Origin/Referer\n".to_string(),
    )
}

fn read_request<R: Read>(reader: &mut BufReader<R>) -> Result<Request> {
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("read dashboard request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("/");
    let path = raw_path.split('?').next().unwrap_or("/").to_string();
    let mut content_length = 0usize;
    let mut headers = BTreeMap::new();

    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .context("read dashboard header")?;
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse::<usize>().unwrap_or(0);
            }
            headers.insert(name, value);
        }
    }

    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .context("read dashboard request body")?;
    }

    Ok(Request {
        method,
        path,
        form: parse_form(&String::from_utf8_lossy(&body)),
        headers,
    })
}

fn enable_session_target(cli: &Cli, form: &BTreeMap<String, String>) -> Result<()> {
    let session_id = form_value(form, "session_id")?;
    let home = home_dir()?;
    let path = config_path(cli.config.clone(), &home);
    ensure_config_file(&path)?;
    let target = target_rule_from_parts(
        Some(session_id.to_string()),
        None,
        None,
        DEFAULT_TARGET_MODEL.to_string(),
    )?;
    add_target_to_config(&path, &target)?;
    Ok(())
}

fn disable_session_target(cli: &Cli, form: &BTreeMap<String, String>) -> Result<()> {
    let session_id = form_value(form, "session_id")?;
    let home = home_dir()?;
    let path = config_path(cli.config.clone(), &home);
    remove_session_target_from_config(&path, session_id)?;
    Ok(())
}

/// Flag-only: flips the same marker file the CLI `enable`/`disable`
/// commands use (never diverges on the actual gate state), but — unlike
/// the CLI's `enable` — never touches launchd. See
/// `crate::master::enable_flag_only` for why a browser-triggered route
/// must not be able to reach `launchctl`.
fn enable_master_switch(cli: &Cli) -> Result<()> {
    let marker = master::marker_path(cli.disarm_marker.clone())?;
    master::enable_flag_only(&marker)?;
    Ok(())
}

/// Same code path as the CLI `counterspell disable` command.
fn disable_master_switch(cli: &Cli) -> Result<()> {
    let marker = master::marker_path(cli.disarm_marker.clone())?;
    master::disable(&marker)?;
    Ok(())
}

fn form_value<'a>(form: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    form.get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing form field {key}"))
}

fn respond<W: Write>(stream: &mut W, status: &str, content_type: &str, body: String) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .context("write dashboard response")
}

fn redirect_home<W: Write>(stream: &mut W) -> Result<()> {
    let response =
        "HTTP/1.1 303 See Other\r\nLocation: /\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream
        .write_all(response.as_bytes())
        .context("write dashboard redirect")
}

fn render_dashboard_json(snapshot: &DashboardSnapshot) -> String {
    let panes = snapshot
        .panes
        .iter()
        .map(|pane| {
            json!({
                "pane_id": pane.pane_id,
                "workspace": {
                    "id": pane.workspace_id,
                    "label": pane.workspace_label,
                    "number": pane.workspace_number,
                },
                "tab": {
                    "id": pane.tab_id,
                    "label": pane.tab_label,
                    "number": pane.tab_number,
                },
                "cwd": pane.cwd,
                "status": pane.pane_status,
                "enabled": pane.enabled(),
                "sessions": pane.sessions.iter().map(|session| {
                    json!({
                        "session_id": session.session_id,
                        "short_session_id": session.short_session_id,
                        "project": session.project,
                        "model": session.model,
                        "updated": session.updated,
                        "enabled": session.enabled,
                        "direct_target": session.direct_target,
                        "auto_target": session.auto_target,
                        "target": session.target,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "generated_at": snapshot.generated_at.to_rfc3339(),
        "master_disarmed": snapshot.master_disarmed,
        "watch_arm_daemon": snapshot.watch_arm_status.label(),
        "summary": {
            "claude_panes": snapshot.summary.claude_panes,
            "enabled_panes": snapshot.summary.enabled_panes,
            "enabled_sessions": snapshot.summary.enabled_sessions,
            "workspaces": snapshot.summary.workspaces,
        },
        "panes": panes,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

fn session_matches_pane(session: &TranscriptSession, pane: &HerdrPane) -> bool {
    let Some(session_cwd) = session.cwd.as_deref() else {
        return false;
    };
    let normalized_session_cwd = normalize_path(session_cwd);
    [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
        .into_iter()
        .flatten()
        .any(|pane_cwd| normalize_path(pane_cwd) == normalized_session_cwd)
}

fn direct_session_targets(targets: &[TargetRule]) -> BTreeSet<String> {
    targets
        .iter()
        .filter_map(|target| target.session_id.clone())
        .collect()
}

fn workspace_label(workspace: Option<&HerdrWorkspace>, workspace_id: &str) -> String {
    workspace
        .and_then(|workspace| workspace.label.clone())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| workspace_id.to_string())
}

fn tab_label(tab: Option<&HerdrTab>, tab_id: &str) -> String {
    tab.and_then(|tab| tab.label.clone())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| tab_id.to_string())
}

fn parse_form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        decoded.push(byte);
                        index += 3;
                        continue;
                    }
                }
                decoded.push(bytes[index]);
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn open_url(url: &str) -> Result<()> {
    let status = ProcessCommand::new("open")
        .arg(url)
        .status()
        .context("run open for dashboard URL")?;
    if !status.success() {
        bail!("open exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    const TEST_PORT: u16 = 18765;

    fn write_config(temp_path: &Path, contents: &str) -> PathBuf {
        let path = temp_path.join("counterspell.toml");
        std::fs::write(&path, contents).expect("config");
        path
    }

    fn send_dashboard_request(
        config: &Path,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> String {
        send_dashboard_request_with_marker(config, None, method, path, headers, body)
    }

    fn send_dashboard_request_with_marker(
        config: &Path,
        marker: Option<&Path>,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> String {
        let mut cli = crate::cli::test_cli_with_config(config.to_path_buf());
        cli.disarm_marker = marker.map(Path::to_path_buf);
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{TEST_PORT}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("Connection: close\r\n\r\n");
        request.push_str(body);

        let mut response = Vec::new();
        handle_request(
            Cursor::new(request.into_bytes()),
            &mut response,
            &cli,
            SocketAddr::from(([127, 0, 0, 1], TEST_PORT)),
        )
        .expect("handle dashboard request");
        String::from_utf8(response).expect("dashboard response")
    }

    #[test]
    fn dashboard_rejects_target_enable_without_origin_or_referer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");

        let response = send_dashboard_request(
            &config,
            "POST",
            "/targets/enable",
            &[],
            "session_id=csrf-reject-session",
        );

        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
        assert!(
            !std::fs::read_to_string(&config)
                .expect("config")
                .contains("csrf-reject-session"),
            "target must not be written when Origin/Referer is missing"
        );
    }

    #[test]
    fn dashboard_rejects_target_enable_with_mismatched_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");

        let response = send_dashboard_request(
            &config,
            "POST",
            "/targets/enable",
            &[("Origin", "http://evil.example")],
            "session_id=csrf-reject-session",
        );

        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
        assert!(
            !std::fs::read_to_string(&config)
                .expect("config")
                .contains("csrf-reject-session"),
            "target must not be written when Origin does not match the dashboard's own origin"
        );
    }

    #[test]
    fn dashboard_accepts_target_enable_with_matching_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");
        let origin = format!("http://127.0.0.1:{TEST_PORT}");

        let response = send_dashboard_request(
            &config,
            "POST",
            "/targets/enable",
            &[("Origin", &origin)],
            "session_id=csrf-accept-session",
        );

        assert!(
            response.starts_with("HTTP/1.1 303"),
            "expected 303 redirect, got: {response}"
        );
        assert!(
            std::fs::read_to_string(&config)
                .expect("config")
                .contains("csrf-accept-session"),
            "target must be written when Origin matches the dashboard's own origin"
        );
    }

    #[test]
    fn dashboard_rejects_target_disable_without_origin_or_referer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(
            temp.path(),
            r#"
[[targets]]
session_id = "csrf-disable-session"
target_model = "claude-fable-5"
"#,
        );

        let response = send_dashboard_request(
            &config,
            "POST",
            "/targets/disable",
            &[],
            "session_id=csrf-disable-session",
        );

        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
        assert!(
            std::fs::read_to_string(&config)
                .expect("config")
                .contains("csrf-disable-session"),
            "target must not be removed when Origin/Referer is missing"
        );
    }

    #[test]
    fn dashboard_rejects_master_disable_without_origin_or_referer() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");
        let marker = temp.path().join("disarmed");

        let response = send_dashboard_request_with_marker(
            &config,
            Some(&marker),
            "POST",
            "/master/disable",
            &[],
            "",
        );

        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
        assert!(
            !marker.exists(),
            "master switch must not flip when Origin/Referer is missing"
        );
    }

    #[test]
    fn dashboard_rejects_master_enable_with_mismatched_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");
        let marker = temp.path().join("disarmed");
        std::fs::write(&marker, "disarmed").expect("seed marker");

        let response = send_dashboard_request_with_marker(
            &config,
            Some(&marker),
            "POST",
            "/master/enable",
            &[("Origin", "http://evil.example")],
            "",
        );

        assert!(
            response.starts_with("HTTP/1.1 403"),
            "expected 403, got: {response}"
        );
        assert!(
            marker.exists(),
            "master switch must stay disabled when Origin does not match"
        );
    }

    #[test]
    fn dashboard_master_disable_then_enable_round_trips_with_matching_origin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = write_config(temp.path(), "");
        let marker = temp.path().join("disarmed");
        let origin = format!("http://127.0.0.1:{TEST_PORT}");

        assert!(!marker.exists(), "starts armed (no marker) in this test");

        let response = send_dashboard_request_with_marker(
            &config,
            Some(&marker),
            "POST",
            "/master/disable",
            &[("Origin", &origin)],
            "",
        );
        assert!(
            response.starts_with("HTTP/1.1 303"),
            "expected 303 redirect, got: {response}"
        );
        assert!(marker.exists(), "disable must create the marker file");

        // Dashboard enable is flag-only (see enable_flag_only) — it never
        // touches launchd, so this needs no HOME/plist isolation at all.
        let response = send_dashboard_request_with_marker(
            &config,
            Some(&marker),
            "POST",
            "/master/enable",
            &[("Origin", &origin)],
            "",
        );
        assert!(
            response.starts_with("HTTP/1.1 303"),
            "expected 303 redirect, got: {response}"
        );
        assert!(!marker.exists(), "enable must remove the marker file");
    }
}
