use super::*;
use crate::config::{parse_config_file, remove_session_target_from_config};
use crate::dashboard::{build_dashboard_snapshot, render_dashboard_html};
use crate::herdr::{
    matching_panes_for_session, pane_id, pane_session_id, session_reporting_broken,
    HerdrAgentSession, HerdrTab, HerdrWorkspace,
};
use std::io::Write;

fn test_config() -> Config {
    Config {
        projects_dir: PathBuf::from("/tmp/projects"),
        recent_hours: 72,
        targets: vec![TargetRule {
            session_id: Some("session-1".to_string()),
            project_pattern: None,
            cwd_pattern: None,
            target_model: "claude-fable-5".to_string(),
        }],
        transcript_quiet_seconds: 30,
        debounce_seconds: 300,
    }
}

fn test_session(now: DateTime<Utc>) -> TranscriptSession {
    TranscriptSession {
        session_id: "session-1".to_string(),
        project: "project".to_string(),
        cwd: Some("/repo".to_string()),
        last_event_at: now - Duration::seconds(60),
        latest_model: Some("claude-opus-4-1".to_string()),
        latest_model_at: Some(now - Duration::seconds(60)),
        latest_compact_at: None,
        model_history: vec!["claude-fable-5".to_string(), "claude-opus-4-1".to_string()],
    }
}

fn idle_pane() -> HerdrPane {
    HerdrPane {
        pane_id: "pane-1".to_string(),
        workspace_id: "w1".to_string(),
        tab_id: "w1:t1".to_string(),
        cwd: Some("/repo".to_string()),
        foreground_cwd: Some("/repo".to_string()),
        agent: Some("claude".to_string()),
        agent_status: Some("idle".to_string()),
        focused: false,
        title: None,
        custom_status: None,
        agent_session: None,
    }
}

fn bound_session(session_id: &str) -> Option<HerdrAgentSession> {
    Some(HerdrAgentSession {
        kind: Some("id".to_string()),
        value: Some(session_id.to_string()),
    })
}

#[test]
fn drift_detection_reads_fable_to_opus_from_transcript_jsonl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session-1.jsonl");
    let mut file = File::create(&path).expect("create transcript");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:00:00Z","cwd":"/repo","message":{{"model":"claude-fable-5"}}}}"#
    )
    .expect("write fable");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:01:00Z","cwd":"/repo","message":{{"model":"claude-opus-4-1"}}}}"#
    )
    .expect("write opus");

    let session = parse_transcript_file(&path, "project".to_string(), Utc::now()).unwrap();
    let drift = detect_drift(&session, "claude-fable-5").expect("drift");

    assert_eq!(
        drift,
        ModelDrift {
            from: "claude-fable-5".to_string(),
            to: "claude-opus-4-1".to_string()
        }
    );
}

#[test]
fn transcript_parser_ignores_angle_bracket_model_sentinels() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session-1.jsonl");
    let mut file = File::create(&path).expect("create transcript");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:00:00Z","cwd":"/repo","message":{{"model":"claude-opus-4-1"}}}}"#
    )
    .expect("write opus");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:01:00Z","cwd":"/repo","message":{{"model":"claude-fable-5"}}}}"#
    )
    .expect("write fable");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:02:00Z","cwd":"/repo","model":"<synthetic>"}}"#
    )
    .expect("write synthetic marker");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:03:00Z","cwd":"/repo","message":{{"model":"<system>"}}}}"#
    )
    .expect("write system marker");

    let session = parse_transcript_file(&path, "project".to_string(), Utc::now()).unwrap();

    assert_eq!(session.latest_model.as_deref(), Some("claude-fable-5"));
    assert_eq!(
        session.model_history,
        vec!["claude-opus-4-1".to_string(), "claude-fable-5".to_string()]
    );
    assert!(detect_drift(&session, "claude-fable-5").is_none());
}

