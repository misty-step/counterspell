//! Library surface for the Counterspell desktop app.
//!
//! The desktop backend (a Tauri v2 shell) is an OBSERVER + CONTROLLER only:
//! it reads the same state the headless `watch --arm` daemon reads and writes
//! only the two stable control surfaces the architecture defines (the global
//! disarm marker and per-session config targets). Enforcement itself stays in
//! the daemon, so protection survives the window closing.
//!
//! Everything here composes the crate's existing pub(crate) primitives; it
//! introduces no new policy. In particular it NEVER shells out to `launchctl`
//! — daemon lifecycle is a deliberately terminal-only concern (see
//! `ARCHITECTURE.md`, "Master Switch And Session Overrides").

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use crate::config::{
    add_target_to_config, config_path, ensure_config_file, remove_session_target_from_config,
    resolve_config, target_rule_from_parts,
};
use crate::defaults::DEFAULT_TARGET_MODEL;
use crate::events::read_recent_records;
use crate::herdr::{
    load_herdr_panes, matching_panes_for_session, pane_id, pane_session_id, HerdrPane,
};
use crate::indicators::watch_arm_daemon_status;
use crate::master;
use crate::model::{Config, TargetRule};
use crate::rebind::{build_report_request, send_report_request};
use crate::remediation::{
    detect_actionable_drift, format_target_match, gate_decision_for_matches, status_state,
    target_for_session,
};
use crate::sessions::discover_recent_sessions;
use crate::store::{load_store, state_path};
use crate::util::{home_dir, human_age, short_session};
use std::path::PathBuf;

/// The single protection question the app answers: "am I protected right now?"
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Verdict {
    /// Enabled, watching, and nothing is drifting.
    Shielded,
    /// A drift is being remediated right now (interrupt/compact/switch/continue chain active).
    Acting,
    /// A drift is detected but cannot be safely remediated; reason in words.
    DriftBlocked { reason: String },
    /// The global master switch is off; the daemon takes no action.
    Disarmed,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Shielded => "SHIELDED",
            Verdict::Acting => "ACTING",
            Verdict::DriftBlocked { .. } => "DRIFT-BLOCKED",
            Verdict::Disarmed => "DISARMED",
        }
    }
}

/// One session row for the roster. Carries the FULL session id (not the
/// display-shortened one) so the per-row controls can act on it.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub session_id: String,
    pub short_session_id: String,
    pub project: String,
    pub cwd: String,
    /// Primary bound pane id, when exactly one pane owns the session; controls
    /// (rebind) act on this. Empty when not-open or ambiguous.
    pub pane_id: String,
    /// All matching pane ids, for display.
    pub panes: String,
    pub agent: String,
    pub state: String,
    pub model: String,
    pub target: String,
    /// Human-readable drift ("claude-fable-5 → claude-opus-4-8"), or None when
    /// on-model or ignored.
    pub drift: Option<String>,
    pub watched: bool,
    /// A live Herdr pane with no matching transcript session yet.
    pub live_pane_only: bool,
    /// True when the pane exists but is not reporting an agent_session, so a
    /// rebind is the fix (counterspell-917).
    pub needs_rebind: bool,
    /// True when config carries an explicit `session_id` target for this
    /// session (the per-session enable toggle state).
    pub has_session_target: bool,
    pub updated: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub total: usize,
    pub watched: usize,
    pub live_panes: usize,
    pub drifting: usize,
}

/// The doctor health strip: both axes of "is Counterspell actually live",
/// plus reachability and freshness, so the dangerous silent state (flag
/// enabled but daemon not scheduled) is never hidden.
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub master_enabled: bool,
    pub marker_path: String,
    /// "scheduled" | "not scheduled" | "not installed" | "unknown".
    pub daemon_status: String,
    pub daemon_scheduled: bool,
    pub herdr_reachable: bool,
    /// Age of the most recent watch-arm log write, e.g. "12s". None when the
    /// daemon has never written a log.
    pub last_tick_age: Option<String>,
    /// The one dangerous combination: flag ENABLED but the daemon will never
    /// actually tick. Callers should warn loudly.
    pub armed_but_idle: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub verdict: Verdict,
    pub summary: Summary,
    pub sessions: Vec<SessionView>,
    pub health: Health,
    pub generated_at: String,
}

