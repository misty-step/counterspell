use crate::dashboard::{ClaudePaneView, ClaudeSessionView, DashboardSnapshot};
use crate::util::html_escape;

struct WorkspaceGroup<'a> {
    id: &'a str,
    label: &'a str,
    number: Option<u64>,
    panes: Vec<&'a ClaudePaneView>,
}

pub(crate) fn render_dashboard_html(snapshot: &DashboardSnapshot) -> String {
    let groups = workspace_groups(&snapshot.panes);
    let body = if snapshot.panes.is_empty() {
        r#"<section class="empty">No live Claude Code panes found in Herdr.</section>"#.to_string()
    } else {
        format!(
            r#"<section class="drilldown" aria-label="Herdr Mirror Column Drilldown">
  {}
  {}
  {}
  {}
</section>"#,
            render_workspace_column(&groups),
            render_pane_column(&groups),
            render_session_column(&snapshot.panes),
            render_action_column(&snapshot.panes)
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en" class="dark">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="refresh" content="10">
  <title>Counterspell</title>
  <style>{}</style>
</head>
<body>
  <main class="shell">
    <aside class="rail">
      <div>
        <span class="chrome">Counterspell</span>
        <h1>Herdr Mirror Column Drilldown</h1>
        <p>Fable Claude Code sessions auto-watch.</p>
      </div>
      <section class="metrics" aria-label="Counterspell summary">
        <div><strong>{}</strong><span>Claude panes</span></div>
        <div><strong>{}</strong><span>Enabled panes</span></div>
        <div><strong>{}</strong><span>Enabled sessions</span></div>
        <div><strong>{}</strong><span>Herdr spaces</span></div>
      </section>
      <footer class="chrome">Generated {}</footer>
    </aside>
    <section class="stage">
      <header class="stage-head">
        <div>
          <span class="chrome">workspace -> tab -> session -> policy</span>
          <strong>Fable sessions are active automatically.</strong>
        </div>
        <a href="/status.json">status.json</a>
      </header>
      {}
    </section>
  </main>
  <script>{}</script>
</body>
</html>
"#,
        style(),
        snapshot.summary.claude_panes,
        snapshot.summary.enabled_panes,
        snapshot.summary.enabled_sessions,
        snapshot.summary.workspaces,
        html_escape(&snapshot.generated_at.to_rfc3339()),
        body,
        script()
    )
}

fn workspace_groups(panes: &[ClaudePaneView]) -> Vec<WorkspaceGroup<'_>> {
    let mut groups = Vec::<WorkspaceGroup<'_>>::new();
    for pane in panes {
        if groups
            .last()
            .map(|group| group.id != pane.workspace_id.as_str())
            .unwrap_or(true)
        {
            groups.push(WorkspaceGroup {
                id: &pane.workspace_id,
                label: &pane.workspace_label,
                number: pane.workspace_number,
                panes: Vec::new(),
            });
        }
        groups.last_mut().expect("workspace group").panes.push(pane);
    }
    groups
}

