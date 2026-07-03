use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn version_flag_reports_package_version() {
    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("counterspell 0.1.0"));
}

#[test]
fn status_discovers_recent_sessions_and_maps_by_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let adminifi_cwd = temp.path().join("adminifi");
    let daybook_cwd = temp.path().join("daybook");
    fs::create_dir_all(&adminifi_cwd).expect("adminifi cwd");
    fs::create_dir_all(&daybook_cwd).expect("daybook cwd");

    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &adminifi_cwd,
        "claude-fable-5",
    );
    write_transcript(
        &projects,
        "-Users-phaedrus-Documents-daybook",
        "daybook-session",
        &daybook_cwd,
        "claude-fable-5",
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"
"#,
    );
    let state = temp.path().join("state.json");

    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[
            ("w13:p1", &adminifi_cwd, "claude", "idle", "adminifi"),
            ("wN:pB", &daybook_cwd, "claude", "idle", "daybook"),
        ],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("recent sessions"))
        .stdout(predicate::str::contains("adminifi"))
        .stdout(predicate::str::contains("daybook"))
        .stdout(predicate::str::contains("watched"))
        .stdout(predicate::str::contains("auto:fable"))
        .stdout(predicate::str::contains("w13:p1"))
        .stdout(predicate::str::contains("wN:pB"))
        .stdout(predicate::str::contains("ago"));
}

#[test]
fn status_shows_unmapped_sessions_as_not_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let watched_cwd = temp.path().join("watched");
    let other_cwd = temp.path().join("other");
    fs::create_dir_all(&watched_cwd).expect("watched cwd");
    fs::create_dir_all(&other_cwd).expect("other cwd");

    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi-habitat",
        "habitat-session",
        &watched_cwd,
        "claude-opus-4-8",
    );
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");

    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w1A:p1", &other_cwd, "claude", "idle", "other")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("habitat"))
        .stdout(predicate::str::contains("ignored"))
        .stdout(predicate::str::contains("not-open"));
}

#[test]
fn status_includes_live_claude_panes_without_recent_transcripts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let transcript_cwd = temp.path().join("adminifi").join("olympus");
    let root_cwd = temp.path().join("adminifi");
    fs::create_dir_all(&transcript_cwd).expect("transcript cwd");
    fs::create_dir_all(&root_cwd).expect("root cwd");

    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "transcript-one",
        &transcript_cwd,
        "claude-fable-5",
    );
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");

    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[
            ("w1B:p1", &transcript_cwd, "claude", "idle", "olympus"),
            ("w13:p1", &root_cwd, "claude", "idle", "adminifi"),
        ],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("transcri"))
        .stdout(predicate::str::contains("w1B:p1"))
        .stdout(predicate::str::contains("pane:w13:p1"))
        .stdout(predicate::str::contains("w13:p1"))
        .stdout(predicate::str::contains("ignored"))
        .stdout(predicate::str::contains("live"));
}

#[test]
fn status_is_empty_when_no_recent_sessions_exist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let state = temp.path().join("state.json");
    fs::create_dir_all(&projects).expect("projects");
    let (fake_herdr, fixture) = fake_herdr(temp.path(), &[]);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("no recent sessions"));
}

#[test]
fn status_tolerates_missing_projects_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("missing-projects");
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(temp.path(), &[]);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("no recent sessions"));
}

#[test]
fn status_json_reports_summary_for_indicator_plugins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        "claude-fable-5",
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let state = temp.path().join("state.json");
    fs::write(
        &state,
        r#"{"version":2,"sessions":{"adminifi-session":{"session_id":"adminifi-session","cwd":"/repo","last_action_unix":1783018006}}}"#,
    )
    .expect("state");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("status")
        .arg("--json")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""summary""#))
        .stdout(predicate::str::contains(r#""watched": 1"#))
        .stdout(predicate::str::contains(
            r#""last_trigger_unix": 1783018006"#,
        ));
}

#[test]
fn init_writes_config_target_override() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("config.toml");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--config")
        .arg(&config)
        .arg("init")
        .arg("--cwd-pattern")
        .arg("/repo/*")
        .arg("--target-model")
        .arg("claude-fable-5")
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));

    let config_text = fs::read_to_string(config).expect("config");
    assert!(config_text.contains("[[targets]]"));
    assert!(config_text.contains("cwd_pattern = \"/repo/*\""));
    assert!(config_text.contains("target_model = \"claude-fable-5\""));
}

