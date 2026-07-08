// Mocked Tauri IPC for headless QA (seed for counterspell-922 fixture mode).
(() => {
  const q = new URLSearchParams(location.search);
  const disarmed = q.get("state") === "disarmed";
  const theme = q.get("theme") || "light";
  document.documentElement.setAttribute("data-theme", theme);

  const sessions = [
    { session_id: "a82c5ce5", project: "conviction", cwd: "/Users/phaedrus/Development/conviction", panes: "w4A:p2", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "working", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "42c23bd2", project: "bitterblossom", cwd: "/Users/phaedrus/Development/bitterblossom", panes: "w49:p2", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "working · stale", watched: true, drift: null, needs_rebind: true, pane_id: "w49:p2", has_session_target: false, live_pane_only: false },
    { session_id: "186911a7", project: "daybook", cwd: "/Users/phaedrus/Documents/daybook", panes: "w30:p1", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "pane-working", watched: true, drift: null, needs_rebind: false, has_session_target: true, live_pane_only: false },
    { session_id: "02f69262", project: "daybook", cwd: "/Users/phaedrus/Documents/daybook", panes: "w4G:p1", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "pane-working", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "8e4a03c7", project: "ol-165", cwd: "/Users/phaedrus/Development/r90/.worktrees/ol-165", panes: "w3P:p1", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "idle", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "4351a99b", project: "mint", cwd: "/Users/phaedrus/Development/mint", panes: "w44:p2", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "idle", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "c3f3387e", project: "crucible", cwd: "/Users/phaedrus/Development/crucible", panes: "w47:p2", model: "claude-fable-5", target: "claude-fable-5 (auto:fable)", state: "working", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "3cce18ed", project: "sanctum", cwd: "/Users/phaedrus/Development/sanctum", panes: "not-open", model: "claude-opus-4-8", target: "claude-fable-5 (auto:fable)", state: "no-pane", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
    { session_id: "59145ef6", project: "simons", cwd: "/Users/phaedrus/Development/simons", panes: "not-open", model: "claude-opus-4-8", target: "claude-fable-5 (auto:fable)", state: "no-pane", watched: true, drift: null, needs_rebind: false, has_session_target: false, live_pane_only: false },
  ];

  const snap = {
    verdict: disarmed ? { kind: "disarmed" } : { kind: "shielded" },
    summary: { total: 44, watched: 29, ignored: 15, mapped: 9 },
    health: {
      master_enabled: !disarmed,
      armed_but_idle: false,
      daemon_scheduled: true,
      daemon_status: "scheduled",
      last_tick_age: "1s ago",
      herdr_reachable: true,
    },
    sessions,
  };

  const log = [
    { outcome: "ignored", at: "17:52", text: "simons — session disarmed by operator, no action taken" },
    { outcome: "blocked", at: "18:11", text: "sanctum w2C:p1 drift detected fable→opus-4-8 — waiting for turn to end" },
    { outcome: "confirmed", at: "18:36", text: "daybook w30:p1 drifted fable→opus-4-8 — interrupted, compact queued, switched back ✓ (2m14s)" },
    { outcome: "in-flight", at: "18:39", text: "mint w44:p2 drifted fable→opus-4-8 — compact queued, waiting to switch" },
  ];

  window.__TAURI__ = { core: { invoke: async (cmd) => {
    if (cmd === "get_status") return snap;
    if (cmd === "get_activation_log") return log;
    if (cmd === "swiftbar_present") return true;
    return null;
  } } };

  addEventListener("load", () => {
    const view = q.get("view");
    if (view) setTimeout(() => document.querySelector(`.rl-item[data-view="${view}"]`)?.click(), 200);
  });
})();
