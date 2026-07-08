// Counterspell Desktop frontend — Rail skin (counterspell-919 option 6). A thin
// view over the Rust IPC commands: poll status every 3s, tail the activation
// log, swap the desk view from the rail, and expose the four control surfaces
// (arm/disarm, per-session toggle, rebind). No client-side policy.
const invoke = window.__TAURI__.core.invoke;

const POLL_MS = 3000;
const LOG_LIMIT = 80;

const state = {
  view: "roster", // roster | log | health
  showAll: false,
  snap: null,
  log: [],
};

function icon(id) {
  return `<svg class="icon"><use href="#${id}" /></svg>`;
}

function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, (ch) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[ch]);
}

// ── Verdict (rail foot; persistent chrome) ───────────────
const VERDICT = {
  shielded: {
    cls: "shielded", glyph: "i-shield-check", label: "SHIELDED",
    sub: (s) => `Watching ${s.summary.watched} session(s) · nothing drifting`,
  },
  acting: {
    cls: "acting", glyph: "i-wand", label: "ACTING",
    sub: () => "Returning a drifted session to Fable",
  },
  "drift-blocked": {
    cls: "drift-blocked", glyph: "i-shield-alert", label: "DRIFT-BLOCKED",
    sub: (s, v) => v.reason || "Drift detected — can't safely act",
  },
  disarmed: {
    cls: "disarmed", glyph: "i-shield-off", label: "DISARMED",
    sub: () => "Enforcement paused — the daemon takes no action",
  },
};

function renderVerdict(snap) {
  const v = snap.verdict;
  const meta = VERDICT[v.kind] || VERDICT.disarmed;
  const el = document.getElementById("verdict");
  el.className = `rl-verdict rl-verdict--${meta.cls}`;
  el.querySelector(".rl-verdict__glyph use").setAttribute("href", `#${meta.glyph}`);
  document.getElementById("verdict-label").textContent = meta.label;
  document.getElementById("verdict-sub").textContent = meta.sub(snap, v);

  const sw = document.getElementById("master-switch");
  sw.checked = snap.health.master_enabled;
  document.getElementById("master-label").textContent = snap.health.master_enabled ? "Armed" : "Disarmed";
  document.getElementById("armed-idle-warn").hidden = !snap.health.armed_but_idle;
}

// ── Roster view ──────────────────────────────────────────
function isLive(row) {
  return row.live_pane_only || (row.panes && row.panes !== "not-open");
}

function rosterRank(row) {
  if (row.drift) return 0; // alarming rows first: "am I protected right now?"
  if (row.needs_rebind) return 1;
  if (row.watched) return 2;
  if (row.live_pane_only) return 3;
  return 4;
}

function cleanName(row) {
  if (row.live_pane_only) return "live pane";
  const base = (row.cwd || "").split("/").filter(Boolean).pop();
  return base || row.project;
}

function statusCell(row) {
  if (row.live_pane_only) return `${esc(row.state)}`;
  if (row.drift) return `<span class="drift">drift ${esc(row.drift)}</span> · ${esc(row.state)}`;
  if (row.watched) return `<span class="on-model">on ${esc(row.model)}</span> → ${esc(row.target)} · ${esc(row.state)}`;
  return `${esc(row.model)} · ${esc(row.target)}`;
}

function actionCell(row) {
  const parts = [];
  if (row.needs_rebind && row.pane_id) {
    const sid = row.session_id || "";
    parts.push(
      `<button class="btn btn--warn btn--sm" data-rebind data-pane="${esc(row.pane_id)}" data-session="${esc(sid)}">${icon("i-refresh")}rebind</button>`
    );
  }
  if (!row.live_pane_only && row.session_id) {
    const checked = row.has_session_target ? "checked" : "";
    parts.push(
      `<label class="switch switch--mini" title="Pin this session as a watched target"><input type="checkbox" data-session-toggle data-session="${esc(row.session_id)}" ${checked} /><span class="switch__track"><span class="switch__thumb"></span></span></label>`
    );
  }
  return parts.join(" ");
}

function rosterView(snap) {
  const all = snap.sessions;
  const live = all.filter(isLive).length;
  const hist = all.length - live;
  const rows = all
    .filter((row) => state.showAll || isLive(row))
    .sort((a, b) => rosterRank(a) - rosterRank(b));

  const header = `<div class="sec-h">${icon("i-radio")}<span>Live roster</span>
    <span class="rt">${live} live · ${hist} historical
      <label class="filter"><input type="checkbox" id="show-all" ${state.showAll ? "checked" : ""} /> show all</label>
    </span></div>`;

  if (rows.length === 0) {
    return `<div class="sec">${header}<p class="empty">No ${state.showAll ? "" : "live "}sessions right now.</p></div>`;
  }

  const body = rows
    .map(
      (row) => `<tr>
        <td class="repo">${esc(cleanName(row))}</td>
        <td class="pane">${esc(row.panes && row.panes !== "not-open" ? row.panes : "not open")}</td>
        <td class="st">${statusCell(row)}</td>
        <td class="act">${actionCell(row)}</td>
      </tr>`
    )
    .join("");

  return `<div class="sec">${header}
    <table class="tbl">
      <thead><tr><th>repo</th><th>pane</th><th>status</th><th></th></tr></thead>
      <tbody>${body}</tbody>
    </table></div>`;
}

// ── Log view ─────────────────────────────────────────────
const LOG_GLYPH = {
  confirmed: "i-circle-check",
  "in-flight": "i-wand",
  blocked: "i-alert",
  detected: "i-dot",
  ignored: "i-minus",
};

