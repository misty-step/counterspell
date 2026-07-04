use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::herdr::{matching_panes_for_cwd, pane_id, HerdrPane};
use crate::model::{Config, SessionState, StatusRow, TranscriptSession, WatchRow, WatchStore};
use crate::remediation::{
    describe_gate, describe_watch_actions, detect_drift, execute_remediation, format_target_match,
    gate_decision_for_matches, remediation_plan, status_state, target_for_session,
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
            let matching_panes = session
                .cwd
                .as_deref()
                .map(|cwd| matching_panes_for_cwd(cwd, panes))
                .unwrap_or_default();
            for pane in &matching_panes {
                used_panes.insert(pane_id(pane).to_string());
            }
            let pane = if matching_panes.is_empty() {
                "not-open".to_string()
            } else {
                join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
            };
            let target = target_for_session(session, config);
            let drift = target
                .as_ref()
                .map(|target| {
                    detect_drift(session, &target.target_model)
                        .map(|drift| format!("{}->{}", drift.from, drift.to))
                        .unwrap_or_else(|| "ok".to_string())
                })
                .unwrap_or_else(|| "ignored".to_string());
            let state = store.sessions.get(&session.session_id);
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
) -> Result<(Vec<WatchRow>, bool)> {
    let mut store_changed = false;
    let mut rows = Vec::new();

    for session in sessions {
        let matching_panes = session
            .cwd
            .as_deref()
            .map(|cwd| matching_panes_for_cwd(cwd, panes))
            .unwrap_or_default();
        let state: Option<&SessionState> = store.sessions.get(&session.session_id);
        let plan = remediation_plan(session, &matching_panes, state, config, now);
        if arm && !plan.actions.is_empty() {
            let pane = plan
                .gate
                .focused_tiebreak
                .as_deref()
                .and_then(|winner| {
                    matching_panes
                        .iter()
                        .copied()
                        .find(|pane| pane_id(pane) == winner)
                })
                .or_else(|| matching_panes.first().copied())
                .context("eligible remediation plan had no Herdr pane")?;
            execute_remediation(pane_id(pane), &plan.actions)?;
            store.sessions.insert(
                session.session_id.clone(),
                SessionState {
                    session_id: session.session_id.clone(),
                    cwd: session.cwd.clone(),
                    last_action_unix: Some(now.timestamp().try_into().unwrap_or(0)),
                },
            );
            store_changed = true;
        }
        let target = target_for_session(session, config);
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

    Ok((rows, store_changed))
}
