use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::events::FeedEvent;
use crate::herdr::{matching_panes_for_session, pane_id, HerdrPane};
use crate::model::{
    Config, PlannedAction, RemediationChainState, RemediationStep, SessionState, StatusRow,
    TargetMatch, TranscriptSession, WatchRow, WatchStore,
};
use crate::remediation::{
    chain_has_completion_evidence, compact_summary_after_chain_start, describe_actions,
    describe_gate, describe_watch_actions, detect_actionable_drift, execute_remediation,
    format_target_match, gate_decision_for_matches, is_auto_fable_target, remediation_plan,
    status_state, target_for_session,
};
use crate::util::{human_age, join_or_dash, short_session};

pub(crate) fn status_rows(
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    store: &WatchStore,
    config: &Config,
    now: DateTime<Utc>,
) -> Vec<StatusRow> {
    let mut used_panes = std::collections::BTreeSet::new();
    let mut rows = sessions
        .iter()
        .map(|session| {
            let matching_panes =
                matching_panes_for_session(&session.session_id, session.cwd.as_deref(), panes);
            for pane in &matching_panes {
                used_panes.insert(pane_id(pane).to_string());
            }
            let pane = if matching_panes.is_empty() {
                "not-open".to_string()
            } else {
                join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
            };
            let target = target_for_session(session, config);
            let state = store.sessions.get(&session.session_id);
            let drift = target
                .as_ref()
                .map(|target| {
                    detect_actionable_drift(session, &target.target_model, state)
                        .map(|drift| format!("{}->{}", drift.from, drift.to))
                        .unwrap_or_else(|| "ok".to_string())
                })
                .unwrap_or_else(|| "ignored".to_string());
            let gate = gate_decision_for_matches(session, &matching_panes, state, config, now);

            StatusRow {
                session_id: short_session(&session.session_id),
                project: session.project.clone(),
                cwd: session.cwd.clone().unwrap_or_else(|| "-".to_string()),
                pane,
                agent: join_or_dash(
                    matching_panes
                        .iter()
                        .filter_map(|pane| pane.agent.as_deref()),
                ),
                state: status_state(&matching_panes, &gate),
                watch: if target.is_some() {
                    "watched".to_string()
                } else {
                    "ignored".to_string()
                },
                target: target
                    .as_ref()
                    .map(format_target_match)
                    .unwrap_or_else(|| "no-target".to_string()),
                model: session
                    .latest_model
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                drift,
                updated: human_age(session.last_event_at, now),
            }
        })
        .collect::<Vec<_>>();

    for pane in panes {
        if pane.agent.as_deref() != Some("claude") || used_panes.contains(pane_id(pane)) {
            continue;
        }

        rows.push(StatusRow {
            session_id: format!("pane:{}", pane_id(pane)),
            project: "herdr-live-pane".to_string(),
            cwd: pane
                .cwd
                .clone()
                .or_else(|| pane.foreground_cwd.clone())
                .unwrap_or_else(|| "-".to_string()),
            pane: pane_id(pane).to_string(),
            agent: pane.agent.clone().unwrap_or_else(|| "-".to_string()),
            state: pane
                .agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            watch: "ignored".to_string(),
            target: "no-transcript-target".to_string(),
            model: "-".to_string(),
            drift: "ignored".to_string(),
            updated: "live".to_string(),
        });
    }

    rows
}