fn render_workspace_column(groups: &[WorkspaceGroup<'_>]) -> String {
    let buttons = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let selected = if index == 0 { "true" } else { "false" };
            let active = if index == 0 { " is-active" } else { "" };
            let watched = group.panes.iter().filter(|pane| pane.enabled()).count();
            format!(
                r#"<button class="choice{}" type="button" data-workspace-trigger="{}" aria-selected="{}">
  <span><strong>{}</strong><em>{}</em></span>
  <b>{}/{}</b>
</button>"#,
                active,
                html_escape(group.id),
                selected,
                html_escape(group.label),
                html_escape(&workspace_label(group)),
                watched,
                group.panes.len()
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<section class="column spaces">
  <header><strong>spaces</strong><span>{}</span></header>
  <div class="column-scroll">{}</div>
</section>"#,
        groups.len(),
        buttons
    )
}

fn render_pane_column(groups: &[WorkspaceGroup<'_>]) -> String {
    let sections = groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let hidden = if index == 0 { "" } else { " is-hidden" };
            let panes = group
                .panes
                .iter()
                .enumerate()
                .map(|(pane_index, pane)| render_pane_button(pane, index == 0 && pane_index == 0))
                .collect::<Vec<_>>()
                .join("");
            format!(
                r#"<div class="pane-list{}" data-workspace-pane-list="{}">{}</div>"#,
                hidden,
                html_escape(group.id),
                panes
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<section class="column tabs">
  <header><strong>tabs</strong><span>Claude Code</span></header>
  <div class="column-scroll">{}</div>
</section>"#,
        sections
    )
}

fn render_pane_button(pane: &ClaudePaneView, selected: bool) -> String {
    let active = if selected { " is-active" } else { "" };
    let selected = if selected { "true" } else { "false" };
    let enabled = pane
        .sessions
        .iter()
        .filter(|session| session.enabled)
        .count();
    let focus = if pane.focused { " · focused" } else { "" };
    format!(
        r#"<button class="choice{}" type="button" data-pane-trigger="{}" aria-selected="{}">
  <span><strong>{}</strong><em>{}</em></span>
  <b>{}</b>
  <span class="status {}">{}</span>
  <span class="chrome">{}/{} targets{}</span>
</button>"#,
        active,
        html_escape(&pane.pane_id),
        selected,
        html_escape(&pane_title(pane)),
        html_escape(&pane.cwd),
        html_escape(&pane.pane_id),
        status_class(&pane.pane_status),
        html_escape(&pane.pane_status),
        enabled,
        pane.sessions.len(),
        focus
    )
}

fn render_session_column(panes: &[ClaudePaneView]) -> String {
    let panels = panes
        .iter()
        .enumerate()
        .map(|(index, pane)| {
            let hidden = if index == 0 { "" } else { " is-hidden" };
            let sessions = if pane.sessions.is_empty() {
                r#"<div class="empty compact">No recent transcript session mapped to this pane cwd.</div>"#
                    .to_string()
            } else {
                pane.sessions
                    .iter()
                    .map(render_session_row)
                    .collect::<Vec<_>>()
                    .join("")
            };
            format!(
                r#"<div class="session-panel{}" data-pane-panel="{}">
  <div class="panel-title"><strong>{}</strong><span>{}</span></div>
  {}
</div>"#,
                hidden,
                html_escape(&pane.pane_id),
                html_escape(&pane.tab_label),
                html_escape(&pane.pane_id),
                sessions
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<section class="column sessions">
  <header><strong>sessions</strong><span>recent transcripts</span></header>
  <div class="column-scroll">{}</div>
</section>"#,
        panels
    )
}

fn render_session_row(session: &ClaudeSessionView) -> String {
    let status_label = if session.auto_target {
        "auto"
    } else if session.enabled {
        "enabled"
    } else {
        "ignored"
    };

    format!(
        r#"<article class="session-row">
  <div>
    <strong class="mono">{}</strong>
    <span>{} · {}</span>
  </div>
  <span class="status {}">{}</span>
  <span class="chrome">{}</span>
</article>"#,
        html_escape(&session.short_session_id),
        html_escape(&session.model),
        html_escape(&session.updated),
        if session.enabled { "ok" } else { "muted" },
        status_label,
        html_escape(&session_target_label(session))
    )
}

fn render_action_column(panes: &[ClaudePaneView]) -> String {
    let panels = panes
        .iter()
        .enumerate()
        .map(|(index, pane)| {
            let hidden = if index == 0 { "" } else { " is-hidden" };
            format!(
                r#"<div class="action-panel{}" data-action-panel="{}">
  <div class="panel-title"><strong>{}</strong><span>{}</span></div>
  <dl>
    <div><dt>workspace</dt><dd>{}</dd></div>
    <div><dt>cwd</dt><dd>{}</dd></div>
    <div><dt>herdr title</dt><dd>{}</dd></div>
    <div><dt>counterspell status</dt><dd>{}</dd></div>
    <div><dt>mode</dt><dd>{}</dd></div>
  </dl>
  <div class="actions">{}</div>
</div>"#,
                hidden,
                html_escape(&pane.pane_id),
                html_escape(&pane_title(pane)),
                html_escape(&pane.pane_status),
                html_escape(&pane.workspace_label),
                html_escape(&pane.cwd),
                html_escape(pane.title.as_deref().unwrap_or("-")),
                html_escape(pane.custom_status.as_deref().unwrap_or("-")),
                if pane.enabled() {
                    "watched Fable sessions"
                } else {
                    "no Fable session"
                },
                render_session_actions(pane)
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<section class="column action">
  <header><strong>policy</strong><span>auto target</span></header>
  <div class="column-scroll">{}</div>
</section>"#,
        panels
    )
}

fn render_session_actions(pane: &ClaudePaneView) -> String {
    if pane.sessions.is_empty() {
        return r#"<button type="button" disabled>No session</button>"#.to_string();
    }

    pane.sessions
        .iter()
        .map(|session| {
            let control = if session.auto_target {
                r#"<button type="button" disabled>Auto</button>"#.to_string()
            } else if session.direct_target {
                format!(
                    r#"<form method="post" action="/targets/disable">
  <input type="hidden" name="session_id" value="{}">
  <button class="secondary danger" type="submit">Disable</button>
</form>"#,
                    html_escape(&session.session_id)
                )
            } else if session.enabled {
                r#"<button type="button" disabled>Configured</button>"#.to_string()
            } else {
                r#"<button type="button" disabled>Not Fable</button>"#.to_string()
            };
            format!(
                r#"<div class="action-row">
  <div><strong class="mono">{}</strong><span>{}</span></div>
  {}
</div>"#,
                html_escape(&session.short_session_id),
                html_escape(&session_target_label(session)),
                control
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn session_target_label(session: &ClaudeSessionView) -> String {
    if session.enabled {
        compact_target(&session.target)
    } else {
        "not targeted".to_string()
    }
}

fn compact_target(target: &str) -> String {
    let model = target.split_whitespace().next().unwrap_or(target);
    format!("target {model}")
}

fn workspace_label(group: &WorkspaceGroup<'_>) -> String {
    group
        .number
        .map(|number| format!("workspace {number} · {}", group.id))
        .unwrap_or_else(|| group.id.to_string())
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
        "working" => "warn",
        "blocked" => "err",
        _ => "muted",
    }
}

fn style() -> &'static str {
    r#"
:root {
  color-scheme: light dark;
  --surface: #fcfcfc;
  --wash: #f3f3f3;
  --ink: #151515;
  --muted: #737373;
  --faint: #a3a3a3;
  --line: #e9e9e9;
  --accent: #2643d0;
  --ok: #15714b;
  --warn: #8a5f32;
  --err: #a84138;
  --font: 'Geist', 'Helvetica Neue', Helvetica, Arial, sans-serif;
  --mono: 'Geist Mono', ui-monospace, SFMono-Regular, Menlo, monospace;
  --ease: cubic-bezier(0.23, 1, 0.32, 1);
  --quick: 160ms;
}
.dark {
  --surface: #121212;
  --wash: #1b1b1b;
  --ink: #ededed;
  --muted: #8f8f8f;
  --faint: #5c5c5c;
  --line: #262626;
  --accent: #8c9eff;
  --ok: #6fd2a8;
  --warn: #c49d72;
  --err: #e58379;
}
* { box-sizing: border-box; }
html, body { min-height: 100%; overflow-x: hidden; }
body {
  margin: 0;
  background: var(--surface);
  color: var(--ink);
  font: 16px/1.45 var(--font);
  -webkit-font-smoothing: antialiased;
}
h1, p { margin: 0; font-size: 1em; }
h1, strong { font-weight: 800; }
a { color: var(--ink); text-underline-offset: .18em; }
button, a { cursor: pointer; }
.shell {
  display: grid;
  grid-template-columns: 18rem minmax(0, 1fr);
  min-height: 100dvh;
  min-width: 0;
}
.rail {
  display: grid;
  grid-template-rows: auto auto minmax(0, 1fr);
  gap: 1.5em;
  padding: 1.5em;
  border-right: 1px solid var(--line);
  background: var(--wash);
  min-width: 0;
}
.rail p, .stage-head span, .choice em, .panel-title span, dt, .session-row span,
.action-row span, .chrome { color: var(--muted); }
.chrome, .mono, .metrics span, .choice em, .status, dt, .session-row span,
.action-row span, footer {
  font-family: var(--mono);
  font-size: 13px;
  font-variant-numeric: tabular-nums;
}
.metrics {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  border-top: 1px solid var(--line);
  border-left: 1px solid var(--line);
}
.metrics div {
  min-height: 4em;
  padding: .75em;
  border-right: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
}
.metrics strong { display: block; }
.stage { min-width: 0; display: grid; grid-template-rows: auto minmax(0, 1fr); }
.stage-head {
  min-height: 4.5em;
  padding: 1em 1.5em;
  border-bottom: 1px solid var(--line);
  display: flex;
  justify-content: space-between;
  gap: 1.5em;
  align-items: center;
}
.stage-head div { display: grid; gap: .25em; min-width: 0; }
.stage-head strong, .rail p { overflow-wrap: anywhere; text-wrap: pretty; }
.drilldown {
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(13rem, .72fr) minmax(17rem, .95fr) minmax(18rem, 1fr) minmax(18rem, .9fr);
}
.column {
  min-width: 0;
  min-height: 0;
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  border-right: 1px solid var(--line);
}
.column:last-child { border-right: 0; }
.column header {
  min-height: 3.4em;
  padding: 0 1em;
  border-bottom: 1px solid var(--line);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1em;
}
.column header span { color: var(--muted); font-family: var(--mono); font-size: 13px; }
.column-scroll { min-height: 0; overflow: auto; }
.choice {
  width: 100%;
  min-height: 4.25em;
  padding: .75em 1em;
  border: 0;
  border-bottom: 1px solid var(--line);
  background: transparent;
  color: var(--ink);
  font: inherit;
  text-align: left;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: .75em;
  align-items: center;
  transition-property: background-color, box-shadow, transform;
  transition-duration: var(--quick);
  transition-timing-function: var(--ease);
}
.choice:active, button:active { transform: scale(.96); }
.choice:focus-visible, button:focus-visible, a:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
}
.choice span { min-width: 0; display: grid; gap: .25em; }
.choice strong, .choice em { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.choice.is-active { box-shadow: inset 3px 0 0 var(--accent); background: var(--wash); }
.status::before {
  content: "";
  display: inline-block;
  width: .65em;
  height: .65em;
  margin-right: .5em;
  border: 1px solid currentColor;
}
.status.ok { color: var(--ok); }
.status.warn { color: var(--warn); }
.status.err { color: var(--err); }
.status.muted { color: var(--faint); }
.pane-list.is-hidden, .session-panel.is-hidden, .action-panel.is-hidden { display: none; }
.panel-title {
  min-height: 4em;
  padding: 1em;
  border-bottom: 1px solid var(--line);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 1em;
}
.session-row, .action-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 1em;
  padding: .9em 1em;
  border-bottom: 1px solid var(--line);
  align-items: center;
}
.session-row div, .action-row div { min-width: 0; display: grid; gap: .2em; }
.session-row .chrome { grid-column: 1 / -1; }
dl { margin: 0; padding: 1em; display: grid; gap: .85em; border-bottom: 1px solid var(--line); }
dl div { min-width: 0; }
dt, dd { margin: 0; overflow-wrap: anywhere; }
.actions { display: grid; align-content: start; }
form { margin: 0; }
button {
  min-height: 40px;
  border: 1px solid var(--ink);
  background: var(--ink);
  color: var(--surface);
  padding: 0 1em;
  font: inherit;
  transition-property: transform, background-color, border-color;
  transition-duration: var(--quick);
  transition-timing-function: var(--ease);
}
button.secondary { background: transparent; color: var(--ink); border-color: var(--line); }
button.danger { color: var(--err); }
button:disabled { background: var(--wash); color: var(--muted); border-color: var(--line); cursor: default; }
.empty { margin: 1.5em; padding: 1em; border: 1px solid var(--line); color: var(--muted); }
.empty.compact { margin: 1em; }
@media (max-width: 980px) {
  .shell { grid-template-columns: minmax(0, 1fr); width: 100vw; max-width: 100vw; overflow: hidden; }
  .rail, .stage, .drilldown, .column { width: 100%; max-width: 100vw; min-width: 0; }
  .rail { border-right: 0; border-bottom: 1px solid var(--line); grid-template-columns: 1fr; }
  .drilldown { grid-template-columns: 1fr; }
  .column { min-height: 18rem; border-right: 0; border-bottom: 1px solid var(--line); }
  .stage-head { display: grid; }
}
@media (max-width: 620px) {
  .rail, .stage-head { padding: 1em; }
  .rail > div, .rail p, .rail h1, .stage-head div, .stage-head strong, .stage-head span {
    display: block;
    max-width: calc(100vw - 2em);
    white-space: normal;
    overflow-wrap: anywhere;
  }
  .metrics { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .session-row, .action-row { grid-template-columns: 1fr; }
  .action-row button { width: 100%; }
}
@media (prefers-reduced-motion: reduce) {
  .choice, button { transition-duration: 0ms; }
  .choice:active, button:active { transform: none; }
}
"#
}

fn script() -> &'static str {
    r#"
function showWorkspace(id) {
  document.querySelectorAll('[data-workspace-trigger]').forEach(function (button) {
    var selected = button.dataset.workspaceTrigger === id;
    button.classList.toggle('is-active', selected);
    button.setAttribute('aria-selected', selected ? 'true' : 'false');
  });
  document.querySelectorAll('[data-workspace-pane-list]').forEach(function (list) {
    list.classList.toggle('is-hidden', list.dataset.workspacePaneList !== id);
  });
  var firstPane = document.querySelector('[data-workspace-pane-list="' + CSS.escape(id) + '"] [data-pane-trigger]');
  if (firstPane) showPane(firstPane.dataset.paneTrigger);
}
function showPane(id) {
  document.querySelectorAll('[data-pane-trigger]').forEach(function (button) {
    var selected = button.dataset.paneTrigger === id;
    button.classList.toggle('is-active', selected);
    button.setAttribute('aria-selected', selected ? 'true' : 'false');
  });
  document.querySelectorAll('[data-pane-panel]').forEach(function (panel) {
    panel.classList.toggle('is-hidden', panel.dataset.panePanel !== id);
  });
  document.querySelectorAll('[data-action-panel]').forEach(function (panel) {
    panel.classList.toggle('is-hidden', panel.dataset.actionPanel !== id);
  });
}
document.querySelectorAll('[data-workspace-trigger]').forEach(function (button) {
  button.addEventListener('click', function () { showWorkspace(button.dataset.workspaceTrigger); });
});
document.querySelectorAll('[data-pane-trigger]').forEach(function (button) {
  button.addEventListener('click', function () { showPane(button.dataset.paneTrigger); });
});
"#
}
