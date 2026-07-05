use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};

use crate::defaults::{
    COMPACT_COMMAND, COMPACT_WAIT_TIMEOUT_MS, DEFAULT_TARGET_MODEL, HERDR_WAIT_MARGIN_MS,
    INTERRUPT_WAIT_TIMEOUT_MS, MODEL_SWITCH_CONFIRM_DELAY_MS, PENDING_COMPACT_EXPIRY_SECONDS,
};
use crate::herdr::{pane_session_id, run_herdr_args, run_herdr_args_with_timeout, HerdrPane};
use crate::model::{
    Config, GateBlocker, GateDecision, ModelDrift, PlannedAction, RemediationPlan, SessionState,
    TargetMatch, TranscriptSession,
};
use crate::sessions::is_model_sentinel;
use crate::util::unix_to_utc;

pub(crate) const AUTO_FABLE_REASON: &str = "auto:fable";

pub(crate) fn execute_remediation(pane_id: &str, actions: &[PlannedAction]) -> Result<()> {
    for action in actions {
        match action {
            PlannedAction::Compact => {
                run_herdr_args(&["pane", "run", pane_id, COMPACT_COMMAND])
                    .with_context(|| format!("send compact command to Herdr pane {pane_id}"))?;
                // Best-effort pacing, IGNORED on failure: panes launched via
                // `herdr agent start` settle to `done`, not `idle`, so this
                // wait can time out even though compact finished. Aborting
                // here would strand the session downgraded with compact spent
                // (the 2026-07-04 failure class); proceeding is queue-safe —
                // the /model typed next queues FIFO and executes post-compact,
                // landing dialog-free on the small context.
                let _ = run_herdr_args_with_timeout(
                    &[
                        "wait",
                        "agent-status",
                        pane_id,
                        "--status",
                        "idle",
                        "--timeout",
                        &COMPACT_WAIT_TIMEOUT_MS.to_string(),
                    ],
                    wait_subprocess_timeout(COMPACT_WAIT_TIMEOUT_MS),
                );
            }
            PlannedAction::Interrupt => {
                // Escape ends the current turn (interrupt, not kill).
                run_herdr_args(&["pane", "send-keys", pane_id, "escape"])
                    .with_context(|| format!("send escape to Herdr pane {pane_id}"))?;
                // Best-effort pause so the queued compact executes right
                // away instead of at the end of a resumed turn. IGNORED on
                // failure: herdr's agent-status lags interrupts, and the
                // rest of the chain is queue-safe regardless — aborting
                // here is what left a session downgraded behind a stuck
                // in-flight marker on 2026-07-04.
                let _ = run_herdr_args_with_timeout(
                    &[
                        "wait",
                        "agent-status",
                        pane_id,
                        "--status",
                        "idle",
                        "--timeout",
                        &INTERRUPT_WAIT_TIMEOUT_MS.to_string(),
                    ],
                    wait_subprocess_timeout(INTERRUPT_WAIT_TIMEOUT_MS),
                );
            }
            PlannedAction::QueueCompact => {
                // No wait afterward, by design: the switch typed behind
                // this queues FIFO and executes post-compact.
                run_herdr_args(&["pane", "run", pane_id, COMPACT_COMMAND])
                    .with_context(|| format!("queue compact command into Herdr pane {pane_id}"))?;
            }
            PlannedAction::SwitchModel(model) => {
                let command = format!("/model {model}");
                run_herdr_args(&["pane", "run", pane_id, command.as_str()])
                    .with_context(|| format!("send model switch to Herdr pane {pane_id}"))?;
                let delay = model_switch_confirm_delay();
                if delay > std::time::Duration::ZERO {
                    std::thread::sleep(delay);
                }
                run_herdr_args(&["pane", "send-keys", pane_id, "enter"])
                    .with_context(|| format!("confirm model switch in Herdr pane {pane_id}"))?;
            }
        }
    }

    Ok(())
}

fn wait_subprocess_timeout(wait_ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis(wait_ms + HERDR_WAIT_MARGIN_MS)
}