/// One activation-log entry, outcome-stamped in plain words.
#[derive(Debug, Clone, Serialize)]
pub struct ActivationEntry {
    pub at: String,
    pub at_unix: i64,
    pub session: String,
    pub pane: String,
    /// The plain-words line, e.g.
    /// "daybook w30:p1 drifted fable→opus — remediation started".
    pub text: String,
    /// "confirmed" | "in-flight" | "blocked" | "detected" | "ignored".
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RebindOutcome {
    pub pane_id: String,
    pub session_id: String,
    pub reported: bool,
}

/// Assemble the full live snapshot: verdict, roster, and health strip.
pub fn status_snapshot() -> Result<StatusSnapshot> {
    let now = Utc::now();
    let config = resolve_config(None, None, None)?;
    let store = load_store(&state_path(None)?)?;
    let sessions = discover_recent_sessions(&config, now)?;

    let (panes, herdr_reachable) = match load_herdr_panes() {
        Ok(panes) => (panes, true),
        Err(_) => (Vec::new(), false),
    };

    let session_targets = explicit_session_targets(&config);
    let mut used_panes = std::collections::BTreeSet::new();
    let mut views = Vec::new();

    for session in &sessions {
        let matching =
            matching_panes_for_session(&session.session_id, session.cwd.as_deref(), &panes);
        for pane in &matching {
            used_panes.insert(pane_id(pane).to_string());
        }
        let target = target_for_session(session, &config);
        let state = store.sessions.get(&session.session_id);
        let gate = gate_decision_for_matches(session, &matching, state, &config, now);
        let drift = target.as_ref().and_then(|target| {
            detect_actionable_drift(session, &target.target_model, state)
                .map(|drift| format!("{} → {}", drift.from, drift.to))
        });
        let pane_id_single = if matching.len() == 1 {
            pane_id(matching[0]).to_string()
        } else {
            String::new()
        };
        views.push(SessionView {
            short_session_id: short_session(&session.session_id),
            session_id: session.session_id.clone(),
            project: session.project.clone(),
            cwd: session.cwd.clone().unwrap_or_else(|| "-".to_string()),
            pane_id: pane_id_single,
            panes: join_pane_ids(&matching),
            agent: join_agents(&matching),
            state: status_state(&matching, &gate),
            model: session
                .latest_model
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            target: target
                .as_ref()
                .map(format_target_match)
                .unwrap_or_else(|| "no-target".to_string()),
            drift,
            watched: target.is_some(),
            live_pane_only: false,
            needs_rebind: false,
            has_session_target: session_targets.contains(&session.session_id),
            updated: human_age(session.last_event_at, now),
        });
    }

    for pane in &panes {
        if pane.agent.as_deref() != Some("claude") || used_panes.contains(pane_id(pane)) {
            continue;
        }
        views.push(SessionView {
            session_id: String::new(),
            short_session_id: format!("pane:{}", pane_id(pane)),
            project: "herdr-live-pane".to_string(),
            cwd: pane
                .cwd
                .clone()
                .or_else(|| pane.foreground_cwd.clone())
                .unwrap_or_else(|| "-".to_string()),
            pane_id: pane_id(pane).to_string(),
            panes: pane_id(pane).to_string(),
            agent: pane.agent.clone().unwrap_or_else(|| "-".to_string()),
            state: pane
                .agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            model: "-".to_string(),
            target: "no-transcript-target".to_string(),
            drift: None,
            watched: false,
            live_pane_only: true,
            needs_rebind: pane_session_id(pane).is_none(),
            has_session_target: false,
            updated: "live".to_string(),
        });
    }

    let marker = master::marker_path(None)?;
    let master_enabled = !master::is_disarmed(&marker);
    let verdict = derive_verdict(master_enabled, &views);
    let health = health(master_enabled, &marker, herdr_reachable)?;

    let summary = Summary {
        total: views.len(),
        watched: views.iter().filter(|view| view.watched).count(),
        live_panes: views.iter().filter(|view| view.live_pane_only).count(),
        drifting: views.iter().filter(|view| view.drift.is_some()).count(),
    };

    Ok(StatusSnapshot {
        verdict,
        summary,
        sessions: views,
        health,
        generated_at: now.to_rfc3339(),
    })
}

/// Flip the global master switch. Flag-file only — never touches launchd, so
/// this is safe to call from a GUI event handler. `true` re-arms (removes the
/// marker), `false` disarms (writes it). Reviving a cold daemon is a terminal
/// action (`counterspell enable`), never a click; the daemon status in
/// [`Health`] surfaces that case.
pub fn set_master(enabled: bool) -> Result<()> {
    let marker = master::marker_path(None)?;
    if enabled {
        master::enable_flag_only(&marker)
    } else {
        master::disable(&marker)
    }
}

/// Per-session override, mirroring the dashboard's `/targets/enable|disable`
/// semantics exactly: enabling writes an explicit `session_id → fable` target;
/// disabling removes it. Independent of the global switch.
pub fn set_session_enabled(session_id: &str, enabled: bool) -> Result<()> {
    let home = home_dir()?;
    let path = config_path(None, &home);
    if enabled {
        ensure_config_file(&path)?;
        let target = target_rule_from_parts(
            Some(session_id.to_string()),
            None,
            None,
            DEFAULT_TARGET_MODEL.to_string(),
        )?;
        add_target_to_config(&path, &target)?;
    } else {
        remove_session_target_from_config(&path, session_id)?;
    }
    Ok(())
}

/// First-class remote rebind (counterspell-917): re-assert a pane's
/// agent-session binding from the app, without needing to run inside that
/// pane. Sends the same `pane.report_agent_session` request the SessionStart
/// hook sends, over the Herdr control socket resolved from the environment
/// (`COUNTERSPELL_HERDR_SOCKET` or `HERDR_SOCKET_PATH`).
pub fn rebind_pane(
    pane_id: &str,
    session_id: &str,
    transcript_path: Option<&str>,
) -> Result<RebindOutcome> {
    let socket = herdr_socket_path()?;
    let seq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("read system clock")?
        .as_nanos() as u64;
    let request = build_report_request(pane_id, session_id, transcript_path, seq);
    let response = send_report_request(&socket, &request)
        .context("send pane.report_agent_session to Herdr")?;
    Ok(RebindOutcome {
        pane_id: pane_id.to_string(),
        session_id: session_id.to_string(),
        reported: response.is_some(),
    })
}

/// The doctor health strip on its own, without assembling the full roster.
/// Cheaper than [`status_snapshot`] when only the health axes are needed.
pub fn health_snapshot() -> Result<Health> {
    let herdr_reachable = load_herdr_panes().is_ok();
    let marker = master::marker_path(None)?;
    let master_enabled = !master::is_disarmed(&marker);
    health(master_enabled, &marker, herdr_reachable)
}

/// Whether the legacy SwiftBar menu-bar plugin is still installed. The native
/// tray icon supersedes it (counterspell-906), so the app offers to remove it.
pub fn swiftbar_plugin_present() -> Result<bool> {
    let home = home_dir()?;
    Ok(crate::indicators::swiftbar_plugin_path(&home).exists())
}

/// Remove the legacy SwiftBar plugin file. Returns whether a file was removed.
/// Touches only Counterspell's own plugin under SwiftBar's Plugins dir.
pub fn remove_swiftbar_plugin() -> Result<bool> {
    let home = home_dir()?;
    let path = crate::indicators::swiftbar_plugin_path(&home);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context(format!("remove SwiftBar plugin {}", path.display())),
    }
}

