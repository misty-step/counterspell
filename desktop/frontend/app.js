// Counterspell Desktop frontend. Thin view over the Rust IPC commands: poll
// status every 3s, tail the activation log, and expose the four control
// surfaces (arm/disarm, per-session toggle, rebind). No client-side policy.
const invoke = window.__TAURI__.core.invoke;

const POLL_MS = 3000;
const LOG_LIMIT = 80;

const state = {
  tab: "roster",
  showAll: false,
  master: true,
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

// ── Verdict ──────────────────────────────────────────────
const VERDICT = {
  SHIELDED: { cls: "shielded", sub: (s) => `Watching ${s.summary.watched} session(s) · nothing drifting` },
  ACTING: { cls: "acting", sub: () => "Returning a drifted session to Fable" },
  "DRIFT-BLOCKED": { cls: "drift-blocked", sub: (s, v) => v.reason || "Drift detected — can't safely act" },
  DISARMED: { cls: "disarmed", sub: () => "Enforcement paused — the daemon takes no action" },
};

function renderVerdict(snap) {
  const v = snap.verdict;
  const label = v.kind ? v.kind.toUpperCase().replace(/-/g, "-") : "";
  const key = v.kind === "shielded" ? "SHIELDED"
    : v.kind === "acting" ? "ACTING"
    : v.kind === "drift-blocked" ? "DRIFT-BLOCKED"
    : "DISARMED";
  const meta = VERDICT[key];
  const el = document.getElementById("verdict");
  el.className = `verdict verdict--${meta.cls}`;
  document.getElementById("verdict-label").textContent = key;
  document.getElementById("verdict-sub").textContent = meta.sub(snap, v);

  state.master = snap.health.master_enabled;
  const sw = document.getElementById("master-switch");
  sw.checked = snap.health.master_enabled;
  document.getElementById("master-label").textContent = snap.health.master_enabled ? "Armed" : "Disarmed";
  document.getElementById("armed-idle-warn").hidden = !snap.health.armed_but_idle;
}

// ── Health strip ─────────────────────────────────────────
function renderHealth(health) {
  const daemonCls = health.daemon_scheduled ? "chip--ok" : (health.daemon_status === "not installed" ? "chip--warn" : "chip--err");
  const herdrCls = health.herdr_reachable ? "chip--ok" : "chip--err";
  const chips = [
    { cls: daemonCls, id: "i-dot", label: "daemon", value: health.daemon_status },
    { cls: herdrCls, id: "i-plug", label: "herdr", value: health.herdr_reachable ? "reachable" : "unreachable" },
    { cls: "", id: "i-refresh", label: "last tick", value: health.last_tick_age || "never" },
  ];
  document.getElementById("health").innerHTML = chips
    .map((c) => `<span class="chip ${c.cls}">${icon(c.id)}${esc(c.label)} <b>${esc(c.value)}</b></span>`)
    .join("");
}

// ── Roster ───────────────────────────────────────────────
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

function renderRoster(sessions) {
  const rows = sessions
    .filter((row) => state.showAll || isLive(row))
    .sort((a, b) => rosterRank(a) - rosterRank(b));
  const panel = document.getElementById("roster");
  if (rows.length === 0) {
    panel.innerHTML = `<p class="empty">No ${state.showAll ? "" : "live "}sessions right now.</p>`;
    return;
  }
  panel.innerHTML = rows.map(rosterRow).join("");
}

function cleanName(row) {
  if (row.live_pane_only) return "live pane";
  const base = (row.cwd || "").split("/").filter(Boolean).pop();
  return base || row.project;
}

function rosterRow(row) {
  const project = cleanName(row);
  const pane = row.panes && row.panes !== "not-open" ? row.panes : "not open";

  let meta;
  if (row.live_pane_only) {
    meta = `<span>${esc(row.cwd)}</span> · ${esc(row.state)}`;
  } else if (row.drift) {
    meta = `<span class="drift">drift ${esc(row.drift)}</span> · ${esc(row.state)}`;
  } else if (row.watched) {
    meta = `<span class="on-model">on ${esc(row.model)}</span> → ${esc(row.target)} · ${esc(row.state)}`;
  } else {
    meta = `${esc(row.model)} · ${esc(row.target)}`;
  }

  const actions = [];
  if (row.needs_rebind && row.pane_id) {
    const sid = row.session_id || "";
    actions.push(
      `<button class="btn btn--sm" data-rebind data-pane="${esc(row.pane_id)}" data-session="${esc(sid)}">rebind</button>`
    );
  }
  if (!row.live_pane_only && row.session_id) {
    const checked = row.has_session_target ? "checked" : "";
    actions.push(
      `<label class="switch" title="Pin this session as a watched target"><input type="checkbox" data-session-toggle data-session="${esc(row.session_id)}" ${checked} /><span class="switch__track"><span class="switch__thumb"></span></span></label>`
    );
  }

  return `<div class="row">
    <div class="row__main">
      <div class="row__title"><span class="row__project">${esc(project)}</span><span class="row__pane">${esc(pane)}</span></div>
      <div class="row__meta">${meta}</div>
    </div>
    <div class="row__actions">${actions.join("")}</div>
  </div>`;
}

// ── Activity ─────────────────────────────────────────────
function renderActivity(entries) {
  const panel = document.getElementById("activity");
  if (!entries || entries.length === 0) {
    panel.innerHTML = `<p class="empty">No activations logged yet.</p>`;
    return;
  }
  panel.innerHTML = entries
    .slice()
    .reverse()
    .map(
      (e) => `<div class="log-entry log-entry--${esc(e.outcome)}">
        <span class="log-entry__dot">${icon("i-dot")}</span>
        <div class="log-entry__body">${esc(e.text)}</div>
        <span class="log-entry__at">${esc(e.at)}</span>
      </div>`
    )
    .join("");
}

// ── Poll + wire ──────────────────────────────────────────
async function refresh() {
  try {
    const [snap, log] = await Promise.all([
      invoke("get_status"),
      invoke("get_activation_log", { limit: LOG_LIMIT }),
    ]);
    renderVerdict(snap);
    renderHealth(snap.health);
    renderRoster(snap.sessions);
    renderActivity(log);
    document.getElementById("freshness").textContent = `updated just now`;
  } catch (error) {
    document.getElementById("freshness").textContent = `error: ${error}`;
  }
  maybeOfferSwiftbar();
}

let swiftbarChecked = false;
async function maybeOfferSwiftbar() {
  if (swiftbarChecked) return;
  swiftbarChecked = true;
  try {
    const present = await invoke("swiftbar_present");
    document.getElementById("swiftbar-offer").hidden = !present;
  } catch (_) {
    /* non-fatal */
  }
}

function switchTab(tab) {
  state.tab = tab;
  document.querySelectorAll(".tab").forEach((btn) => {
    btn.classList.toggle("tab--active", btn.dataset.tab === tab);
  });
  document.getElementById("roster").hidden = tab !== "roster";
  document.getElementById("activity").hidden = tab !== "activity";
  document.getElementById("roster-filter").style.visibility = tab === "roster" ? "visible" : "hidden";
}

function wire() {
  document.getElementById("master-switch").addEventListener("change", async (e) => {
    try {
      await invoke("set_master", { enabled: e.target.checked });
    } finally {
      refresh();
    }
  });

  document.getElementById("show-all").addEventListener("change", (e) => {
    state.showAll = e.target.checked;
    invoke("get_status").then((snap) => renderRoster(snap.sessions)).catch(() => {});
  });

  document.querySelectorAll(".tab").forEach((btn) => {
    btn.addEventListener("click", () => switchTab(btn.dataset.tab));
  });

  document.getElementById("roster").addEventListener("click", async (e) => {
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
    }
  });

  document.getElementById("roster").addEventListener("change", async (e) => {
    const toggle = e.target.closest("[data-session-toggle]");
    if (toggle) {
      try {
        await invoke("set_session_enabled", { sessionId: toggle.dataset.session, enabled: toggle.checked });
      } finally {
        refresh();
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

  document.getElementById("swiftbar-remove").addEventListener("click", async () => {
    try {
      await invoke("remove_swiftbar");
    } finally {
      document.getElementById("swiftbar-offer").hidden = true;
    }
  });
}

wire();
refresh();
setInterval(refresh, POLL_MS);