fn model_switch_confirm_delay() -> std::time::Duration {
    let delay_ms = std::env::var("COUNTERSPELL_MODEL_SWITCH_CONFIRM_DELAY_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(MODEL_SWITCH_CONFIRM_DELAY_MS);
    std::time::Duration::from_millis(delay_ms)
}

pub(crate) fn remediation_plan(
    session: &TranscriptSession,
    matching_panes: &[&HerdrPane],
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> RemediationPlan {
    let target = target_for_session(session, config);
    let drift = target
        .as_ref()
        .and_then(|target| detect_actionable_drift(session, &target.target_model, state));

    // Fast path: a downgraded session must not keep working on the wrong
    // model, and remediation must not depend on ever SAMPLING the pane idle
    // — a busy lead session with queued teammate messages is never
    // observably idle, which is exactly how the 2026-07-04 switch was lost
    // (compact fired, every subsequent tick sampled `working`, /model never
    // went out). Instead the whole chain runs synchronously in one pass:
    // Escape ends the current turn, /compact runs on the confirmed-idle
    // pane, and /model goes out right behind it — if a queued message
    // steals the pane first, both inputs queue FIFO and execute in order,
    // /model landing dialog-free on the small post-compact context. All of
    // it requires the pane to be bound to this exact session id — never a
    // cwd guess.
    if let (Some(target), Some(_)) = (&target, &drift) {
        if let [pane] = matching_panes {
            if pane_session_id(pane) == Some(session.session_id.as_str())
                && pane.agent_status.as_deref() == Some("working")
            {
                if has_pending_compact(state, now) {
                    // A chain is (or may be) already in flight — persisted
                    // before it started typing. Never double-Escape.
                    return RemediationPlan {
                        gate: GateDecision {
                            blockers: vec![GateBlocker::CompactPending],
                        },
                        actions: Vec::new(),
                    };
                }
                if !is_debounced(state, config, now) {
                    return RemediationPlan {
                        gate: GateDecision {
                            blockers: Vec::new(),
                        },
                        actions: vec![
                            PlannedAction::Interrupt,
                            PlannedAction::QueueCompact,
                            PlannedAction::SwitchModel(target.target_model.clone()),
                        ],
                    };
                }
            }
        }
    }

    let gate = gate_decision_for_matches(session, matching_panes, state, config, now);
    let actions = if let Some(target) = target {
        if drift.is_some() && gate.is_allowed() {
            vec![
                PlannedAction::Compact,
                PlannedAction::SwitchModel(target.target_model),
            ]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    RemediationPlan { gate, actions }
}

fn has_pending_compact(state: Option<&SessionState>, now: DateTime<Utc>) -> bool {
    state
        .and_then(|state| state.pending_compact_unix)
        .and_then(unix_to_utc)
        .is_some_and(|queued_at| {
            now - queued_at < Duration::seconds(PENDING_COMPACT_EXPIRY_SECONDS as i64)
        })
}

fn is_debounced(state: Option<&SessionState>, config: &Config, now: DateTime<Utc>) -> bool {
    state
        .and_then(|state| state.last_action_unix)
        .and_then(unix_to_utc)
        .is_some_and(|last_action_at| {
            now - last_action_at < Duration::seconds(config.debounce_seconds as i64)
        })
}

pub(crate) fn target_for_session(
    session: &TranscriptSession,
    config: &Config,
) -> Option<TargetMatch> {
    if has_auto_fable_history(session) {
        return Some(TargetMatch {
            target_model: DEFAULT_TARGET_MODEL.to_string(),
            reason: AUTO_FABLE_REASON.to_string(),
        });
    }

    for target in &config.targets {
        if target
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id == session.session_id)
        {
            return Some(TargetMatch {
                target_model: target.target_model.clone(),
                reason: "session_id".to_string(),
            });
        }

        if target
            .project_pattern
            .as_deref()
            .is_some_and(|pattern| wildcard_match(pattern, &session.project))
        {
            return Some(TargetMatch {
                target_model: target.target_model.clone(),
                reason: format!("project:{}", target.project_pattern.as_deref().unwrap()),
            });
        }

        if let Some(cwd) = session.cwd.as_deref() {
            if target
                .cwd_pattern
                .as_deref()
                .is_some_and(|pattern| wildcard_match(pattern, cwd))
            {
                return Some(TargetMatch {
                    target_model: target.target_model.clone(),
                    reason: format!("cwd:{}", target.cwd_pattern.as_deref().unwrap()),
                });
            }
        }
    }

    None
}

pub(crate) fn has_auto_fable_history(session: &TranscriptSession) -> bool {
    session
        .model_history
        .iter()
        .any(|model| model == DEFAULT_TARGET_MODEL)
}

pub(crate) fn is_auto_fable_target(target: &TargetMatch) -> bool {
    target.reason == AUTO_FABLE_REASON
}

pub(crate) fn format_target_match(target: &TargetMatch) -> String {
    format!("{} ({})", target.target_model, target.reason)
}

pub(crate) fn detect_drift(session: &TranscriptSession, desired_model: &str) -> Option<ModelDrift> {
    let desired_model = desired_model.trim();
    if desired_model.is_empty() || is_model_sentinel(desired_model) {
        return None;
    }

    let latest = latest_real_model(session)?;
    if latest == desired_model {
        return None;
    }

    let (from, to) = if session
        .model_history
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty() && !is_model_sentinel(model))
        .any(|model| model == desired_model)
    {
        (desired_model.to_string(), latest.to_string())
    } else {
        (latest.to_string(), desired_model.to_string())
    };

    Some(ModelDrift { from, to })
}

fn latest_real_model(session: &TranscriptSession) -> Option<&str> {
    session
        .latest_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty() && !is_model_sentinel(model))
        .or_else(|| {
            session
                .model_history
                .iter()
                .rev()
                .map(String::as_str)
                .map(str::trim)
                .find(|model| !model.is_empty() && !is_model_sentinel(model))
        })
}

pub(crate) fn detect_actionable_drift(
    session: &TranscriptSession,
    desired_model: &str,
    state: Option<&SessionState>,
) -> Option<ModelDrift> {
    let drift = detect_drift(session, desired_model)?;
    if switch_recorded_after_latest_model(session, state) {
        return None;
    }
    Some(drift)
}

fn switch_recorded_after_latest_model(
    session: &TranscriptSession,
    state: Option<&SessionState>,
) -> bool {
    let Some(latest_model_at) = session.latest_model_at else {
        return false;
    };
    state
        .and_then(|state| state.last_action_unix)
        .and_then(unix_to_utc)
        .is_some_and(|last_action_at| last_action_at >= latest_model_at)
}

pub(crate) fn gate_decision_for_matches(
    session: &TranscriptSession,
    matching_panes: &[&HerdrPane],
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> GateDecision {
    let mut blockers = Vec::new();

    if now - session.last_event_at < Duration::seconds(config.transcript_quiet_seconds as i64) {
        blockers.push(GateBlocker::TranscriptActive);
    }

    match matching_panes {
        [] => blockers.push(GateBlocker::NoPane),
        // `done` is herdr's settled state for `herdr agent start` panes whose
        // turn finished: the TUI sits at the prompt awaiting input, exactly as
        // injectable as `idle`. Treating it as busy left managed lanes
        // permanently unremediated (probe pane w3H:p2, 2026-07-04).
        [pane] if matches!(pane.agent_status.as_deref(), Some("idle" | "done")) => {}
        [pane] => blockers.push(GateBlocker::PaneBusy(
            pane.agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )),
        // Keystroke injection must never guess between panes. Session-id
        // binding (matching_panes_for_session) is the disambiguator; if it
        // still yields more than one pane, block.
        panes => blockers.push(GateBlocker::AmbiguousPane(panes.len())),
    }

    if is_debounced(state, config, now) {
        blockers.push(GateBlocker::Debounce);
    }

    GateDecision { blockers }
}

pub(crate) fn status_state(panes: &[&HerdrPane], gate: &GateDecision) -> String {
    if panes.is_empty() {
        return "not-open".to_string();
    }
    if gate.is_allowed() {
        return "idle".to_string();
    }
    describe_gate(gate)
}

pub(crate) fn describe_gate(gate: &GateDecision) -> String {
    if gate.is_allowed() {
        return "allowed".to_string();
    }

    gate.blockers
        .iter()
        .map(|blocker| match blocker {
            GateBlocker::NoPane => "no-pane".to_string(),
            GateBlocker::AmbiguousPane(count) => format!("ambiguous-pane:{count}"),
            GateBlocker::TranscriptActive => "transcript-active".to_string(),
            GateBlocker::PaneBusy(state) => format!("pane-{state}"),
            GateBlocker::Debounce => "debounce".to_string(),
            GateBlocker::CompactPending => "compact-pending".to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn describe_actions(actions: &[PlannedAction]) -> String {
    if actions.is_empty() {
        return "-".to_string();
    }

    actions
        .iter()
        .map(|action| match action {
            PlannedAction::Compact => "compact".to_string(),
            PlannedAction::Interrupt => "interrupt".to_string(),
            PlannedAction::QueueCompact => "queue-compact".to_string(),
            PlannedAction::SwitchModel(model) => format!("switch:{model}"),
        })
        .collect::<Vec<_>>()
        .join(" then ")
}

pub(crate) fn describe_watch_actions(actions: &[PlannedAction], arm: bool) -> String {
    if actions.is_empty() {
        return "-".to_string();
    }

    let actions = describe_actions(actions);
    if arm {
        actions
    } else {
        format!("dry-run:{actions}")
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut remaining = value;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        let Some(stripped) = remaining.strip_prefix(first) else {
            return false;
        };
        remaining = stripped;
    }

    for part in parts
        .iter()
        .skip(1)
        .take(parts.len().saturating_sub(2))
        .filter(|part| !part.is_empty())
    {
        let Some(index) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[index + part.len()..];
    }

    if let Some(last) = parts.last().filter(|part| !part.is_empty()) {
        remaining.ends_with(last)
    } else {
        true
    }
}