#[test]
fn transcript_parser_keeps_real_downgrade_before_sentinel() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session-1.jsonl");
    let mut file = File::create(&path).expect("create transcript");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:00:00Z","cwd":"/repo","message":{{"model":"claude-fable-5"}}}}"#
    )
    .expect("write fable");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:01:00Z","cwd":"/repo","message":{{"model":"claude-opus-4-1"}}}}"#
    )
    .expect("write opus");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:02:00Z","cwd":"/repo","model":"<synthetic>"}}"#
    )
    .expect("write synthetic marker");

    let session = parse_transcript_file(&path, "project".to_string(), Utc::now()).unwrap();
    let drift = detect_drift(&session, "claude-fable-5").expect("drift");

    assert_eq!(session.latest_model.as_deref(), Some("claude-opus-4-1"));
    assert_eq!(
        session.model_history,
        vec!["claude-fable-5".to_string(), "claude-opus-4-1".to_string()]
    );
    assert_eq!(
        drift,
        ModelDrift {
            from: "claude-fable-5".to_string(),
            to: "claude-opus-4-1".to_string()
        }
    );
}

#[test]
fn transcript_parser_records_claude_compact_boundary_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session-1.jsonl");
    let mut file = File::create(&path).expect("create transcript");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:00:00Z","cwd":"/repo","message":{{"model":"claude-fable-5"}}}}"#
    )
    .expect("write fable");
    writeln!(
        file,
        r#"{{"type":"system","subtype":"compact_boundary","timestamp":"2026-07-02T12:01:00Z","sessionId":"session-1","isCompactSummary":true,"compactMetadata":{{"trigger":"manual"}}}}"#
    )
    .expect("write compact boundary");

    let session = parse_transcript_file(&path, "project".to_string(), Utc::now()).unwrap();

    assert_eq!(
        session.latest_compact_at,
        Some(
            DateTime::parse_from_rfc3339("2026-07-02T12:01:00Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
}

#[test]
fn drift_detection_defensively_ignores_sentinel_latest_model() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut session = test_session(now);
    session.latest_model = Some("<synthetic>".to_string());
    session.model_history = vec![
        "claude-fable-5".to_string(),
        "claude-opus-4-1".to_string(),
        "<synthetic>".to_string(),
    ];

    let drift = detect_drift(&session, "claude-fable-5").expect("drift");

    assert_eq!(
        drift,
        ModelDrift {
            from: "claude-fable-5".to_string(),
            to: "claude-opus-4-1".to_string()
        }
    );

    session.model_history = vec!["claude-fable-5".to_string(), "<synthetic>".to_string()];
    assert!(detect_drift(&session, "claude-fable-5").is_none());
}

#[test]
fn unattended_gate_requires_quiet_transcript_idle_pane_and_debounce() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut config = test_config();
    config.transcript_quiet_seconds = 30;
    config.debounce_seconds = 300;
    let pane = idle_pane();
    let panes = [&pane];
    let quiet_session = test_session(now);

    assert!(gate_decision_for_matches(&quiet_session, &panes, None, &config, now).is_allowed());

    let mut active_session = quiet_session.clone();
    active_session.last_event_at = now - Duration::seconds(5);
    assert_eq!(
        gate_decision_for_matches(&active_session, &panes, None, &config, now).blockers,
        vec![GateBlocker::TranscriptActive]
    );

    let mut busy_pane = pane.clone();
    busy_pane.agent_status = Some("working".to_string());
    let busy_panes = [&busy_pane];
    assert_eq!(
        gate_decision_for_matches(&quiet_session, &busy_panes, None, &config, now).blockers,
        vec![GateBlocker::PaneBusy("working".to_string())]
    );

    let state = SessionState {
        session_id: "session-1".to_string(),
        cwd: Some("/repo".to_string()),
        last_action_unix: Some((now - Duration::seconds(60)).timestamp() as u64),
        pending_compact_unix: None,
        remediation_chain: None,
    };
    assert_eq!(
        gate_decision_for_matches(&quiet_session, &panes, Some(&state), &config, now).blockers,
        vec![GateBlocker::Debounce]
    );
}

#[test]
fn drift_plan_sends_compact_first() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let pane = idle_pane();
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert_eq!(plan.actions, vec![PlannedAction::Compact]);
}

#[test]
fn ambiguous_pane_matches_block_remediation() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let left = idle_pane();
    let mut right = idle_pane();
    right.pane_id = "pane-2".to_string();
    let panes = [&left, &right];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert_eq!(plan.gate.blockers, vec![GateBlocker::AmbiguousPane(2)]);
    assert!(plan.actions.is_empty());
}

