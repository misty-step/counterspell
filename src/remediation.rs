use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};

use crate::defaults::{COMPACT_COMMAND, COMPACT_WAIT_TIMEOUT_MS, DEFAULT_TARGET_MODEL};
use crate::herdr::{pane_id, run_herdr_args, HerdrPane};
use crate::model::{
    Config, GateBlocker, GateDecision, ModelDrift, PlannedAction, RemediationPlan, SessionState,
    TargetMatch, TranscriptSession,
};
use crate::util::unix_to_utc;

pub(crate) const AUTO_FABLE_REASON: &str = "auto:fable";

pub(crate) fn execute_remediation(pane_id: &str, actions: &[PlannedAction]) -> Result<()> {
    for action in actions {
        match action {
            PlannedAction::Compact => {
                run_herdr_args(&["pane", "run", pane_id, COMPACT_COMMAND])
                    .with_context(|| format!("send compact command to Herdr pane {pane_id}"))?;
                run_herdr_args(&[
                    "wait",
                    "agent-status",
                    pane_id,
                    "--status",
                    "idle",
                    "--timeout",
                    &COMPACT_WAIT_TIMEOUT_MS.to_string(),
                ])
                .with_context(|| format!("wait for compact to finish in Herdr pane {pane_id}"))?;
            }
            PlannedAction::SwitchModel(model) => {
                let command = format!("/model {model}");
                run_herdr_args(&["pane", "run", pane_id, command.as_str()])
                    .with_context(|| format!("send model switch to Herdr pane {pane_id}"))?;
            }
        }
    }

    Ok(())
}

pub(crate) fn remediation_plan(
    session: &TranscriptSession,
    matching_panes: &[&HerdrPane],
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> RemediationPlan {
    let target = target_for_session(session, config);
    let gate = gate_decision_for_matches(session, matching_panes, state, config, now);
    let actions = if let Some(target) = target {
        if detect_drift(session, &target.target_model).is_some() && gate.is_allowed() {
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
    let latest = session.latest_model.as_ref()?;
    if latest == desired_model {
        return None;
    }

    let (from, to) = if session
        .model_history
        .iter()
        .any(|model| model == desired_model)
    {
        (desired_model.to_string(), latest.clone())
    } else {
        (latest.clone(), desired_model.to_string())
    };

    Some(ModelDrift { from, to })
}

pub(crate) fn gate_decision_for_matches(
    session: &TranscriptSession,
    matching_panes: &[&HerdrPane],
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> GateDecision {
    let mut blockers = Vec::new();
    let mut focused_tiebreak = None;

    if now - session.last_event_at < Duration::seconds(config.transcript_quiet_seconds as i64) {
        blockers.push(GateBlocker::TranscriptActive);
    }

    match matching_panes {
        [] => blockers.push(GateBlocker::NoPane),
        [pane] if pane.agent_status.as_deref() == Some("idle") => {}
        [pane] => blockers.push(GateBlocker::PaneBusy(
            pane.agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )),
        panes => match sole_focused_pane(panes) {
            Some(pane) if pane.agent_status.as_deref() == Some("idle") => {
                focused_tiebreak = Some(pane_id(pane).to_string());
            }
            Some(pane) => blockers.push(GateBlocker::PaneBusy(
                pane.agent_status
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            )),
            None => blockers.push(GateBlocker::AmbiguousPane(panes.len())),
        },
    }

    if let Some(last_action_unix) = state.and_then(|state| state.last_action_unix) {
        if let Some(last_action_at) = unix_to_utc(last_action_unix) {
            if now - last_action_at < Duration::seconds(config.debounce_seconds as i64) {
                blockers.push(GateBlocker::Debounce);
            }
        }
    }

    GateDecision {
        blockers,
        focused_tiebreak,
    }
}

/// Resolves the tiebreak winner among multiple same-cwd pane matches: the
/// single pane with `focused == true`, if exactly one exists. Zero or more
/// than one focused pane leaves the ambiguity unresolved.
fn sole_focused_pane<'a>(panes: &[&'a HerdrPane]) -> Option<&'a HerdrPane> {
    let mut focused = panes.iter().copied().filter(|pane| pane.focused);
    let candidate = focused.next()?;
    if focused.next().is_some() {
        None
    } else {
        Some(candidate)
    }
}

pub(crate) fn status_state(panes: &[&HerdrPane], gate: &GateDecision) -> String {
    if panes.is_empty() {
        return "not-open".to_string();
    }
    if gate.is_allowed() {
        return match &gate.focused_tiebreak {
            Some(pane_id) => format!("idle (focused-tiebreak:{pane_id})"),
            None => "idle".to_string(),
        };
    }
    describe_gate(gate)
}

pub(crate) fn describe_gate(gate: &GateDecision) -> String {
    if gate.is_allowed() {
        return match &gate.focused_tiebreak {
            Some(pane_id) => format!("allowed (focused-tiebreak:{pane_id})"),
            None => "allowed".to_string(),
        };
    }

    gate.blockers
        .iter()
        .map(|blocker| match blocker {
            GateBlocker::NoPane => "no-pane".to_string(),
            GateBlocker::AmbiguousPane(count) => format!("ambiguous-pane:{count}"),
            GateBlocker::TranscriptActive => "transcript-active".to_string(),
            GateBlocker::PaneBusy(state) => format!("pane-{state}"),
            GateBlocker::Debounce => "debounce".to_string(),
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
