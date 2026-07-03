use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command as ProcessCommand;

use crate::cli::{Cli, UiArgs};
use crate::config::load_config;
use crate::herdr::load_herdr_panes;
use crate::model::{StatusRow, StatusSummary};
use crate::output::status_summary;
use crate::sessions::discover_recent_sessions;
use crate::status::status_rows;
use crate::store::{load_store, state_path};
use crate::util::html_escape;

pub(crate) struct DashboardSnapshot {
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) summary: StatusSummary,
    pub(crate) rows: Vec<StatusRow>,
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
    let store = load_store(&state_path(cli.state.clone())?)?;
    let sessions = discover_recent_sessions(&config, generated_at)?;
    let panes = load_herdr_panes().context("load Herdr panes for dashboard")?;
    let rows = status_rows(&sessions, &panes, &store, &config, generated_at);
    let summary = status_summary(&rows, &store, generated_at);

    Ok(DashboardSnapshot {
        generated_at,
        summary,
        rows,
    })
}

pub(crate) fn render_dashboard_html(snapshot: &DashboardSnapshot) -> String {
    let rows = if snapshot.rows.is_empty() {
        r#"<tr><td colspan="11" class="empty">No recent Claude sessions found.</td></tr>"#
            .to_string()
    } else {
        snapshot
            .rows
            .iter()
            .map(render_row)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let running_detail = format!(
        "{} watched / {} ignored / {} mapped",
        snapshot.summary.watched, snapshot.summary.ignored, snapshot.summary.mapped
    );
    let last_trigger = snapshot
        .summary
        .last_trigger_event
        .clone()
        .unwrap_or_else(|| "none".to_string());

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
      --muted: #5d6874;
      --line: #d8dee5;
      --green: #1d7f56;
      --red: #b33939;
      --amber: #986d18;
      --blue: #245e9a;
      --shadow: 0 16px 42px rgba(23, 32, 38, .10);
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      background: var(--bg);
      color: var(--ink);
      font: 14px/1.45 ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }}
    main {{ width: min(1480px, calc(100vw - 48px)); margin: 28px auto 40px; }}
    header {{
      display: flex;
      align-items: flex-start;
      justify-content: space-between;
      gap: 24px;
      padding-bottom: 20px;
      border-bottom: 1px solid var(--line);
    }}
    .brand {{ display: flex; gap: 14px; align-items: center; min-width: 0; }}
    .mark {{
      width: 44px;
      height: 44px;
      border: 1px solid var(--ink);
      border-radius: 8px;
      display: grid;
      place-items: center;
      background: #ffffff;
      box-shadow: var(--shadow);
      flex: 0 0 auto;
    }}
    h1 {{ margin: 0; font-size: 26px; line-height: 1.05; letter-spacing: 0; }}
    .subtitle {{ margin-top: 6px; color: var(--muted); overflow-wrap: anywhere; }}
    .running {{
      display: flex;
      align-items: center;
      gap: 9px;
      min-width: 260px;
      justify-content: flex-end;
      color: var(--muted);
    }}
    .dot {{ width: 10px; height: 10px; border-radius: 999px; background: var(--green); box-shadow: 0 0 0 4px rgba(29,127,86,.14); }}
    .summary {{
      display: grid;
      grid-template-columns: repeat(5, minmax(120px, 1fr));
      gap: 10px;
      margin: 20px 0;
    }}
    .metric {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
      padding: 12px 14px;
      min-height: 78px;
    }}
    .metric span {{ display: block; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: 0; }}
    .metric strong {{ display: block; margin-top: 7px; font-size: 28px; line-height: 1; letter-spacing: 0; }}
    .table-wrap {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 8px;
      overflow: auto;
      box-shadow: var(--shadow);
    }}
    table {{ width: 100%; border-collapse: collapse; min-width: 1120px; }}
    th, td {{ padding: 10px 12px; border-bottom: 1px solid var(--line); text-align: left; vertical-align: top; }}
    th {{ font-size: 11px; text-transform: uppercase; color: var(--muted); background: #eef3f6; letter-spacing: 0; }}
    td {{ font-size: 13px; }}
    tbody tr:last-child td {{ border-bottom: 0; }}
    .mono {{ font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace; overflow-wrap: anywhere; }}
    .cwd {{ max-width: 340px; }}
    .chip {{
      display: inline-flex;
      align-items: center;
      min-height: 24px;
      padding: 2px 8px;
      border-radius: 999px;
      border: 1px solid var(--line);
      background: #f8fafb;
      white-space: nowrap;
    }}
    .chip.ok {{ color: var(--green); border-color: rgba(29,127,86,.28); background: rgba(29,127,86,.08); }}
    .chip.warn {{ color: var(--amber); border-color: rgba(152,109,24,.28); background: rgba(152,109,24,.10); }}
    .chip.blocked {{ color: var(--red); border-color: rgba(179,57,57,.28); background: rgba(179,57,57,.08); }}
    .chip.info {{ color: var(--blue); border-color: rgba(36,94,154,.26); background: rgba(36,94,154,.08); }}
    footer {{ margin-top: 14px; color: var(--muted); font-size: 12px; display: flex; justify-content: space-between; gap: 16px; flex-wrap: wrap; }}
    .empty {{ color: var(--muted); text-align: center; padding: 28px; }}
    @media (max-width: 820px) {{
      main {{ width: min(100vw - 24px, 1480px); margin-top: 18px; }}
      header {{ display: block; }}
      .running {{ justify-content: flex-start; margin-top: 14px; }}
      .summary {{ grid-template-columns: repeat(2, minmax(120px, 1fr)); }}
      h1 {{ font-size: 24px; }}
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
          <div class="subtitle">Visual watch status for local Claude sessions and Herdr panes.</div>
        </div>
      </div>
      <div class="running"><span class="dot" aria-hidden="true"></span><span>Running locally: {}</span></div>
    </header>

    <section class="summary" aria-label="Counterspell summary">
      <div class="metric"><span>Total sessions</span><strong>{}</strong></div>
      <div class="metric"><span>Watched</span><strong>{}</strong></div>
      <div class="metric"><span>Ignored</span><strong>{}</strong></div>
      <div class="metric"><span>Mapped panes</span><strong>{}</strong></div>
      <div class="metric"><span>Live panes</span><strong>{}</strong></div>
    </section>

    <div class="table-wrap">
      <table>
        <thead>
          <tr>
            <th>Session</th><th>Project</th><th>CWD</th><th>Pane</th><th>Agent</th><th>State</th>
            <th>Watch</th><th>Target</th><th>Model</th><th>Drift</th><th>Updated</th>
          </tr>
        </thead>
        <tbody>
          {}
        </tbody>
      </table>
    </div>

    <footer>
      <span>{}</span>
      <span>Last trigger: {}</span>
      <span>Generated: {}</span>
    </footer>
  </main>
</body>
</html>
"#,
        scroll_text_icon(),
        html_escape(&running_detail),
        snapshot.summary.total,
        snapshot.summary.watched,
        snapshot.summary.ignored,
        snapshot.summary.mapped,
        snapshot.summary.live_panes,
        rows,
        html_escape(&running_detail),
        html_escape(&last_trigger),
        snapshot.generated_at.to_rfc3339()
    )
}

fn handle_connection(mut stream: TcpStream, cli: &Cli) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("clone dashboard stream")?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("read dashboard request line")?;

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    while {
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .context("read dashboard header")?;
        !header.trim().is_empty()
    } {}

    match path {
        "/" | "/index.html" => {
            let snapshot = load_dashboard_snapshot(cli)?;
            respond(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                render_dashboard_html(&snapshot),
            )
        }
        "/status.json" => {
            let snapshot = load_dashboard_snapshot(cli)?;
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "generated_at": snapshot.generated_at.to_rfc3339(),
                "summary": snapshot.summary,
                "rows": snapshot.rows,
            }))?;
            respond(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            )
        }
        "/favicon.ico" => respond(&mut stream, "204 No Content", "text/plain", String::new()),
        _ => respond(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    }
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