#[test]
fn single_focused_pane_no_longer_breaks_tie_among_multiple_idle_matches() {
    // Regression: on 2026-07-04 the focused-pane tiebreak routed a
    // compact+switch for one session into a different live session that
    // happened to hold focus in the same cwd. Focus must never disambiguate.
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut focused = idle_pane();
    focused.pane_id = "pane-2".to_string();
    focused.focused = true;
    let unfocused = idle_pane();
    let panes = [&unfocused, &focused];

    let gate = gate_decision_for_matches(&session, &panes, None, &config, now);

    assert_eq!(gate.blockers, vec![GateBlocker::AmbiguousPane(2)]);
    assert_eq!(status_state(&panes, &gate), "ambiguous-pane:2");
    assert_eq!(describe_gate(&gate), "ambiguous-pane:2");

    let plan = remediation_plan(&session, &panes, None, &config, now);
    assert!(plan.actions.is_empty());
}

fn fast_path_state(now: DateTime<Utc>, pending_secs_ago: i64) -> SessionState {
    SessionState {
        session_id: "session-1".to_string(),
        cwd: Some("/repo".to_string()),
        last_action_unix: None,
        pending_compact_unix: Some((now - Duration::seconds(pending_secs_ago)).timestamp() as u64),
        remediation_chain: None,
    }
}

fn chain_state(now: DateTime<Utc>, step: RemediationStep, step_secs_ago: i64) -> SessionState {
    let sent = (now - Duration::seconds(step_secs_ago)).timestamp() as u64;
    SessionState {
        session_id: "session-1".to_string(),
        cwd: Some("/repo".to_string()),
        last_action_unix: None,
        pending_compact_unix: None,
        remediation_chain: Some(RemediationChainState {
            target_model: "claude-fable-5".to_string(),
            started_unix: sent,
            step_sent_unix: sent,
            step,
            recovery_reason: None,
        }),
    }
}

#[test]
fn drift_on_working_session_bound_pane_interrupts_and_sends_compact_immediately() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    // Transcript still streaming — fast path must not care.
    let mut session = test_session(now);
    session.last_event_at = now - Duration::seconds(2);
    let mut pane = idle_pane();
    pane.agent_status = Some("working".to_string());
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert!(plan.gate.is_allowed());
    assert_eq!(
        plan.actions,
        vec![PlannedAction::Interrupt, PlannedAction::Compact]
    );
}

#[test]
fn compact_sent_chain_blocks_reentry_until_summary_evidence() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("working".to_string());
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Compact, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.gate.blockers,
        vec![GateBlocker::RemediationInFlight(RemediationStep::Compact)]
    );
    assert!(plan.actions.is_empty());
}

#[test]
fn compact_sent_chain_advances_to_switch_and_continue_after_summary_evidence() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let mut session = test_session(now);
    session.latest_compact_at = Some(now - Duration::seconds(30));
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Compact, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert!(plan.gate.is_allowed());
    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::SwitchModel("claude-fable-5".to_string()),
            PlannedAction::Continue
        ]
    );
}

#[test]
fn compact_sent_chain_does_not_complete_before_continue_even_if_fable_is_visible() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let mut session = test_session(now);
    session.latest_compact_at = Some(now - Duration::seconds(30));
    session.latest_model = Some("claude-fable-5".to_string());
    session.latest_model_at = Some(now - Duration::seconds(20));
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Compact, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::SwitchModel("claude-fable-5".to_string()),
            PlannedAction::Continue
        ]
    );
}

#[test]
fn switch_sent_chain_resumes_with_continue() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Switch, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert!(plan.gate.is_allowed());
    assert_eq!(plan.actions, vec![PlannedAction::Continue]);
}

#[test]
fn continue_sent_chain_waits_for_compact_and_fable_evidence() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Continue, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.gate.blockers,
        vec![GateBlocker::RemediationInFlight(RemediationStep::Continue)]
    );
    assert!(plan.actions.is_empty());
}

