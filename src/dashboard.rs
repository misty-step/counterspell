use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
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
use crate::model::{Config, TargetRule, TranscriptSession};
use crate::remediation::{format_target_match, target_for_session};
use crate::sessions::discover_recent_sessions;
use crate::util::{home_dir, html_escape, human_age, normalize_path, short_session};

pub(crate) struct DashboardSnapshot {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) panes: Vec<ClaudePaneView>,
    pub(crate) summary: DashboardSummary,
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
    pub(crate) target: String,
}

struct Request {
    method: String,
    path: String,
    form: BTreeMap<String, String>,
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
    Ok(build_dashboard_snapshot(
        generated_at,
        &config,
        &sessions,
        &panes,
        &workspaces,
        &tabs,
    ))
}

pub(crate) fn build_dashboard_snapshot(
    generated_at: DateTime<Utc>,
    config: &Config,
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    workspaces: &[HerdrWorkspace],
    tabs: &[HerdrTab],
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
    }
}

pub(crate) fn render_dashboard_html(snapshot: &DashboardSnapshot) -> String {
    let panes = if snapshot.panes.is_empty() {
        r#"<div class="empty">No live Claude Code panes found in Herdr.</div>"#.to_string()
    } else {
        let mut output = String::new();
        let mut current_workspace = None::<&str>;
        for pane in &snapshot.panes {
            if current_workspace != Some(pane.workspace_id.as_str()) {
                if current_workspace.is_some() {
                    output.push_str("</section>");
                }
                current_workspace = Some(&pane.workspace_id);
                output.push_str(&format!(
                    r#"<section class="workspace"><div class="workspace-head"><div><span class="eyebrow">Workspace {}</span><h2>{}</h2></div><span class="count">{}</span></div>"#,
                    workspace_number(pane),
                    html_escape(&pane.workspace_label),
                    html_escape(&pane.workspace_id)
                ));
            }
            output.push_str(&render_pane(pane));
        }
        output.push_str("</section>");
        output
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="refresh" content="10">
  <title>Counterspell</title>
  <style>
    :root {{
      color-scheme: light;
      --bg: #f5f7f8;
      --panel: #ffffff;
      --ink: #172026;
      --muted: #63707d;
      --line: #d8dee5;
      --soft: #eef3f6;
      --green: #1d7f56;
      --red: #b33939;
      --amber: #986d18;
      --blue: #245e9a;
      --shadow: 0 10px 28px rgba(23, 32, 38, .08);
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      background: var(--bg);
      color: var(--ink);
      font: 14px/1.42 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      overflow-x: hidden;
    }}
    main {{ width: min(1180px, calc(100vw - 40px)); margin: 24px auto 36px; }}
    header {{
      display: flex;
      align-items: flex-start;
      justify-content: space-between;
      gap: 24px;
      padding-bottom: 16px;
      border-bottom: 1px solid var(--line);
    }}
    .brand {{ display: flex; gap: 12px; align-items: center; min-width: 0; }}
    .mark {{
      width: 38px;
      height: 38px;
      border: 1px solid var(--ink);
      border-radius: 8px;
      display: grid;
      place-items: center;
      background: var(--panel);
      box-shadow: var(--shadow);
      flex: 0 0 auto;
    }}
    h1 {{ margin: 0; font-size: 24px; line-height: 1.05; letter-spacing: 0; }}
    h2 {{ margin: 2px 0 0; font-size: 17px; line-height: 1.15; letter-spacing: 0; }}
    .subtitle {{ margin-top: 5px; color: var(--muted); overflow-wrap: anywhere; }}
    .summary {{ display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end; }}
    .metric {{
      min-width: 108px;
      padding: 8px 10px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
    }}
    .metric span {{ display: block; color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0; }}
    .metric strong {{ display: block; margin-top: 3px; font-size: 19px; line-height: 1; letter-spacing: 0; }}
    .workspace {{ margin-top: 22px; }}
    .workspace-head {{
      display: flex;
      align-items: flex-end;
      justify-content: space-between;
      gap: 16px;
      margin-bottom: 8px;
    }}
    .eyebrow {{ color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0; }}
    .count {{ color: var(--muted); font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }}
    .pane {{
      display: grid;
      grid-template-columns: minmax(240px, 300px) minmax(0, 1fr);
      gap: 14px;
      align-items: stretch;
      padding: 12px;
      margin-top: 8px;
      border: 1px solid var(--line);
      border-radius: 8px;
      background: var(--panel);
      box-shadow: 0 1px 2px rgba(23, 32, 38, .06);
      overflow: hidden;
    }}
    .pane-main {{ min-width: 0; }}
    .pane-title {{ display: flex; align-items: center; gap: 8px; min-width: 0; flex-wrap: wrap; }}
    .pane-title strong {{ font-size: 15px; }}
    .mono {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace; overflow-wrap: anywhere; }}
    .path {{ margin-top: 7px; color: var(--muted); font-size: 12px; }}
    .meta {{ margin-top: 8px; display: flex; gap: 6px; flex-wrap: wrap; }}
    .sessions {{ min-width: 0; }}
    .session-head {{
      display: grid;
      grid-template-columns: 84px minmax(0, 1fr) 128px;
      gap: 10px;
      padding: 0 0 6px;
      color: var(--muted);
      font-size: 11px;
      text-transform: uppercase;
      letter-spacing: 0;
      border-bottom: 1px solid var(--line);
    }}
    .session {{
      display: grid;
      grid-template-columns: 84px minmax(0, 1fr) 128px;
      gap: 10px;
      align-items: center;
      padding: 9px 0;
      border-bottom: 1px solid var(--line);
    }}
    .session:last-child {{ border-bottom: 0; }}
    .session-id {{ font-size: 13px; font-weight: 700; color: var(--ink); }}
    .session-detail {{ min-width: 0; color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }}
    .session-detail strong {{ color: var(--ink); font-weight: 600; }}
    .session-subline {{ margin-top: 2px; }}
    .session-action {{ display: flex; justify-content: flex-end; align-items: center; gap: 8px; }}
    .chip {{
      display: inline-flex;
      align-items: center;
      min-height: 23px;
      padding: 2px 8px;
      border-radius: 999px;
      border: 1px solid var(--line);
      background: #f8fafb;
      white-space: nowrap;
      font-size: 12px;
      max-width: 100%;
      overflow: hidden;
      text-overflow: ellipsis;
    }}
    .chip.ok {{ color: var(--green); border-color: rgba(29,127,86,.28); background: rgba(29,127,86,.08); }}
    .chip.warn {{ color: var(--amber); border-color: rgba(152,109,24,.28); background: rgba(152,109,24,.10); }}
    .chip.blocked {{ color: var(--red); border-color: rgba(179,57,57,.28); background: rgba(179,57,57,.08); }}
    .chip.info {{ color: var(--blue); border-color: rgba(36,94,154,.26); background: rgba(36,94,154,.08); }}
    form {{ margin: 0; }}
    button {{
      min-width: 80px;
      min-height: 34px;
      border-radius: 8px;
      border: 1px solid var(--line);
      background: var(--ink);
      color: #fff;
      font: inherit;
      cursor: pointer;
      transition-property: transform, background-color, border-color;
      transition-duration: 140ms;
      transition-timing-function: cubic-bezier(.2, 0, 0, 1);
    }}
    button:active {{ transform: scale(.96); }}
    button:focus-visible {{ outline: 2px solid var(--blue); outline-offset: 2px; }}
    button.off {{ background: #fff; color: var(--red); border-color: rgba(179,57,57,.35); }}
    button:disabled {{ cursor: default; color: var(--muted); background: var(--soft); }}
    .empty {{ margin-top: 20px; padding: 20px; border: 1px solid var(--line); border-radius: 8px; color: var(--muted); background: var(--panel); }}
    footer {{ margin-top: 14px; color: var(--muted); font-size: 12px; display: flex; justify-content: space-between; gap: 16px; flex-wrap: wrap; }}
    @media (max-width: 820px) {{
      main {{ width: min(100%, calc(100vw - 24px)); margin-top: 18px; }}
      header {{ display: block; }}
      .summary {{ display: grid !important; grid-template-columns: repeat(2, minmax(0, 1fr)); justify-content: stretch; width: 100%; max-width: 100%; margin-top: 14px; }}
      .metric {{ width: auto; min-width: 0; padding: 8px; }}
      .metric span {{ font-size: 10px; overflow-wrap: anywhere; }}
      .counterspell-chip {{ display: none; }}
      .pane {{ grid-template-columns: 1fr; }}
      .session-head {{ display: none; }}
      .session {{ grid-template-columns: 76px minmax(0, 1fr); align-items: start; }}
      .session-action {{ grid-column: 1 / -1; justify-content: flex-start; }}
      button {{ min-width: 96px; }}
    }}
    @media (prefers-reduced-motion: reduce) {{
      button {{ transition-duration: 0ms; }}
      button:active {{ transform: none; }}
    }}
  </style>
</head>
<body>
  <main>
    <header>
      <div class="brand">
        <div class="mark" aria-hidden="true">{}</div>
        <div>
          <h1>Counterspell</h1>
          <div class="subtitle">Herdr Claude Code panes</div>
        </div>
      </div>
      <div class="summary" aria-label="Counterspell summary">
        <div class="metric"><span>Claude panes</span><strong>{}</strong></div>
        <div class="metric"><span>Enabled panes</span><strong>{}</strong></div>
        <div class="metric"><span>Enabled sessions</span><strong>{}</strong></div>
      </div>
    </header>

    {}

    <footer>
      <span>{} workspace(s)</span>
      <span>Generated: {}</span>
    </footer>
  </main>
</body>
</html>
"#,
        scroll_text_icon(),
        snapshot.summary.claude_panes,
        snapshot.summary.enabled_panes,
        snapshot.summary.enabled_sessions,
        panes,
        snapshot.summary.workspaces,
        snapshot.generated_at.to_rfc3339()
    )
}

impl ClaudePaneView {
    fn enabled(&self) -> bool {
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
    let mut reader = BufReader::new(stream.try_clone().context("clone dashboard stream")?);
    let request = read_request(&mut reader)?;

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => {
            let snapshot = load_dashboard_snapshot(cli)?;
            respond(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                render_dashboard_html(&snapshot),
            )
        }
        ("GET", "/status.json") => {
            let snapshot = load_dashboard_snapshot(cli)?;
            respond(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                render_dashboard_json(&snapshot),
            )
        }
        ("POST", "/targets/enable") => {
            enable_session_target(cli, &request.form)?;
            redirect_home(&mut stream)
        }
        ("POST", "/targets/disable") => {
            disable_session_target(cli, &request.form)?;
            redirect_home(&mut stream)
        }
        ("GET", "/favicon.ico") => {
            respond(&mut stream, "204 No Content", "text/plain", String::new())
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Result<Request> {
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("read dashboard request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let raw_path = parts.next().unwrap_or("/");
    let path = raw_path.split('?').next().unwrap_or("/").to_string();
    let mut content_length = 0usize;

    loop {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .context("read dashboard header")?;
        let header = header.trim();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().unwrap_or(0);
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

fn form_value<'a>(form: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    form.get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing form field {key}"))
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: String) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .context("write dashboard response")
}

fn redirect_home(stream: &mut TcpStream) -> Result<()> {
    let response =
        "HTTP/1.1 303 See Other\r\nLocation: /\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream
        .write_all(response.as_bytes())
        .context("write dashboard redirect")
}

fn render_pane(pane: &ClaudePaneView) -> String {
    let sessions = if pane.sessions.is_empty() {
        r#"<div class="empty">No recent transcript session mapped to this pane cwd.</div>"#
            .to_string()
    } else {
        format!(
            r#"<div class="session-head"><span>Session</span><span>Latest match</span><span>Counterspell</span></div>{}"#,
            pane.sessions
                .iter()
                .map(render_session)
                .collect::<Vec<_>>()
                .join("")
        )
    };
    let enabled_count = pane
        .sessions
        .iter()
        .filter(|session| session.enabled)
        .count();
    let session_total = pane.sessions.len();
    let enabled_class = if pane.enabled() { "ok" } else { "warn" };
    let enabled_label = format!("{enabled_count}/{session_total} enabled");
    let focus = if pane.focused {
        r#"<span class="chip info">focused</span>"#
    } else {
        ""
    };
    let title = pane.title.as_deref().unwrap_or("-");
    let custom_status = pane.custom_status.as_deref().unwrap_or("-");

    format!(
        r#"<article class="pane">
  <div class="pane-main">
    <div class="pane-title"><strong>{}</strong>{}</div>
    <div class="path mono">{}</div>
    <div class="meta">
      <span class="chip {}">{}</span>
      <span class="chip {}">{}</span>
      {}
    </div>
  </div>
  <div class="sessions">{}</div>
</article>"#,
        html_escape(&pane_title(pane)),
        focus,
        html_escape(&pane.cwd),
        status_class(&pane.pane_status),
        html_escape(&pane.pane_status),
        enabled_class,
        html_escape(&enabled_label),
        render_counterspell_meta(title, custom_status),
        sessions
    )
}

fn render_session(session: &ClaudeSessionView) -> String {
    let action = if session.direct_target {
        format!(
            r#"<form method="post" action="/targets/disable">
  <input type="hidden" name="session_id" value="{}">
  <button class="off" type="submit">Disable</button>
</form>"#,
            html_escape(&session.session_id)
        )
    } else if session.enabled {
        r#"<button type="button" disabled>Pattern</button>"#.to_string()
    } else {
        format!(
            r#"<form method="post" action="/targets/enable">
  <input type="hidden" name="session_id" value="{}">
  <button type="submit">Enable</button>
</form>"#,
            html_escape(&session.session_id)
        )
    };
    let enabled_class = if session.enabled { "ok" } else { "warn" };
    let enabled_label = if session.enabled { "on" } else { "off" };
    let target = if session.enabled {
        compact_target(&session.target)
    } else {
        "not targeted".to_string()
    };

    format!(
        r#"<div class="session">
  <div><span class="mono session-id">{}</span></div>
  <div class="session-detail">
    <strong>{}</strong> <span>{}</span>
    <div class="session-subline"><span>{}</span></div>
  </div>
  <div class="session-action"><span class="chip {}">{}</span>{}</div>
</div>"#,
        html_escape(&session.short_session_id),
        html_escape(&session.model),
        html_escape(&session.updated),
        html_escape(&target),
        enabled_class,
        enabled_label,
        action
    )
}

fn compact_target(target: &str) -> String {
    let model = target.split_whitespace().next().unwrap_or(target);
    format!("target {model}")
}

fn render_counterspell_meta(title: &str, custom_status: &str) -> String {
    if title == "-" && custom_status == "-" {
        return String::new();
    }
    let mut items = Vec::new();
    if title != "-" {
        items.push(format!(
            r#"<span class="chip info counterspell-chip">{}</span>"#,
            html_escape(title)
        ));
    }
    if custom_status != "-" {
        items.push(format!(
            r#"<span class="chip info counterspell-chip">{}</span>"#,
            html_escape(custom_status)
        ));
    }
    items.join("")
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
                        "target": session.target,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "generated_at": snapshot.generated_at.to_rfc3339(),
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

fn workspace_number(pane: &ClaudePaneView) -> String {
    pane.workspace_number
        .map(|number| number.to_string())
        .unwrap_or_else(|| pane.workspace_id.clone())
}

fn pane_title(pane: &ClaudePaneView) -> String {
    let tab = pane
        .tab_number
        .map(|number| format!("Tab {number}: {}", pane.tab_label))
        .unwrap_or_else(|| pane.tab_label.clone());
    format!("{tab} / {}", pane.pane_id)
}

fn status_class(value: &str) -> &'static str {
    match value {
        "idle" | "done" => "ok",
        "working" => "info",
        "blocked" => "blocked",
        _ => "warn",
    }
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

fn scroll_text_icon() -> &'static str {
    r#"<svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
  <path d="M8 21h8a3 3 0 0 0 3-3V7a4 4 0 0 0-4-4H7a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3h1"/>
  <path d="M8 21a3 3 0 0 1-3-3V7"/>
  <path d="M9 8h6"/>
  <path d="M9 12h6"/>
  <path d="M9 16h4"/>
</svg>"#
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