pub(crate) fn watch_rows(
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    store: &mut WatchStore,
    config: &Config,
    now: DateTime<Utc>,
    arm: bool,
    // Where to durably persist the in-flight marker before a remediation
    // chain starts typing. None in tests/dry contexts skips the mid-pass
    // save (the caller's ordinary end-of-pass save still applies).
    state_path: Option<&std::path::Path>,
) -> Result<(Vec<WatchRow>, bool, Vec<FeedEvent>)> {
    let mut store_changed = false;
    let mut rows = Vec::new();
    let mut feed_events = Vec::new();

    for session in sessions {
        let matching_panes =
            matching_panes_for_session(&session.session_id, session.cwd.as_deref(), panes);
        let now_unix: u64 = now.timestamp().try_into().unwrap_or(0);
        let completed_chain = store
            .sessions
            .get(&session.session_id)
            .and_then(|state| state.remediation_chain.as_ref())
            .filter(|chain| chain_has_completion_evidence(session, chain))
            .cloned();
        if let Some(chain) = &completed_chain {
            store.sessions.insert(
                session.session_id.clone(),
                SessionState {
                    session_id: session.session_id.clone(),
                    cwd: session.cwd.clone(),
                    last_action_unix: session
                        .latest_model_at
                        .and_then(|at| at.timestamp().try_into().ok())
                        .or(Some(now_unix)),
                    pending_compact_unix: None,
                    remediation_chain: None,
                },
            );
            store_changed = true;
            feed_events.push(FeedEvent {
                session_id: session.session_id.clone(),
                pane: event_pane(&matching_panes),
                from_model: "-".to_string(),
                to_model: chain.target_model.clone(),
                gate: "allowed".to_string(),
                action: "remediation_confirmed".to_string(),
                action_taken: "confirmed".to_string(),
                origin: "evidence".to_string(),
            });
        }

        let state: Option<&SessionState> = store.sessions.get(&session.session_id);
        let prior_last_action_unix = state.and_then(|state| state.last_action_unix);
        let prior_chain = state.and_then(|state| state.remediation_chain.clone());
        let plan = remediation_plan(session, &matching_panes, state, config, now);
        let target = target_for_session(session, config);
        let drift = target
            .as_ref()
            .and_then(|target| detect_actionable_drift(session, &target.target_model, state));
        let gate = describe_gate(&plan.gate);
        let pane = event_pane(&matching_panes);

        if let Some(drift) = &drift {
            feed_events.push(FeedEvent {
                session_id: session.session_id.clone(),
                pane: pane.clone(),
                from_model: drift.from.clone(),
                to_model: drift.to.clone(),
                gate: gate.clone(),
                action: "model_drift_detected".to_string(),
                action_taken: drift_action_taken(&plan.actions, &plan.gate, arm),
                origin: target
                    .as_ref()
                    .map(|target| {
                        if is_auto_fable_target(target) {
                            "downgraded-from-fable"
                        } else {
                            "configured-target-drift"
                        }
                    })
                    .unwrap_or("unknown")
                    .to_string(),
            });
        } else if target.is_none() && is_born_on_opus(session) {
            feed_events.push(FeedEvent {
                session_id: session.session_id.clone(),
                pane: pane.clone(),
                from_model: "none".to_string(),
                to_model: session
                    .latest_model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                gate: gate.clone(),
                action: "session_ignored".to_string(),
                action_taken: "none".to_string(),
                origin: "born-on-opus".to_string(),
            });
        }

        if arm && !plan.actions.is_empty() {
            // The gate only allows a plan when exactly one pane matched.
            let pane = matching_panes
                .first()
                .copied()
                .context("eligible remediation plan had no Herdr pane")?;
            let target_model = target
                .as_ref()
                .map(|target| target.target_model.clone())
                .or_else(|| prior_chain.as_ref().map(|chain| chain.target_model.clone()))
                .unwrap_or_else(|| crate::defaults::DEFAULT_TARGET_MODEL.to_string());
            let restart_chain = plan.recovery_reason.is_some()
                && prior_chain
                    .as_ref()
                    .is_some_and(|chain| !compact_summary_after_chain_start(session, chain))
                && plan
                    .actions
                    .iter()
                    .any(|action| matches!(action, PlannedAction::Interrupt));
            let chain_started_unix = if restart_chain {
                now_unix
            } else {
                prior_chain
                    .as_ref()
                    .map(|chain| chain.started_unix)
                    .unwrap_or(now_unix)
            };

            if let Some(reason) = &plan.recovery_reason {
                let (from_model, to_model) = drift_models(&drift, &target_model);
                feed_events.push(FeedEvent {
                    session_id: session.session_id.clone(),
                    pane: pane_id(pane).to_string(),
                    from_model,
                    to_model,
                    gate: gate.clone(),
                    action: "remediation_recovery".to_string(),
                    action_taken: reason.clone(),
                    origin: target_origin(target.as_ref()).to_string(),
                });
            }

            for action in &plan.actions {
                let Some(step) = step_for_action(action) else {
                    continue;
                };
                store.sessions.insert(
                    session.session_id.clone(),
                    SessionState {
                        session_id: session.session_id.clone(),
                        cwd: session.cwd.clone(),
                        last_action_unix: prior_last_action_unix,
                        pending_compact_unix: None,
                        remediation_chain: Some(RemediationChainState {
                            target_model: target_model.clone(),
                            started_unix: chain_started_unix,
                            step_sent_unix: now_unix,
                            step,
                            recovery_reason: plan.recovery_reason.clone(),
                        }),
                    },
                );
                if let Some(state_path) = state_path {
                    crate::store::save_store(state_path, store)?;
                }

                if let Err(error) = execute_remediation(pane_id(pane), std::slice::from_ref(action))
                {
                    // The command failed before we can trust the send. Clear
                    // the in-flight state so the next pass can retry from
                    // live evidence instead of waiting for a phantom step.
                    store.sessions.insert(
                        session.session_id.clone(),
                        SessionState {
                            session_id: session.session_id.clone(),
                            cwd: session.cwd.clone(),
                            last_action_unix: prior_last_action_unix,
                            pending_compact_unix: None,
                            remediation_chain: None,
                        },
                    );
                    if let Some(state_path) = state_path {
                        crate::store::save_store(state_path, store)?;
                    }
                    return Err(error);
                }
                store_changed = true;
                let (from_model, to_model) = drift_models(&drift, &target_model);
                feed_events.push(FeedEvent {
                    session_id: session.session_id.clone(),
                    pane: pane_id(pane).to_string(),
                    from_model,
                    to_model,
                    gate: gate.clone(),
                    action: action_event(action).to_string(),
                    action_taken: action_taken(action),
                    origin: target_origin(target.as_ref()).to_string(),
                });
            }
        }
        rows.push(WatchRow {
            session_id: short_session(&session.session_id),
            pane: if matching_panes.is_empty() {
                "not-open".to_string()
            } else {
                join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
            },
            model: session
                .latest_model
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            target: target
                .as_ref()
                .map(format_target_match)
                .unwrap_or_else(|| "ignored:no-target".to_string()),
            gate: describe_gate(&plan.gate),
            actions: describe_watch_actions(&plan.actions, arm),
        });
    }

    Ok((rows, store_changed, feed_events))
}