#[test]
fn chain_completion_requires_compact_summary_and_post_chain_fable() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut session = test_session(now);
    let state = chain_state(now, RemediationStep::Continue, 60);
    let chain = state.remediation_chain.as_ref().expect("chain");

    assert!(!chain_has_completion_evidence(&session, chain));

    session.latest_compact_at = Some(now - Duration::seconds(30));
    assert!(!chain_has_completion_evidence(&session, chain));

    session.latest_model = Some("claude-fable-5".to_string());
    session.latest_model_at = Some(now - Duration::seconds(20));
    assert!(chain_has_completion_evidence(&session, chain));
}

#[test]
fn timed_out_compact_without_summary_restarts_with_reason_not_while_in_flight() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = chain_state(now, RemediationStep::Compact, 3600);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.actions,
        vec![PlannedAction::Interrupt, PlannedAction::Compact]
    );
    assert_eq!(
        plan.recovery_reason.as_deref(),
        Some("timeout:compact-sent")
    );
}

#[test]
fn legacy_pending_compact_still_blocks_requeue_before_timeout() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("working".to_string());
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = fast_path_state(now, 60);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.gate.blockers,
        vec![GateBlocker::RemediationInFlight(RemediationStep::Compact)]
    );
    assert!(plan.actions.is_empty());
}

#[test]
fn working_pane_without_session_binding_never_gets_fast_path() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("working".to_string());
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert!(plan.actions.is_empty());
    assert!(plan
        .gate
        .blockers
        .contains(&GateBlocker::PaneBusy("working".to_string())));
}

#[test]
fn blocked_pane_never_gets_fast_path_even_when_session_bound() {
    // "blocked" usually means a permission prompt is open — injected text
    // would answer the prompt.
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("blocked".to_string());
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert!(plan.actions.is_empty());
    assert!(plan
        .gate
        .blockers
        .contains(&GateBlocker::PaneBusy("blocked".to_string())));
}

#[test]
fn debounced_working_pane_does_not_queue_compact() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("working".to_string());
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = SessionState {
        session_id: "session-1".to_string(),
        cwd: Some("/repo".to_string()),
        last_action_unix: Some((now - Duration::seconds(60)).timestamp() as u64),
        pending_compact_unix: None,
        remediation_chain: None,
    };

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert!(plan.actions.is_empty());
    assert!(plan.gate.blockers.contains(&GateBlocker::Debounce));
}

#[test]
fn session_bound_pane_is_authoritative_over_cwd_matches() {
    let bound = {
        let mut pane = idle_pane();
        pane.pane_id = "pane-2".to_string();
        pane.agent_session = bound_session("session-1");
        pane
    };
    let unbound = idle_pane();
    let panes = vec![unbound, bound];

    let matches = matching_panes_for_session("session-1", Some("/repo"), &panes);

    assert_eq!(
        matches.iter().map(|pane| pane_id(pane)).collect::<Vec<_>>(),
        vec!["pane-2"]
    );
}

#[test]
fn cwd_fallback_excludes_panes_bound_to_other_sessions() {
    let foreign = {
        let mut pane = idle_pane();
        pane.pane_id = "pane-2".to_string();
        pane.agent_session = bound_session("other-session");
        pane
    };
    let unbound = idle_pane();
    let panes = vec![unbound, foreign];

    let matches = matching_panes_for_session("session-1", Some("/repo"), &panes);

    assert_eq!(
        matches.iter().map(|pane| pane_id(pane)).collect::<Vec<_>>(),
        vec!["pane-1"]
    );
}

#[test]
fn all_panes_bound_to_other_sessions_yield_no_match() {
    let mut foreign = idle_pane();
    foreign.agent_session = bound_session("other-session");
    let panes = vec![foreign];

    let matches = matching_panes_for_session("session-1", Some("/repo"), &panes);

    assert!(matches.is_empty());
}

#[test]
fn path_kind_agent_session_binds_by_file_stem() {
    let mut pane = idle_pane();
    pane.agent_session = Some(HerdrAgentSession {
        kind: Some("path".to_string()),
        value: Some("/home/u/.claude/projects/-repo/session-1.jsonl".to_string()),
    });

    assert_eq!(pane_session_id(&pane), Some("session-1"));

    let panes = vec![pane];
    let matches = matching_panes_for_session("session-1", None, &panes);
    assert_eq!(matches.len(), 1);
}