fn render_row(row: &StatusRow) -> String {
    format!(
        "<tr><td class=\"mono\">{}</td><td>{}</td><td class=\"mono cwd\">{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td></tr>",
        html_escape(&row.session_id),
        html_escape(&row.project),
        html_escape(&row.cwd),
        html_escape(&row.pane),
        html_escape(&row.agent),
        chip(&row.state, state_class(&row.state)),
        chip(&row.watch, if row.watch == "watched" { "ok" } else { "warn" }),
        html_escape(&row.target),
        html_escape(&row.model),
        chip(&row.drift, drift_class(&row.drift)),
        html_escape(&row.updated)
    )
}

fn chip(value: &str, class_name: &str) -> String {
    format!(
        "<span class=\"chip {}\">{}</span>",
        class_name,
        html_escape(value)
    )
}

fn state_class(value: &str) -> &'static str {
    if value == "idle" || value == "live" {
        "ok"
    } else if value.contains("ambiguous") || value.contains("busy") || value.contains("pane-") {
        "blocked"
    } else {
        "warn"
    }
}

fn drift_class(value: &str) -> &'static str {
    match value {
        "ok" => "ok",
        "ignored" => "warn",
        "-" => "info",
        _ => "blocked",
    }
}

fn scroll_text_icon() -> &'static str {
    r#"<svg viewBox="0 0 24 24" width="27" height="27" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
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
        anyhow::bail!("open exited with {status}");
    }
    Ok(())
}