fn drift_action_taken(
    actions: &[crate::model::PlannedAction],
    gate: &crate::model::GateDecision,
    arm: bool,
) -> String {
    if actions.is_empty() {
        if gate.is_allowed() {
            "none".to_string()
        } else {
            "blocked".to_string()
        }
    } else if arm {
        "remediation-started".to_string()
    } else {
        format!("dry-run:{}", describe_actions(actions))
    }
}

fn step_for_action(action: &PlannedAction) -> Option<RemediationStep> {
    match action {
        PlannedAction::Interrupt => Some(RemediationStep::Interrupt),
        PlannedAction::Compact => Some(RemediationStep::Compact),
        PlannedAction::SwitchModel(_) => Some(RemediationStep::Switch),
        PlannedAction::Continue => Some(RemediationStep::Continue),
    }
}

fn action_event(action: &PlannedAction) -> &'static str {
    match action {
        PlannedAction::Interrupt => "interrupt_sent",
        PlannedAction::Compact => "compact_sent",
        PlannedAction::SwitchModel(_) => "model_switched",
        PlannedAction::Continue => "continue_sent",
    }
}

fn action_taken(action: &PlannedAction) -> String {
    match action {
        PlannedAction::Interrupt => "interrupt_sent".to_string(),
        PlannedAction::Compact => "compact_sent".to_string(),
        PlannedAction::SwitchModel(model) => format!("model_switched:{model}"),
        PlannedAction::Continue => "continue_sent".to_string(),
    }
}

fn target_origin(target: Option<&TargetMatch>) -> &'static str {
    target
        .map(|target| {
            if is_auto_fable_target(target) {
                "downgraded-from-fable"
            } else {
                "configured-target-drift"
            }
        })
        .unwrap_or("unknown")
}

fn drift_models(drift: &Option<crate::model::ModelDrift>, target_model: &str) -> (String, String) {
    drift
        .as_ref()
        .map(|drift| (drift.from.clone(), drift.to.clone()))
        .unwrap_or_else(|| ("unknown".to_string(), target_model.to_string()))
}

fn event_pane(panes: &[&HerdrPane]) -> String {
    if panes.is_empty() {
        "not-open".to_string()
    } else {
        join_or_dash(panes.iter().map(|pane| pane_id(pane)))
    }
}

fn is_born_on_opus(session: &TranscriptSession) -> bool {
    !session
        .model_history
        .iter()
        .any(|model| model == crate::defaults::DEFAULT_TARGET_MODEL)
        && session
            .latest_model
            .as_deref()
            .is_some_and(|model| model.to_ascii_lowercase().contains("opus"))
}