#[test]
fn session_reporting_broken_true_when_every_claude_pane_lacks_a_session() {
    let mut other = idle_pane();
    other.pane_id = "pane-2".to_string();
    let panes = vec![idle_pane(), other];

    assert!(session_reporting_broken(&panes));
}

#[test]
fn session_reporting_broken_false_when_any_claude_pane_reports_a_session() {
    let mut bound = idle_pane();
    bound.pane_id = "pane-2".to_string();
    bound.agent_session = bound_session("session-1");
    let panes = vec![idle_pane(), bound];

    assert!(!session_reporting_broken(&panes));
}

#[test]
fn session_reporting_broken_false_when_no_claude_panes_exist() {
    assert!(!session_reporting_broken(&[]));
}

#[test]
fn zero_focused_panes_still_hard_blocks_ambiguous_matches() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let left = idle_pane();
    let mut right = idle_pane();
    right.pane_id = "pane-2".to_string();
    let panes = [&left, &right];

    let gate = gate_decision_for_matches(&session, &panes, None, &config, now);

    assert_eq!(gate.blockers, vec![GateBlocker::AmbiguousPane(2)]);
}

#[test]
fn multiple_focused_panes_still_hard_blocks_ambiguous_matches() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut left = idle_pane();
    left.focused = true;
    let mut right = idle_pane();
    right.pane_id = "pane-2".to_string();
    right.focused = true;
    let panes = [&left, &right];

    let gate = gate_decision_for_matches(&session, &panes, None, &config, now);

    assert_eq!(gate.blockers, vec![GateBlocker::AmbiguousPane(2)]);
}

#[test]
fn fable_history_is_auto_targeted_without_config() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut config = test_config();
    config.targets.clear();
    let session = test_session(now);
    let pane = idle_pane();
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert!(detect_drift(&session, "claude-fable-5").is_some());
    assert_eq!(plan.actions, vec![PlannedAction::Compact]);
    assert_eq!(
        format_target_match(&target_for_session(&session, &config).expect("auto target")),
        "claude-fable-5 (auto:fable)"
    );
}

#[test]
fn status_marks_fable_history_sessions_watched_without_config() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut config = test_config();
    config.targets.clear();
    let session = test_session(now);
    let pane = idle_pane();
    let store = WatchStore::default();

    let rows = status_rows(&[session], &[pane], &store, &config, now);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].watch, "watched");
    assert_eq!(rows[0].target, "claude-fable-5 (auto:fable)");
    assert_eq!(rows[0].model, "claude-opus-4-1");
    assert_eq!(rows[0].drift, "claude-fable-5->claude-opus-4-1");
}

#[test]
fn status_maps_transcripts_only_to_claude_panes() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let claude_pane = idle_pane();
    let mut codex_pane = idle_pane();
    codex_pane.pane_id = "pane-codex".to_string();
    codex_pane.agent = Some("codex".to_string());
    let store = WatchStore::default();

    let rows = status_rows(&[session], &[claude_pane, codex_pane], &store, &config, now);

    assert_eq!(rows[0].pane, "pane-1");
    assert_eq!(rows[0].agent, "claude");
}

#[test]
fn target_reason_renders_without_debug_quotes() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = Config {
        targets: vec![TargetRule {
            session_id: None,
            project_pattern: Some("project*".to_string()),
            cwd_pattern: None,
            target_model: "claude-fable-5".to_string(),
        }],
        ..test_config()
    };
    let mut session = test_session(now);
    session.latest_model = Some("claude-opus-4-1".to_string());
    session.model_history = vec!["claude-opus-4-1".to_string()];

    let target = target_for_session(&session, &config).expect("target");

    assert_eq!(
        format_target_match(&target),
        "claude-fable-5 (project:project*)"
    );
}

#[test]
fn auto_fable_target_precedes_configured_targets() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = Config {
        targets: vec![TargetRule {
            session_id: Some("session-1".to_string()),
            project_pattern: None,
            cwd_pattern: None,
            target_model: "claude-opus-4-8".to_string(),
        }],
        ..test_config()
    };
    let session = test_session(now);

    let target = target_for_session(&session, &config).expect("target");

    assert_eq!(format_target_match(&target), "claude-fable-5 (auto:fable)");
}