function logView(entries) {
  const header = `<div class="sec-h">${icon("i-activity")}<span>Activation log</span><span class="rt">outcome-stamped</span></div>`;
  if (!entries || entries.length === 0) {
    return `<div class="sec">${header}<p class="empty">No activations logged yet.</p></div>`;
  }
  const rows = entries
    .slice()
    .reverse()
    .map(
      (e) => `<div class="lrow lrow--${esc(e.outcome)}">
        <span class="lg">${icon(LOG_GLYPH[e.outcome] || "i-dot")}</span>
        <span class="lt">${esc(e.at)}</span>
        <span class="lb">${esc(e.text)}</span>
      </div>`
    )
    .join("");
  return `<div class="sec">${header}<div class="log">${rows}</div></div>`;
}

// ── Health view (full doctor) ────────────────────────────
function healthView(health) {
  const daemonBad = !health.daemon_scheduled;
  const items = [
    { ok: !daemonBad, label: "daemon", note: `${health.daemon_status}${health.last_tick_age ? ` · last tick ${health.last_tick_age}` : ""}` },
    { ok: health.herdr_reachable, label: "herdr", note: health.herdr_reachable ? "reachable" : "unreachable" },
    { ok: true, label: "master switch", note: health.master_enabled ? "armed" : "disarmed" },
  ]
    .map(
      (h) => `<span class="hitem ${h.ok ? "ok" : "bad"}">${icon(h.ok ? "i-circle-check" : "i-alert")}
        <span class="hl">${esc(h.label)}</span><span class="hn">${esc(h.note)}</span></span>`
    )
    .join("");

  const warn = health.armed_but_idle
    ? `<div class="banner banner--warn">Armed, but the watch-arm daemon is not scheduled — run <code>counterspell enable</code>.</div>`
    : "";

  const swiftbar = state.swiftbarPresent
    ? `<div class="banner banner--muted"><span>A legacy SwiftBar menu-bar plugin is still installed. The tray icon replaces it.</span><button id="swiftbar-remove" class="btn btn--sm" type="button">Remove it</button></div>`
    : "";

  return `<div class="sec"><div class="sec-h">${icon("i-pulse")}<span>Health</span></div>
    <div class="health">${items}</div></div>${warn}${swiftbar}`;
}

// ── Desk render + rail nav ───────────────────────────────
function renderDesk() {
  const desk = document.getElementById("desk");
  if (!state.snap) {
    desk.innerHTML = `<div class="sec"><p class="empty">Loading…</p></div>`;
    return;
  }
  if (state.view === "roster") desk.innerHTML = rosterView(state.snap);
  else if (state.view === "log") desk.innerHTML = logView(state.log);
  else desk.innerHTML = healthView(state.snap.health);
}

function setView(view) {
  state.view = view;
  document.querySelectorAll(".rl-item").forEach((btn) => {
    btn.classList.toggle("rl-item--on", btn.dataset.view === view);
  });
  renderDesk();
}

// ── Poll + wire ──────────────────────────────────────────
async function refresh() {
  try {
    const [snap, log] = await Promise.all([
      invoke("get_status"),
      invoke("get_activation_log", { limit: LOG_LIMIT }),
    ]);
    state.snap = snap;
    state.log = log;
    renderVerdict(snap);
    renderDesk();
    document.getElementById("freshness").textContent = "updated just now";
  } catch (error) {
    document.getElementById("freshness").textContent = `error: ${error}`;
  }
  maybeCheckSwiftbar();
}

let swiftbarChecked = false;
async function maybeCheckSwiftbar() {
  if (swiftbarChecked) return;
  swiftbarChecked = true;
  try {
    state.swiftbarPresent = await invoke("swiftbar_present");
    if (state.view === "health") renderDesk();
  } catch (_) {
    /* non-fatal */
  }
}

function wire() {
  document.querySelectorAll(".rl-item").forEach((btn) => {
    btn.addEventListener("click", () => setView(btn.dataset.view));
  });

  document.getElementById("master-switch").addEventListener("change", async (e) => {
    try {
      await invoke("set_master", { enabled: e.target.checked });
    } finally {
      refresh();
    }
  });

  // Desk is re-rendered on every refresh, so bind controls via delegation.
  const desk = document.getElementById("desk");

  desk.addEventListener("change", async (e) => {
    const showAll = e.target.closest("#show-all");
    if (showAll) {
      state.showAll = e.target.checked;
      renderDesk();
      return;
    }
    const toggle = e.target.closest("[data-session-toggle]");
    if (toggle) {
      try {
        await invoke("set_session_enabled", { sessionId: toggle.dataset.session, enabled: toggle.checked });
      } finally {
        refresh();
      }
    }
  });

  desk.addEventListener("click", async (e) => {
    const rebind = e.target.closest("[data-rebind]");
    if (rebind) {
      rebind.disabled = true;
      rebind.textContent = "…";
      try {
        await invoke("rebind_pane", { paneId: rebind.dataset.pane, sessionId: rebind.dataset.session });
      } catch (error) {
        rebind.textContent = "failed";
      } finally {
        refresh();
      }
      return;
    }
    const swiftbar = e.target.closest("#swiftbar-remove");
    if (swiftbar) {
      try {
        await invoke("remove_swiftbar");
      } finally {
        state.swiftbarPresent = false;
        renderDesk();
      }
    }
  });

  document.getElementById("refresh").addEventListener("click", refresh);

  document.getElementById("theme-toggle").addEventListener("click", () => {
    const root = document.documentElement;
    const current = root.getAttribute("data-theme");
    const isDark = current
      ? current === "dark"
      : window.matchMedia("(prefers-color-scheme: dark)").matches;
    const next = isDark ? "light" : "dark";
    root.setAttribute("data-theme", next);
    document.querySelector("#theme-toggle use").setAttribute("href", next === "dark" ? "#i-moon" : "#i-sun");
  });
}

wire();
refresh();
setInterval(refresh, POLL_MS);