/// The activation log, newest-last, outcome-stamped in plain words.
pub fn activation_log(limit: usize) -> Result<Vec<ActivationEntry>> {
    let now = Utc::now();
    let records = read_recent_records(limit)?;
    Ok(records
        .into_iter()
        .map(|record| {
            let at = chrono::DateTime::from_timestamp(record.occurred_at_unix, 0)
                .map(|timestamp| human_age(timestamp, now))
                .unwrap_or_else(|| record.occurred_at.clone());
            let outcome = outcome_of(&record.action, &record.action_taken).to_string();
            ActivationEntry {
                text: format_activation(&record),
                at,
                at_unix: record.occurred_at_unix,
                session: short_session(&record.session_id),
                pane: record.pane.clone(),
                outcome,
            }
        })
        .collect())
}

fn herdr_socket_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("COUNTERSPELL_HERDR_SOCKET") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH") {
        return Ok(PathBuf::from(path));
    }
    anyhow::bail!(
        "no Herdr control socket found: set COUNTERSPELL_HERDR_SOCKET or launch the app from a \
         Herdr-managed environment that exports HERDR_SOCKET_PATH"
    )
}

fn health(master_enabled: bool, marker: &std::path::Path, herdr_reachable: bool) -> Result<Health> {
    let home = home_dir()?;
    let daemon = watch_arm_daemon_status(&home)
        .map(|status| status.label().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let daemon_scheduled = daemon == "scheduled";
    let last_tick_age = watch_arm_last_tick_age(&home);
    Ok(Health {
        master_enabled,
        marker_path: marker.display().to_string(),
        daemon_status: daemon,
        daemon_scheduled,
        herdr_reachable,
        last_tick_age,
        armed_but_idle: master_enabled && !daemon_scheduled,
    })
}

fn watch_arm_last_tick_age(home: &std::path::Path) -> Option<String> {
    let log = home
        .join("Library")
        .join("Logs")
        .join("counterspell-watch-arm.log");
    let modified = std::fs::metadata(&log).ok()?.modified().ok()?;
    let modified: chrono::DateTime<Utc> = modified.into();
    Some(human_age(modified, Utc::now()))
}

fn explicit_session_targets(config: &Config) -> std::collections::BTreeSet<String> {
    config
        .targets
        .iter()
        .filter_map(|target: &TargetRule| target.session_id.clone())
        .collect()
}

fn join_pane_ids(panes: &[&HerdrPane]) -> String {
    if panes.is_empty() {
        "not-open".to_string()
    } else {
        panes
            .iter()
            .map(|pane| pane_id(pane))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn join_agents(panes: &[&HerdrPane]) -> String {
    let agents = panes
        .iter()
        .filter_map(|pane| pane.agent.as_deref())
        .collect::<Vec<_>>();
    if agents.is_empty() {
        "-".to_string()
    } else {
        agents.join(", ")
    }
}

/// Pure verdict derivation over the assembled roster. Kept free of I/O so it
/// is unit-testable against synthetic rows.
pub(crate) fn derive_verdict(master_enabled: bool, views: &[SessionView]) -> Verdict {
    if !master_enabled {
        return Verdict::Disarmed;
    }
    // Only LIVE drift drives the headline: a watched session drifting on an
    // actual bound pane. A session that drifted and then closed (no pane) is
    // history under "show all", not a current protection concern.
    let drifting: Vec<&SessionView> = views
        .iter()
        .filter(|view| view.watched && view.drift.is_some() && view.panes != "not-open")
        .collect();
    if drifting.is_empty() {
        return Verdict::Shielded;
    }
    // In-flight remediation wins the headline: a compact/switch/continue
    // chain is active on at least one drifted session.
    if drifting.iter().any(|view| is_in_flight_state(&view.state)) {
        return Verdict::Acting;
    }
    // A drift that cannot be safely acted on (ambiguous pane, no pane, etc.)
    // is the honest alarm state.
    if let Some(blocked) = drifting.iter().find(|view| is_blocked_state(&view.state)) {
        return Verdict::DriftBlocked {
            reason: humanize_block(&blocked.state),
        };
    }
    // Drift detected on an actionable pane: the next daemon tick remediates.
    Verdict::Acting
}

fn is_blocked_state(state: &str) -> bool {
    !(state == "idle" || state == "not-open" || state == "live" || is_in_flight_state(state))
}

fn is_in_flight_state(state: &str) -> bool {
    state.contains("compact-pending")
        || state.contains("remediation-in-flight")
        || state.contains("remediation-timed-out")
}

fn humanize_block(state: &str) -> String {
    if state.starts_with("ambiguous-pane") {
        "multiple panes share this session — can't safely target one".to_string()
    } else if state == "no-pane" || state == "not-open" {
        "no live pane is bound to this session".to_string()
    } else if state == "transcript-active" {
        "the session is actively working; waiting for a safe boundary".to_string()
    } else if state == "debounce" {
        "just acted; waiting out the debounce window".to_string()
    } else if state.starts_with("remediation-in-flight") {
        "remediation is already in flight".to_string()
    } else if state.starts_with("remediation-timed-out") {
        "remediation timed out; retrying from recorded state".to_string()
    } else if state.starts_with("pane-") {
        "the pane is busy (a prompt may be open)".to_string()
    } else {
        state.to_string()
    }
}

fn outcome_of(action: &str, action_taken: &str) -> &'static str {
    match action {
        "remediation_confirmed" => "confirmed",
        "model_switched" => "confirmed",
        "compact_sent"
        | "compact_queued"
        | "interrupt_sent"
        | "continue_sent"
        | "remediation_recovery" => "in-flight",
        "session_ignored" => "ignored",
        _ => {
            if action_taken == "blocked" {
                "blocked"
            } else if action_taken.starts_with("remediation") {
                "in-flight"
            } else {
                "detected"
            }
        }
    }
}

/// Turn a raw record into one plain-words line. Model names are shortened to
/// their family (fable/opus/sonnet/haiku) so the log reads like a sentence,
/// not an id dump.
pub(crate) fn format_activation(record: &crate::events::ActivationRecord) -> String {
    let project = "";
    let _ = project;
    let from = model_family(&record.from_model);
    let to = model_family(&record.to_model);
    let pane = if record.pane == "not-open" {
        String::new()
    } else {
        format!("{} ", record.pane)
    };
    match record.action.as_str() {
        "model_drift_detected" => {
            let tail = match record.action_taken.as_str() {
                "blocked" => format!("blocked — {}", record.gate),
                "none" => "no action needed".to_string(),
                "remediation-started" => "remediation started".to_string(),
                other if other.starts_with("dry-run") => "detected (dry-run)".to_string(),
                _ => "detected".to_string(),
            };
            format!("{pane}drifted {from}→{to} — {tail}")
        }
        "compact_queued" => format!("{pane}queued a compact to hand off before switching"),
        "compact_sent" => format!("{pane}sent a compact before switching back to {to}"),
        "interrupt_sent" => format!("{pane}interrupted the drifted turn"),
        "continue_sent" => format!("{pane}sent continue after switching"),
        "remediation_recovery" => format!("{pane}recovering timed-out remediation"),
        "model_switched" => format!("{pane}switched back to {to} ✓"),
        "remediation_confirmed" => format!("{pane}back on {to} ✓"),
        "session_ignored" => format!(
            "{pane}ignored — never ran {}",
            model_family(&record.to_model)
        ),
        other => format!("{pane}{other}"),
    }
}

fn model_family(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    for family in ["fable", "opus", "sonnet", "haiku", "mythos"] {
        if lower.contains(family) {
            return family.to_string();
        }
    }
    if model.is_empty() || model == "none" {
        "?".to_string()
    } else {
        model.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(watched: bool, drift: Option<&str>, state: &str) -> SessionView {
        SessionView {
            session_id: "sess-abcdef".to_string(),
            short_session_id: "sess-ab".to_string(),
            project: "daybook".to_string(),
            cwd: "/tmp".to_string(),
            pane_id: "w1:p1".to_string(),
            panes: "w1:p1".to_string(),
            agent: "claude".to_string(),
            state: state.to_string(),
            model: "claude-opus-4-8".to_string(),
            target: "auto-fable".to_string(),
            drift: drift.map(str::to_string),
            watched,
            live_pane_only: false,
            needs_rebind: false,
            has_session_target: false,
            updated: "1m".to_string(),
        }
    }

    #[test]
    fn disarmed_wins_regardless_of_drift() {
        let views = vec![view(true, Some("fable → opus"), "idle")];
        assert_eq!(derive_verdict(false, &views), Verdict::Disarmed);
    }

    #[test]
    fn no_drift_is_shielded() {
        let views = vec![view(true, None, "idle"), view(false, Some("x → y"), "idle")];
        assert_eq!(derive_verdict(true, &views), Verdict::Shielded);
    }

    #[test]
    fn compact_pending_is_acting() {
        let views = vec![view(true, Some("fable → opus"), "compact-pending")];
        assert_eq!(derive_verdict(true, &views), Verdict::Acting);
    }

    #[test]
    fn blocked_drift_reports_reason() {
        let views = vec![view(true, Some("fable → opus"), "ambiguous-pane:2")];
        match derive_verdict(true, &views) {
            Verdict::DriftBlocked { reason } => assert!(reason.contains("multiple panes")),
            other => panic!("expected drift-blocked, got {other:?}"),
        }
    }

    #[test]
    fn actionable_drift_is_acting() {
        let views = vec![view(true, Some("fable → opus"), "idle")];
        assert_eq!(derive_verdict(true, &views), Verdict::Acting);
    }

    #[test]
    fn format_activation_reads_as_plain_words() {
        let record = crate::events::ActivationRecord {
            occurred_at: "2026-07-07T18:36:00Z".to_string(),
            occurred_at_unix: 0,
            session_id: "sess-1".to_string(),
            pane: "w30:p1".to_string(),
            from_model: "claude-fable-5".to_string(),
            to_model: "claude-opus-4-8".to_string(),
            gate: "allow".to_string(),
            action: "model_drift_detected".to_string(),
            action_taken: "remediation-started".to_string(),
            origin: "downgraded-from-fable".to_string(),
        };
        let text = format_activation(&record);
        assert_eq!(text, "w30:p1 drifted fable→opus — remediation started");
    }

    #[test]
    fn format_activation_switch_confirms() {
        let record = crate::events::ActivationRecord {
            occurred_at: "2026-07-07T18:38:00Z".to_string(),
            occurred_at_unix: 0,
            session_id: "sess-1".to_string(),
            pane: "w30:p1".to_string(),
            from_model: "claude-fable-5".to_string(),
            to_model: "claude-fable-5".to_string(),
            gate: "allow".to_string(),
            action: "model_switched".to_string(),
            action_taken: "model_switched:claude-fable-5".to_string(),
            origin: "downgraded-from-fable".to_string(),
        };
        assert_eq!(
            format_activation(&record),
            "w30:p1 switched back to fable ✓"
        );
        assert_eq!(
            outcome_of("model_switched", "model_switched:claude-fable-5"),
            "confirmed"
        );
    }
}