#[test]
fn config_parsing_reads_counterspell_toml() {
    let raw = r#"
projects_dir = "/tmp/claude-projects"
recent_hours = 12
transcript_quiet_seconds = 45
debounce_seconds = 600

[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"
"#;
    let parsed: FileConfig = toml::from_str(raw).expect("config");

    assert_eq!(
        parsed.projects_dir,
        Some(PathBuf::from("/tmp/claude-projects"))
    );
    assert_eq!(parsed.recent_hours, Some(12));
    assert_eq!(
        parsed.targets,
        vec![TargetRule {
            session_id: None,
            project_pattern: Some("-Users-phaedrus-Development-adminifi*".to_string()),
            cwd_pattern: None,
            target_model: "claude-fable-5".to_string()
        }]
    );
    assert_eq!(parsed.transcript_quiet_seconds, Some(45));
    assert_eq!(parsed.debounce_seconds, Some(600));
}

#[test]
fn config_rejects_global_target_without_selector() {
    let raw = r#"
[[targets]]
target_model = "claude-fable-5"
"#;
    let parsed: FileConfig = toml::from_str(raw).expect("config");
    assert!(validate_targets(parsed.targets).is_err());
}

#[test]
fn remove_session_target_rewrites_config_without_touching_other_targets() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("counterspell.toml");
    std::fs::write(
        &path,
        r#"
recent_hours = 12

[[targets]]
session_id = "session-1"
target_model = "claude-fable-5"

[[targets]]
project_pattern = "project*"
target_model = "claude-fable-5"
"#,
    )
    .expect("config");

    assert!(remove_session_target_from_config(&path, "session-1").expect("remove"));
    let parsed = parse_config_file(&path).expect("parse");

    assert_eq!(parsed.targets.len(), 1);
    assert_eq!(
        parsed.targets[0].project_pattern,
        Some("project*".to_string())
    );
    assert_eq!(parsed.recent_hours, Some(12));
}

#[test]
fn dashboard_render_shows_herdr_panes_and_session_toggles() {
    let generated_at = DateTime::parse_from_rfc3339("2026-07-03T18:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let snapshot = build_dashboard_snapshot(
        generated_at,
        &test_config(),
        &[test_session(generated_at)],
        &[idle_pane()],
        &[HerdrWorkspace {
            workspace_id: "w1".to_string(),
            label: Some("commander".to_string()),
            number: Some(1),
        }],
        &[HerdrTab {
            tab_id: "w1:t1".to_string(),
            label: Some("pure act".to_string()),
            number: Some(19),
        }],
        false,
        WatchArmDaemonStatus::Scheduled,
    );

    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell"));
    assert!(html.contains("Herdr Mirror Column Drilldown"));
    assert!(html.contains("Fable Claude Code sessions auto-watch"));
    assert!(html.contains("workspace -> tab -> session -> policy"));
    assert!(html.contains("data-workspace-trigger=\"w1\""));
    assert!(html.contains("data-pane-trigger=\"pane-1\""));
    assert!(html.contains("commander"));
    assert!(html.contains("Tab 19: pure act / pane-1"));
    assert!(html.contains("claude-fable-5"));
    assert!(html.contains("Auto"));
}