#[test]
fn target_add_appends_configured_target_override() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("config.toml");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--config")
        .arg(&config)
        .arg("target")
        .arg("add")
        .arg("--session-id")
        .arg("session-1")
        .assert()
        .success()
        .stdout(predicate::str::contains("added target"));

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--config")
        .arg(&config)
        .arg("target")
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "session_id=session-1 -> claude-fable-5",
        ));

    let config_text = fs::read_to_string(config).expect("config");
    assert!(config_text.contains("[[targets]]"));
    assert!(config_text.contains("session_id = \"session-1\""));
    assert!(config_text.contains("target_model = \"claude-fable-5\""));
}

#[test]
fn setup_can_create_config_target_and_ui_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("config.toml");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--config")
        .arg(&config)
        .arg("setup")
        .arg("--session-id")
        .arg("session-1")
        .arg("--install-ui")
        .env("HOME", temp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("added target"))
        .stdout(predicate::str::contains("installed SwiftBar plugin"))
        .stdout(predicate::str::contains(
            "installed Herdr annotation LaunchAgent",
        ));

    let config_text = fs::read_to_string(&config).expect("config");
    assert!(config_text.contains("session_id = \"session-1\""));

    let plugin = temp
        .path()
        .join("Library")
        .join("Application Support")
        .join("SwiftBar")
        .join("Plugins")
        .join("counterspell.5m.sh");
    let launch_agent = temp
        .path()
        .join("Library")
        .join("LaunchAgents")
        .join("com.misty-step.counterspell.annotate-herdr.plist");
    assert!(plugin.exists());
    assert!(launch_agent.exists());
    assert!(fs::read_to_string(plugin)
        .expect("plugin")
        .contains("COUNTERSPELL_BIN=\"${COUNTERSPELL_BIN:-"));
    assert!(fs::read_to_string(launch_agent)
        .expect("launch agent")
        .contains("--annotate-herdr"));
}

#[test]
fn doctor_reports_config_targets_and_herdr_summary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        "claude-fable-5",
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("doctor")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("counterspell doctor"))
        .stdout(predicate::str::contains("targets: 1"))
        .stdout(predicate::str::contains("herdr: reachable"))
        .stdout(predicate::str::contains("watched=1"));
}

#[test]
fn status_fails_when_herdr_mapping_fails_for_existing_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        "claude-fable-5",
    );
    let config = write_config(temp.path(), "");
    let failing_herdr = failing_herdr(temp.path());
    let state = temp.path().join("state.json");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &failing_herdr)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "load Herdr panes for session status",
        ));
}

#[test]
fn watch_reports_compact_then_switch_when_drift_is_gated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("watch pass"))
        .stdout(predicate::str::contains("session_id"))
        .stdout(predicate::str::contains(
            "compact then switch:claude-fable-5",
        ));
}

#[test]
fn watch_blocks_ambiguous_same_cwd_panes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[
            ("w13:p1", &cwd, "claude", "idle", "adminifi-a"),
            ("w13:p2", &cwd, "claude", "idle", "adminifi-b"),
        ],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("ambiguous-pane:2"))
        .stdout(predicate::str::contains("compact then switch").not());
}

#[test]
fn watch_auto_targets_fable_history_without_explicit_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("auto:fable"))
        .stdout(predicate::str::contains(
            "compact then switch:claude-fable-5",
        ));
}

#[test]
fn watch_persists_action_state_and_debounces_next_pass() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let state = temp.path().join("counterspell-state.json");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "compact then switch:claude-fable-5",
        ));

    let state_json = fs::read_to_string(&state).expect("state");
    assert!(state_json.contains("adminifi-session"));
    assert!(state_json.contains("last_action_unix"));

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("debounce"))
        .stdout(predicate::str::contains("compact then switch").not());
}

#[test]
fn watch_arm_injects_compact_then_wait_then_model_switch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let state = temp.path().join("counterspell-state.json");
    let herdr_log = temp.path().join("herdr.log");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .arg("--arm")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "compact then switch:claude-fable-5",
        ));

    let log = fs::read_to_string(herdr_log).expect("herdr log");
    let compact = log.find("pane run w13:p1 /compact").expect("compact");
    let wait = log.find("wait agent-status w13:p1").expect("wait");
    let model = log
        .find("pane run w13:p1 /model claude-fable-5")
        .expect("model");
    assert!(compact < wait);
    assert!(wait < model);
}

