use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};

use crate::defaults::{
    COMPACT_COMMAND, CONTINUE_COMMAND, DEFAULT_TARGET_MODEL, HERDR_WAIT_MARGIN_MS,
    INTERRUPT_WAIT_TIMEOUT_MS, MODEL_SWITCH_CONFIRM_DELAY_MS, REMEDIATION_CHAIN_TIMEOUT_SECONDS,
};
use crate::herdr::{pane_session_id, run_herdr_args, run_herdr_args_with_timeout, HerdrPane};
use crate::model::{
    Config, GateBlocker, GateDecision, ModelDrift, PlannedAction, RemediationChainState,
    RemediationPlan, RemediationStep, SessionState, TargetMatch, TranscriptSession,
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
            }
            PlannedAction::Interrupt => {
                // Escape ends the current turn (interrupt, not kill).
                run_herdr_args(&["pane", "send-keys", pane_id, "escape"])
                    .with_context(|| format!("send escape to Herdr pane {pane_id}"))?;
                // Best-effort pause so the compact executes immediately
                // after interruption. IGNORED on failure: the durable chain
                // state blocks duplicate compacts while Herdr status catches
                // up or until an explicit timeout recovery path takes over.
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
            PlannedAction::Continue => {
                run_herdr_args(&["pane", "run", pane_id, CONTINUE_COMMAND])
                    .with_context(|| format!("send continue command to Herdr pane {pane_id}"))?;
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

    if let Some(target) = &target {
        if let Some(chain) = active_chain(state, &target.target_model) {
            if chain_has_completion_evidence(session, &chain) {
                return RemediationPlan {
                    gate: GateDecision {
                        blockers: Vec::new(),
                    },
                    actions: Vec::new(),
                    recovery_reason: None,
                };
            }

            if let Some(actions) = next_actions_for_chain_progress(session, &chain) {
                let gate = chain_recovery_gate(session, matching_panes);
                return RemediationPlan {
                    actions: if gate.is_allowed() {
                        actions
                    } else {
                        Vec::new()
                    },
                    gate,
                    recovery_reason: None,
                };
            }

            if !chain_timed_out(&chain, now) {
                return RemediationPlan {
                    gate: GateDecision {
                        blockers: vec![GateBlocker::RemediationInFlight(chain.step)],
                    },
                    actions: Vec::new(),
                    recovery_reason: None,
                };
            }

            let gate = chain_recovery_gate(session, matching_panes);
            if !gate.is_allowed() {
                return RemediationPlan {
                    gate: GateDecision {
                        blockers: gate
                            .blockers
                            .into_iter()
                            .chain(std::iter::once(GateBlocker::RemediationTimedOut(
                                chain.step,
                            )))
                            .collect(),
                    },
                    actions: Vec::new(),
                    recovery_reason: Some(format!("timeout:{}", chain.step.label())),
                };
            }

            let actions = recovery_actions(session, &chain);
            return RemediationPlan {
                gate,
                actions,
                recovery_reason: Some(format!("timeout:{}", chain.step.label())),
            };
        }
    }

    let mut gate = gate_decision_for_matches(session, matching_panes, state, config, now);
    let actions = if target.is_some() {
        if drift.is_some() {
            let working_bound = matching_panes.first().is_some_and(|pane| {
                matching_panes.len() == 1
                    && pane_session_id(pane) == Some(session.session_id.as_str())
                    && pane.agent_status.as_deref() == Some("working")
                    && !is_debounced(state, config, now)
            });
            if working_bound {
                gate = GateDecision {
                    blockers: Vec::new(),
                };
                vec![PlannedAction::Interrupt, PlannedAction::Compact]
            } else if gate.is_allowed() {
                vec![PlannedAction::Compact]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    RemediationPlan {
        gate,
        actions,
        recovery_reason: None,
    }
}

fn active_chain(state: Option<&SessionState>, target_model: &str) -> Option<RemediationChainState> {
    let state = state?;
    if let Some(chain) = &state.remediation_chain {
        return Some(chain.clone());
    }
    state
        .pending_compact_unix
        .map(|queued_at| RemediationChainState {
            target_model: target_model.to_string(),
            started_unix: queued_at,
            step_sent_unix: queued_at,
            step: RemediationStep::Compact,
            recovery_reason: Some("legacy-pending-compact".to_string()),
        })
}

pub(crate) fn chain_has_completion_evidence(
    session: &TranscriptSession,
    chain: &RemediationChainState,
) -> bool {
    chain.step == RemediationStep::Continue
        && compact_summary_after_chain_start(session, chain)
        && target_model_after_chain_start(session, &chain.target_model, chain)
}

pub(crate) fn compact_summary_after_chain_start(
    session: &TranscriptSession,
    chain: &RemediationChainState,
) -> bool {
    session
        .latest_compact_at
        .and_then(|at| unix_to_utc(chain.started_unix).map(|started| at >= started))
        .unwrap_or(false)
}

fn target_model_after_chain_start(
    session: &TranscriptSession,
    target_model: &str,
    chain: &RemediationChainState,
) -> bool {
    session.latest_model.as_deref() == Some(target_model)
        && session
            .latest_model_at
            .and_then(|at| unix_to_utc(chain.started_unix).map(|started| at >= started))
            .unwrap_or(false)
}

fn chain_timed_out(chain: &RemediationChainState, now: DateTime<Utc>) -> bool {
    unix_to_utc(chain.step_sent_unix)
        .is_some_and(|sent_at| now - sent_at >= Duration::seconds(chain_timeout_seconds() as i64))
}

pub(crate) fn chain_timeout_seconds() -> u64 {
    std::env::var("COUNTERSPELL_REMEDIATION_CHAIN_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(REMEDIATION_CHAIN_TIMEOUT_SECONDS)
}

fn chain_recovery_gate(session: &TranscriptSession, matching_panes: &[&HerdrPane]) -> GateDecision {
    let mut blockers = Vec::new();
    match matching_panes {
        [] => blockers.push(GateBlocker::NoPane),
        [pane] if pane_session_id(pane) == Some(session.session_id.as_str()) => {
            if pane.agent_status.as_deref() == Some("blocked") {
                blockers.push(GateBlocker::PaneBusy("blocked".to_string()));
            }
        }
        [pane] => blockers.push(GateBlocker::PaneBusy(
            pane.agent_status
                .clone()
                .unwrap_or_else(|| "unbound".to_string()),
        )),
        panes => blockers.push(GateBlocker::AmbiguousPane(panes.len())),
    }
    GateDecision { blockers }
}

fn recovery_actions(
    session: &TranscriptSession,
    chain: &RemediationChainState,
) -> Vec<PlannedAction> {
    if compact_summary_after_chain_start(session, chain) {
        return vec![
            PlannedAction::SwitchModel(chain.target_model.clone()),
            PlannedAction::Continue,
        ];
    }

    match chain.step {
        RemediationStep::Interrupt => vec![PlannedAction::Compact],
        RemediationStep::Compact | RemediationStep::Switch | RemediationStep::Continue => {
            vec![PlannedAction::Interrupt, PlannedAction::Compact]
        }
    }
}

fn next_actions_for_chain_progress(
    session: &TranscriptSession,
    chain: &RemediationChainState,
) -> Option<Vec<PlannedAction>> {
    match chain.step {
        RemediationStep::Interrupt => Some(vec![PlannedAction::Compact]),
        RemediationStep::Compact => compact_summary_after_chain_start(session, chain).then(|| {
            vec![
                PlannedAction::SwitchModel(chain.target_model.clone()),
                PlannedAction::Continue,
            ]
        }),
        RemediationStep::Switch => Some(vec![PlannedAction::Continue]),
        RemediationStep::Continue => None,
    }
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

    if let Some(chain) = state.and_then(|state| state.remediation_chain.as_ref()) {
        if chain_has_completion_evidence(session, chain) {
            return GateDecision { blockers };
        }
        if chain_timed_out(chain, now) {
            blockers.push(GateBlocker::RemediationTimedOut(chain.step));
        } else {
            blockers.push(GateBlocker::RemediationInFlight(chain.step));
        }
        return GateDecision { blockers };
    }

    if state
        .and_then(|state| state.pending_compact_unix)
        .and_then(unix_to_utc)
        .is_some_and(|queued_at| {
            now - queued_at < Duration::seconds(chain_timeout_seconds() as i64)
        })
    {
        blockers.push(GateBlocker::CompactPending);
        return GateDecision { blockers };
    }

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
            GateBlocker::RemediationInFlight(step) => {
                format!("remediation-in-flight:{}", step.label())
            }
            GateBlocker::RemediationTimedOut(step) => {
                format!("remediation-timed-out:{}", step.label())
            }
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
            PlannedAction::SwitchModel(model) => format!("switch:{model}"),
            PlannedAction::Continue => "continue".to_string(),
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
