use super::*;
use crate::config::{parse_config_file, remove_session_target_from_config};
use crate::dashboard::{build_dashboard_snapshot, render_dashboard_html};
use crate::herdr::{
    matching_panes_for_session, pane_id, pane_session_id, HerdrAgentSession, HerdrTab,
    HerdrWorkspace,
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
    };
    assert_eq!(
        gate_decision_for_matches(&quiet_session, &panes, Some(&state), &config, now).blockers,
        vec![GateBlocker::Debounce]
    );
}

#[test]
fn drift_plan_sequences_compact_then_switch() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let pane = idle_pane();
    let panes = [&pane];

    let plan = remediation_plan(&session, &panes, None, &config, now);

    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::Compact,
            PlannedAction::SwitchModel("claude-fable-5".to_string())
        ]
    );
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
    }
}

#[test]
fn drift_on_working_session_bound_pane_interrupts_and_remediates_in_one_pass() {
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
    // The whole chain fires in one pass: the switch must never depend on a
    // later tick happening to sample the pane idle (that lost the
    // 2026-07-04 switch on a busy session). The compact is QUEUED, not
    // waited on — the switch typed behind it executes post-compact FIFO.
    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::Interrupt,
            PlannedAction::QueueCompact,
            PlannedAction::SwitchModel("claude-fable-5".to_string())
        ]
    );
}

#[test]
fn working_pane_with_pending_compact_blocks_requeue() {
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

    assert_eq!(plan.gate.blockers, vec![GateBlocker::CompactPending]);
    assert!(plan.actions.is_empty());
}

#[test]
fn idle_pane_with_pending_compact_never_bare_switches() {
    // A pending marker does NOT prove the compact ran (the chain may have
    // crashed before typing it). A bare /model on a large context pops the
    // cache-rewind dialog and wedges the pane — so an idle pane with a
    // pending marker must fall through to the ordinary gate, not shortcut
    // to a lone switch.
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let mut session = test_session(now);
    session.last_event_at = now - Duration::seconds(3);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = fast_path_state(now, 90);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(plan.gate.blockers, vec![GateBlocker::TranscriptActive]);
    assert!(plan.actions.is_empty());
}

#[test]
fn idle_pane_with_pending_compact_recovers_via_full_compact_then_switch() {
    // Crash recovery: once the transcript is quiet, the ordinary idle path
    // re-runs the full compact-then-switch pair. Re-compacting an already
    // compacted (tiny) context is cheap; bare-switching an uncompacted one
    // is not safe. The marker never shortcuts the sequence.
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = fast_path_state(now, 90);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert!(plan.gate.is_allowed());
    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::Compact,
            PlannedAction::SwitchModel("claude-fable-5".to_string())
        ]
    );
}

#[test]
fn expired_pending_compact_falls_back_to_full_remediation() {
    let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let config = test_config();
    let session = test_session(now);
    let mut pane = idle_pane();
    pane.agent_session = bound_session("session-1");
    let panes = [&pane];
    let state = fast_path_state(now, 3600);

    let plan = remediation_plan(&session, &panes, Some(&state), &config, now);

    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::Compact,
            PlannedAction::SwitchModel("claude-fable-5".to_string())
        ]
    );
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
    assert_eq!(
        plan.actions,
        vec![
            PlannedAction::Compact,
            PlannedAction::SwitchModel("claude-fable-5".to_string())
        ]
    );
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
    );

    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Auto"));
    assert!(html.contains("target claude-fable-5"));
    assert!(!html.contains(r#"<button type="submit">Enable</button>"#));
}