#[test]
fn annotate_herdr_labels_watched_panes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let herdr_log = temp.path().join("herdr.log");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        "claude-fable-5",
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--annotate-herdr")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
        .assert()
        .success()
        .stdout(predicate::str::contains("annotated 1 Herdr pane"));

    let log = fs::read_to_string(herdr_log).expect("herdr log");
    assert!(log.contains("pane report-metadata w13:p1"));
    assert!(log.contains("--source counterspell"));
    assert!(log.contains("--title Counterspell: claude-fable-5"));
    assert!(log.contains("--custom-status watched"));
}

#[test]
fn watch_without_arm_is_dry_run_and_does_not_persist_action_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let state = temp.path().join("counterspell-state.json");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    let config = write_config(
        temp.path(),
        r#"
[[targets]]
session_id = "adminifi-session"
target_model = "claude-fable-5"
"#,
    );
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[("w13:p1", &cwd, "claude", "idle", "adminifi")],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("--recent-hours")
        .arg("999")
        .arg("watch")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "dry-run:compact then switch:claude-fable-5",
        ));

    assert!(!state.exists());
}

fn write_transcript(projects: &Path, project: &str, session_id: &str, cwd: &Path, model: &str) {
    write_transcript_with_models(projects, project, session_id, cwd, &[model]);
}

fn write_transcript_with_models(
    projects: &Path,
    project: &str,
    session_id: &str,
    cwd: &Path,
    models: &[&str],
) {
    let project_dir = projects.join(project);
    fs::create_dir_all(&project_dir).expect("project dir");
    let path = project_dir.join(format!("{session_id}.jsonl"));
    let mut lines = Vec::new();
    for (index, model) in models.iter().enumerate() {
        lines.push(format!(
            r#"{{"type":"assistant","sessionId":"{session_id}","timestamp":"{}","cwd":"{}","message":{{"model":"{model}"}}}}"#,
            chrono::Utc::now()
                .checked_sub_signed(chrono::Duration::seconds((models.len() - index) as i64))
                .unwrap()
                .to_rfc3339(),
            cwd.display()
        ));
    }
    fs::write(&path, format!("{}\n", lines.join("\n"))).expect("transcript");
}

fn write_config(temp_path: &Path, contents: &str) -> PathBuf {
    let path = temp_path.join("counterspell.toml");
    fs::write(&path, contents).expect("config");
    path
}

fn fake_herdr(temp_path: &Path, panes: &[(&str, &Path, &str, &str, &str)]) -> (PathBuf, PathBuf) {
    let fixture = temp_path.join("herdr.json");
    let panes = panes
        .iter()
        .map(|(pane_id, cwd, agent, status, label)| {
            serde_json::json!({
                "pane_id": pane_id,
                "cwd": cwd,
                "foreground_cwd": cwd,
                "agent": agent,
                "agent_status": status,
                "label": label
            })
        })
        .collect::<Vec<_>>();
    let herdr_json = serde_json::json!({
        "id": "cli:pane:list",
        "result": {
            "type": "pane_list",
            "panes": panes
        }
    });
    fs::write(&fixture, herdr_json.to_string()).expect("fixture");

    let fake_herdr = temp_path.join("fake-herdr");
    fs::write(
        &fake_herdr,
        r#"#!/bin/sh
if [ "$1" = "pane" ] && [ "$2" = "list" ]; then
  cat "$COUNTERSPELL_HERDR_FIXTURE"
  exit 0
fi
if [ -n "$COUNTERSPELL_HERDR_LOG" ]; then
  printf '%s\n' "$*" >> "$COUNTERSPELL_HERDR_LOG"
fi
exit 0
"#,
    )
    .expect("fake herdr");
    chmod_exec(&fake_herdr);
    (fake_herdr, fixture)
}

fn failing_herdr(temp_path: &Path) -> PathBuf {
    let fake_herdr = temp_path.join("failing-herdr");
    fs::write(&fake_herdr, "#!/bin/sh\nexit 42\n").expect("fake herdr");
    chmod_exec(&fake_herdr);
    fake_herdr
}

fn chmod_exec(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}
