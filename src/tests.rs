use super::*;
use crate::dashboard::{render_dashboard_html, DashboardSnapshot};
use crate::model::{StatusRow, StatusSummary};
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
        model_history: vec!["claude-fable-5".to_string(), "claude-opus-4-1".to_string()],
    }
}

fn idle_pane() -> HerdrPane {
    HerdrPane {
        pane_id: "pane-1".to_string(),
        cwd: Some("/repo".to_string()),
        foreground_cwd: Some("/repo".to_string()),
        agent: Some("claude".to_string()),
        agent_status: Some("idle".to_string()),
    }
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
fn unconfigured_drift_is_observed_but_not_armed() {
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
    assert!(plan.actions.is_empty());
    assert_eq!(target_for_session(&session, &config), None);
}

#[test]
fn status_marks_unconfigured_sessions_ignored_not_ok() {
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
    assert_eq!(rows[0].watch, "ignored");
    assert_eq!(rows[0].target, "no-target");
    assert_eq!(rows[0].model, "claude-opus-4-1");
    assert_eq!(rows[0].drift, "ignored");
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
    let session = test_session(now);

    let target = target_for_session(&session, &config).expect("target");

    assert_eq!(
        format_target_match(&target),
        "claude-fable-5 (project:project*)"
    );
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
fn dashboard_render_shows_running_state_and_watched_session() {
    let snapshot = DashboardSnapshot {
        generated_at: DateTime::parse_from_rfc3339("2026-07-03T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        summary: StatusSummary {
            total: 1,
            watched: 1,
            ignored: 0,
            mapped: 1,
            live_panes: 0,
            last_trigger_event: None,
            last_trigger_unix: None,
        },
        rows: vec![StatusRow {
            session_id: "session1".to_string(),
            project: "project".to_string(),
            cwd: "/repo".to_string(),
            pane: "wN:pB".to_string(),
            agent: "claude".to_string(),
            state: "idle".to_string(),
            watch: "watched".to_string(),
            target: "claude-fable-5 (session_id)".to_string(),
            model: "claude-fable-5".to_string(),
            drift: "ok".to_string(),
            updated: "live".to_string(),
        }],
    };

    let html = render_dashboard_html(&snapshot);

    assert!(html.contains("Counterspell"));
    assert!(html.contains("Running locally"));
    assert!(html.contains("wN:pB"));
    assert!(html.contains("claude-fable-5"));
}