#[test]
fn dashboard_render_marks_fable_history_sessions_auto() {
    let generated_at = DateTime::parse_from_rfc3339("2026-07-03T18:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut config = test_config();
    config.targets.clear();
    let snapshot = build_dashboard_snapshot(
        generated_at,
        &config,
        &[test_session(generated_at)],
        &[idle_pane()],
        &[HerdrWorkspace {
            workspace_id: "w1".to_string(),
            label: Some("commander".to_string()),
            number: Some(1),
        }],
        &[HerdrTab {
            tab_id: "w1:t1".to_string(),
            label: Some("pure act".to_string()),
            number: Some(19),
        }],
        false,
        WatchArmDaemonStatus::Scheduled,
    );

    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Auto"));
    assert!(html.contains("target claude-fable-5"));
    assert!(!html.contains(r#"<button type="submit">Enable</button>"#));
}

fn empty_dashboard_snapshot(
    generated_at: DateTime<Utc>,
    master_disarmed: bool,
    watch_arm_status: WatchArmDaemonStatus,
) -> crate::dashboard::DashboardSnapshot {
    build_dashboard_snapshot(
        generated_at,
        &test_config(),
        &[],
        &[],
        &[],
        &[],
        master_disarmed,
        watch_arm_status,
    )
}

#[test]
fn dashboard_banner_warns_when_flag_enabled_but_daemon_not_scheduled() {
    // The one combination the operator must never be left to discover the
    // hard way: the flag says go, but nothing is actually loaded to run it.
    let generated_at = Utc::now();
    let snapshot =
        empty_dashboard_snapshot(generated_at, false, WatchArmDaemonStatus::NotScheduled);
    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell: ENABLED"));
    assert!(html.contains("watch-arm daemon: not scheduled"));
    assert!(html.contains("class=\"master-mismatch\""));
    assert!(html.contains("nothing will actually run"));
}

#[test]
fn dashboard_banner_warns_when_flag_enabled_but_daemon_not_installed() {
    let generated_at = Utc::now();
    let snapshot =
        empty_dashboard_snapshot(generated_at, false, WatchArmDaemonStatus::NotInstalled);
    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell: ENABLED"));
    assert!(html.contains("watch-arm daemon: not installed"));
    assert!(html.contains("class=\"master-mismatch\""));
}

#[test]
fn dashboard_banner_has_no_mismatch_warning_when_armed_and_scheduled() {
    let generated_at = Utc::now();
    let snapshot = empty_dashboard_snapshot(generated_at, false, WatchArmDaemonStatus::Scheduled);
    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell: ENABLED"));
    assert!(html.contains("watch-arm daemon: scheduled"));
    assert!(!html.contains("class=\"master-mismatch\""));
}

#[test]
fn dashboard_banner_has_no_mismatch_warning_when_disabled_regardless_of_daemon() {
    // Disabled is always safe: the flag itself blocks the hot path, so an
    // unscheduled daemon is not a dangerous combination worth alarming over.
    let generated_at = Utc::now();
    let snapshot = empty_dashboard_snapshot(generated_at, true, WatchArmDaemonStatus::NotScheduled);
    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell: DISABLED"));
    assert!(html.contains("watch-arm daemon: not scheduled"));
    assert!(!html.contains("class=\"master-mismatch\""));
}

#[test]
fn done_pane_is_remediable_like_idle() {
    // herdr reports `done` (turn complete, awaiting input) for panes launched
    // via `herdr agent start` — observed live 2026-07-04: probe pane w3H:p2
    // sat interactive at the prompt for 4+ minutes while the gate blocked
    // every tick with pane-done. A done pane accepts keystrokes exactly like
    // an idle one; refusing it means managed lanes are never remediated.
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_status = Some("done".to_string());
    let panes = [&pane];

    assert!(gate_decision_for_matches(&session, &panes, None, &config, now).is_allowed());

    let plan = remediation_plan(&session, &panes, None, &config, now);
    assert_eq!(plan.actions, vec![PlannedAction::Compact]);
}

#[test]
fn build_report_request_matches_herdr_report_agent_session_shape() {
    let request = build_report_request(
        "w1:p1",
        "session-xyz",
        Some("/path/to/session-xyz.jsonl"),
        12345,
    );

    assert_eq!(request["method"], "pane.report_agent_session");
    assert_eq!(request["params"]["pane_id"], "w1:p1");
    assert_eq!(request["params"]["source"], "herdr:claude");
    assert_eq!(request["params"]["agent"], "claude");
    assert_eq!(request["params"]["seq"], 12345);
    assert_eq!(request["params"]["agent_session_id"], "session-xyz");
    assert_eq!(
        request["params"]["agent_session_path"],
        "/path/to/session-xyz.jsonl"
    );
    assert!(request["id"]
        .as_str()
        .expect("id is a string")
        .starts_with("herdr:claude:"));
}

#[test]
fn build_report_request_omits_path_field_when_absent() {
    let request = build_report_request("w1:p1", "session-xyz", None, 1);
    assert!(request["params"].get("agent_session_path").is_none());
}

#[test]
fn resolve_target_session_prefers_explicit_session_id_override() {
    let config = test_config();
    let (session_id, transcript_path) = resolve_target_session(
        &config,
        Some("explicit-session"),
        None,
        std::path::Path::new("/whatever"),
        Utc::now(),
    )
    .expect("resolve");

    assert_eq!(session_id, "explicit-session");
    assert_eq!(transcript_path, None);
}

#[test]
fn resolve_target_session_derives_session_id_from_explicit_transcript_path() {
    let config = test_config();
    let path = PathBuf::from("/some/where/abc-123.jsonl");
    let (session_id, transcript_path) = resolve_target_session(
        &config,
        None,
        Some(path.as_path()),
        std::path::Path::new("/whatever"),
        Utc::now(),
    )
    .expect("resolve");

    assert_eq!(session_id, "abc-123");
    assert_eq!(
        transcript_path.as_deref(),
        Some("/some/where/abc-123.jsonl")
    );
}

#[test]
fn resolve_target_session_discovers_newest_transcript_for_matching_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let project_dir = projects.join("-Users-phaedrus-Development-repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    let transcript_path = project_dir.join("session-abc.jsonl");
    let mut file = File::create(&transcript_path).expect("create transcript");
    writeln!(
        file,
        r#"{{"type":"assistant","sessionId":"session-abc","timestamp":"2026-07-02T12:00:00Z","cwd":"{}","message":{{"model":"claude-fable-5"}}}}"#,
        cwd.display()
    )
    .expect("write transcript");

    let config = Config {
        projects_dir: projects,
        recent_hours: 999,
        targets: Vec::new(),
        transcript_quiet_seconds: 30,
        debounce_seconds: 300,
    };

    let (session_id, resolved_transcript_path) =
        resolve_target_session(&config, None, None, &cwd, Utc::now()).expect("resolve");

    assert_eq!(session_id, "session-abc");
    assert_eq!(
        resolved_transcript_path,
        Some(transcript_path.to_string_lossy().into_owned())
    );
}

#[test]
fn resolve_target_session_errors_clearly_when_no_transcript_matches_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = Config {
        projects_dir: temp.path().join("projects"),
        recent_hours: 999,
        targets: Vec::new(),
        transcript_quiet_seconds: 30,
        debounce_seconds: 300,
    };

    let error = resolve_target_session(&config, None, None, temp.path(), Utc::now())
        .expect_err("no transcript should match an empty projects dir");

    assert!(format!("{error:#}").contains("--session-id"));
}

#[test]
fn resolve_target_session_refuses_to_guess_when_two_sessions_share_a_cwd() {
    // Live regression (2026-07-07, commit 7f32423 smoke test): discovery
    // picked the newest of two concurrent sessions sharing a cwd and sent a
    // real report for the WRONG one. Ambiguity must hard-block here exactly
    // like `matching_panes_for_session` hard-blocks ambiguous cwd pane ties
    // — never silently pick "newest".
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let project_dir = projects.join("-Users-phaedrus-Development-repo");
    std::fs::create_dir_all(&project_dir).expect("project dir");

    let mut older = File::create(project_dir.join("session-older.jsonl")).expect("create older");
    writeln!(
        older,
        r#"{{"type":"assistant","sessionId":"session-older","timestamp":"2026-07-02T12:00:00Z","cwd":"{}","message":{{"model":"claude-fable-5"}}}}"#,
        cwd.display()
    )
    .expect("write older transcript");

    let mut newer = File::create(project_dir.join("session-newer.jsonl")).expect("create newer");
    writeln!(
        newer,
        r#"{{"type":"assistant","sessionId":"session-newer","timestamp":"2026-07-05T12:00:00Z","cwd":"{}","message":{{"model":"claude-fable-5"}}}}"#,
        cwd.display()
    )
    .expect("write newer transcript");

    let config = Config {
        projects_dir: projects,
        recent_hours: 999,
        targets: Vec::new(),
        transcript_quiet_seconds: 30,
        debounce_seconds: 300,
    };

    let error = resolve_target_session(&config, None, None, &cwd, Utc::now())
        .expect_err("ambiguous cwd match must refuse rather than guess");
    let message = format!("{error:#}");

    assert!(message.contains("session-older"));
    assert!(message.contains("session-newer"));
    assert!(message.contains("--session-id"));
    assert!(message.contains("2026-07-02T12:00:00"));
    assert!(message.contains("2026-07-05T12:00:00"));
}
