use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
    let watch_agent = temp
        .path()
        .join("Library")
        .join("LaunchAgents")
        .join("com.misty-step.counterspell.watch-arm.plist");
    assert!(plugin.exists());
    assert!(launch_agent.exists());
    assert!(watch_agent.exists());
    assert!(fs::read_to_string(plugin)
        .expect("plugin")
        .contains("COUNTERSPELL_BIN=\"${COUNTERSPELL_BIN:-"));
    assert!(fs::read_to_string(launch_agent)
        .expect("launch agent")
        .contains("--annotate-herdr"));
    let watch_agent_text = fs::read_to_string(watch_agent).expect("watch agent");
    assert!(watch_agent_text.contains("watch"));
    assert!(watch_agent_text.contains("--arm"));
    // The armed watch runs on a tight interval so downgrades are answered
    // while the downgraded turn is still running.
    assert!(watch_agent_text.contains("<integer>10</integer>"));
}

#[test]
fn install_ui_loads_annotation_and_watch_arm_agents() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launchctl_log = temp.path().join("launchctl.log");
    let launchctl = fake_launchctl(temp.path(), true);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("install-ui")
        .arg("--no-swiftbar")
        .arg("--load")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .env("COUNTERSPELL_LAUNCHCTL_LOG", &launchctl_log)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "installed Herdr annotation LaunchAgent",
        ))
        .stdout(predicate::str::contains("installed watch-arm LaunchAgent"))
        .stdout(predicate::str::contains(
            "loaded com.misty-step.counterspell.annotate-herdr",
        ))
        .stdout(predicate::str::contains(
            "loaded com.misty-step.counterspell.watch-arm",
        ));

    let log = fs::read_to_string(launchctl_log).expect("launchctl log");
    assert!(log.contains("bootstrap"));
    assert!(log.contains("com.misty-step.counterspell.annotate-herdr.plist"));
    assert!(log.contains("com.misty-step.counterspell.watch-arm.plist"));
    assert!(log.contains("kickstart -k"));
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
    let launchctl = fake_launchctl(temp.path(), true);
    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("install-ui")
        .arg("--no-swiftbar")
        .arg("--load")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .assert()
        .success();
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
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .assert()
        .success()
        .stdout(predicate::str::contains("counterspell doctor"))
        .stdout(predicate::str::contains("targets: 1"))
        .stdout(predicate::str::contains("herdr: reachable"))
        .stdout(predicate::str::contains("watched=1"))
        .stdout(predicate::str::contains("watch-arm agent:"))
        .stdout(predicate::str::contains("scheduled"));
}

#[test]
fn doctor_fails_when_watch_arm_agent_is_not_scheduled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    fs::create_dir_all(&projects).expect("projects");
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(temp.path(), &[]);
    let launchctl = fake_launchctl(temp.path(), false);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("doctor")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .assert()
        .failure()
        .stdout(predicate::str::contains("watch-arm agent:"))
        .stderr(predicate::str::contains(
            "armed watch daemon is not scheduled",
        ));
}

#[test]
fn doctor_fails_when_installed_binary_predates_repo_head() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    fs::create_dir_all(&projects).expect("projects");
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");
    let stale_bin = temp.path().join("counterspell-stale");
    fs::copy(env!("CARGO_BIN_EXE_counterspell"), &stale_bin).expect("copy stale binary");
    chmod_exec(&stale_bin);
    Command::new("touch")
        .arg("-t")
        .arg("202001010000")
        .arg(&stale_bin)
        .assert()
        .success();

    let (fake_herdr, fixture) = fake_herdr(temp.path(), &[]);
    let launchctl = fake_launchctl(temp.path(), true);

    Command::new(&stale_bin)
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("doctor")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .assert()
        .failure()
        .stdout(predicate::str::contains("binary freshness: stale"))
        .stderr(predicate::str::contains(
            "installed binary is older than repo HEAD",
        ));
}

#[test]
fn doctor_fails_when_release_binary_predates_latest_release() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    fs::create_dir_all(&projects).expect("projects");
    let config = write_config(temp.path(), "");
    let state = temp.path().join("state.json");
    let (fake_herdr, fixture) = fake_herdr(temp.path(), &[]);
    let launchctl = fake_launchctl(temp.path(), true);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--config")
        .arg(&config)
        .arg("--state")
        .arg(&state)
        .arg("doctor")
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_LAUNCHCTL_BIN", &launchctl)
        .env("COUNTERSPELL_REPO_HEAD_UNIX", "none")
        .env("COUNTERSPELL_LATEST_RELEASE_VERSION", "9.9.9")
        .assert()
        .failure()
        .stdout(predicate::str::contains("binary freshness: stale"))
        .stderr(predicate::str::contains(
            "installed binary is older than latest release",
        ));
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("watch pass"))
        .stdout(predicate::str::contains("auto:fable"))
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("ambiguous-pane:2"))
        .stdout(predicate::str::contains("compact then switch").not());
}

#[test]
fn watch_blocks_same_cwd_tie_even_with_sole_focused_pane() {
    // Regression for the 2026-07-04 misfire: the focused-pane tiebreak sent
    // compact+switch keystrokes into a different live session sharing the
    // cwd. Focus must never route keystroke injection.
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
    let (fake_herdr, fixture) = fake_herdr_with_focus(
        temp.path(),
        &[
            ("w13:p1", &cwd, "claude", "idle", "adminifi-a", false),
            ("w13:p2", &cwd, "claude", "idle", "adminifi-b", true),
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("ambiguous-pane:2"))
        .stdout(predicate::str::contains("compact then switch").not());
}

#[test]
fn watch_fast_path_interrupts_compacts_and_switches_in_one_pass() {
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
    let herdr_log = temp.path().join("herdr.log");

    // Pass 1: the downgraded session's pane is WORKING. The fast path must
    // run the entire chain synchronously — interrupt (Escape), compact,
    // switch — in this single pass. It must never leave the switch for a
    // later tick to deliver (a busy session is never observably idle).
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[(
            "w13:p1",
            &cwd,
            "claude",
            "working",
            false,
            Some("adminifi-session"),
        )],
    );
    let run = |desc: &str| {
        let assert = Command::cargo_bin("counterspell")
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
            .env("HOME", temp.path())
            .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
            .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
            .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
            .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
            .assert()
            .success();
        eprintln!("--- pass: {desc}");
        assert
    };

    run("working pane").stdout(predicate::str::contains(
        "interrupt then queue-compact then switch:claude-fable-5",
    ));
    let log = fs::read_to_string(&herdr_log).expect("herdr log");
    let escape_at = log
        .find("pane send-keys w13:p1 escape")
        .expect("escape must be sent to end the current turn");
    let compact_at = log
        .find("pane run w13:p1 /compact")
        .expect("compact must be sent in the same pass");
    let switch_at = log
        .find("pane run w13:p1 /model claude-fable-5")
        .expect("switch must be sent in the same pass, never left for a later tick");
    let confirm_at = log
        .find("pane send-keys w13:p1 enter")
        .expect("model switch confirmation must be sent in the same pass");
    assert!(
        escape_at < compact_at && compact_at < switch_at && switch_at < confirm_at,
        "chain must run escape -> compact -> switch -> confirm in order, log: {log}"
    );

    // Pass 2: the switch just fired, so the session is debounced — a second
    // pass must not type anything again even if drift still shows (the
    // transcript lags the live pane).
    run("debounced follow-up").stdout(predicate::str::contains("debounce"));
    let log = fs::read_to_string(&herdr_log).expect("herdr log");
    assert_eq!(
        log.matches("/compact").count(),
        1,
        "the follow-up pass must not compact again, log: {log}"
    );
    assert_eq!(
        log.matches("/model").count(),
        1,
        "the follow-up pass must not switch again, log: {log}"
    );
    assert_eq!(
        log.matches("pane send-keys w13:p1 escape").count(),
        1,
        "the follow-up pass must not escape again, log: {log}"
    );
    assert_eq!(
        log.matches("pane send-keys w13:p1 enter").count(),
        1,
        "the follow-up pass must not confirm again, log: {log}"
    );
}

#[test]
fn watch_clears_in_flight_marker_when_chain_aborts_so_next_tick_refires() {
    // 2026-07-04 second incident: an aborted chain left pending_compact set,
    // and every subsequent tick reported compact-pending while the session
    // kept working downgraded for the marker's whole expiry.
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
    let herdr_log = temp.path().join("herdr.log");
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[(
            "w13:p1",
            &cwd,
            "claude",
            "working",
            false,
            Some("adminifi-session"),
        )],
    );

    let run = |herdr_bin: &Path, expect_success: bool| {
        let assert = Command::cargo_bin("counterspell")
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
            .env("HOME", temp.path())
            .env("COUNTERSPELL_HERDR_BIN", herdr_bin)
            .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
            .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
            .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
            .assert();
        if expect_success {
            assert.success()
        } else {
            assert.failure()
        }
    };

    // Pass 1: pane run fails mid-chain — the pass errors out, but the
    // in-flight marker must NOT survive on disk.
    let broken_herdr = fake_herdr_failing_pane_run(temp.path());
    run(&broken_herdr, false);
    let state_json = fs::read_to_string(&state).expect("state written");
    assert!(
        !state_json.contains("pending_compact_unix"),
        "aborted chain must clear the in-flight marker, state: {state_json}"
    );

    // Pass 2: herdr healthy again — the chain re-fires instead of sitting
    // behind compact-pending.
    run(&fake_herdr, true);
    let log = fs::read_to_string(&herdr_log).expect("herdr log");
    assert!(
        log.contains("pane run w13:p1 /model claude-fable-5"),
        "retry tick must deliver the switch, log: {log}"
    );
}

fn fake_herdr_failing_pane_run(temp_path: &Path) -> PathBuf {
    let fake_herdr = temp_path.join("fake-herdr-failing-run");
    fs::write(
        &fake_herdr,
        r#"#!/bin/sh
if [ "$1" = "pane" ] && [ "$2" = "list" ]; then
  if [ -n "$COUNTERSPELL_HERDR_LOG" ]; then
    printf '%s\n' "$*" >> "$COUNTERSPELL_HERDR_LOG"
  fi
  cat "$COUNTERSPELL_HERDR_FIXTURE"
  exit 0
fi
if [ -n "$COUNTERSPELL_HERDR_LOG" ]; then
  printf '%s\n' "$*" >> "$COUNTERSPELL_HERDR_LOG"
fi
if [ "$1" = "pane" ] && [ "$2" = "run" ]; then
  exit 7
fi
exit 0
"#,
    )
    .expect("fake herdr");
    chmod_exec(&fake_herdr);
    fake_herdr
}

#[test]
fn watch_routes_remediation_to_session_bound_pane_among_same_cwd_matches() {
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
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[
            // The other pane is focused AND bound to a different session:
            // under the old cwd+focus matching it would have received the
            // keystrokes. Session binding must route to w13:p1.
            (
                "w13:p1",
                &cwd,
                "claude",
                "idle",
                true,
                Some("adminifi-session"),
            ),
            (
                "w13:p2",
                &cwd,
                "claude",
                "idle",
                false,
                Some("other-session"),
            ),
        ],
    );
    let herdr_log = temp.path().join("herdr.log");

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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "compact then switch:claude-fable-5",
        ));

    let log = fs::read_to_string(&herdr_log).expect("herdr log");
    assert!(
        log.lines().any(|line| line.contains("w13:p1")),
        "remediation should target the session-bound pane, log: {log}"
    );
    assert!(
        !log.lines()
            .any(|line| line.starts_with("pane run w13:p2") || line.contains("run w13:p2")),
        "remediation must not touch the foreign-bound pane, log: {log}"
    );
}

#[test]
fn watch_self_heals_herdr_integration_when_no_pane_reports_a_session() {
    // Reproduces the 2026-07-07 incident: a settings.json rewrite dropped the
    // herdr SessionStart hook, so every claude pane stopped reporting
    // agent_session, cwd-fallback matching became permanently ambiguous, and
    // remediation silently never fired. `watch` must notice zero session
    // reporting across all claude panes and re-run the installer itself
    // rather than gate forever without a trace.
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        &["claude-fable-5"],
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
    // Two claude panes share the cwd and neither reports an agent_session —
    // exactly what herdr returns once its SessionStart hook is unwired.
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[
            ("w13:p1", &cwd, "claude", "idle", false, None),
            ("w13:p2", &cwd, "claude", "idle", false, None),
        ],
    );
    let herdr_log = temp.path().join("herdr.log");

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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "no Herdr claude pane reported an agent_session",
        ));

    let log = fs::read_to_string(&herdr_log).expect("herdr log");
    assert!(
        log.lines()
            .any(|line| line.contains("integration install claude")),
        "watch should self-heal by re-running the herdr integration installer, log: {log}"
    );
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
        .env("HOME", temp.path())
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
        .env("HOME", temp.path())
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("debounce"))
        .stdout(predicate::str::contains("compact then switch").not());
}

#[test]
fn watch_suppresses_stale_drift_after_recorded_switch_until_transcript_advances() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("repo");
    let state = temp.path().join("counterspell-state.json");
    let herdr_log = temp.path().join("herdr.log");
    fs::create_dir_all(&cwd).expect("cwd");

    let project_dir = projects.join("-Users-phaedrus-Development-adminifi");
    fs::create_dir_all(&project_dir).expect("project dir");
    fs::write(
        project_dir.join("adminifi-session.jsonl"),
        format!(
            concat!(
                "{{\"type\":\"assistant\",\"sessionId\":\"adminifi-session\",\"timestamp\":\"2026-07-02T12:00:00Z\",\"cwd\":\"{}\",\"message\":{{\"model\":\"claude-fable-5\"}}}}\n",
                "{{\"type\":\"assistant\",\"sessionId\":\"adminifi-session\",\"timestamp\":\"2026-07-02T12:01:00Z\",\"cwd\":\"{}\",\"message\":{{\"model\":\"claude-opus-4-8\"}}}}\n",
            ),
            cwd.display(),
            cwd.display()
        ),
    )
    .expect("transcript");
    let last_switch_unix = chrono::DateTime::parse_from_rfc3339("2026-07-02T12:02:00Z")
        .unwrap()
        .timestamp();
    let cwd_text = cwd.display().to_string();
    fs::write(
        &state,
        serde_json::json!({
            "version": 2,
            "sessions": {
                "adminifi-session": {
                    "session_id": "adminifi-session",
                    "cwd": cwd_text,
                    "last_action_unix": last_switch_unix
                }
            }
        })
        .to_string(),
    )
    .expect("state");
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_HERDR_LOG", &herdr_log)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success()
        .stdout(predicate::str::contains("compact then switch").not())
        .stdout(predicate::str::contains("switch:claude-fable-5").not());

    // The log now records read-only `pane list` samples too, so "no
    // injection" means no pane run / send-keys lines, not an absent file.
    let log = fs::read_to_string(&herdr_log).unwrap_or_default();
    assert!(
        !log.contains("pane run") && !log.contains("send-keys"),
        "stale drift behind a recorded switch must not inject into Herdr"
    );
}

#[test]
fn watch_arm_injects_compact_then_samples_status_then_model_switch() {
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
        .env("HOME", temp.path())
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
    // Pacing between compact and switch is a direct status sample
    // (`pane list`), not `wait agent-status`: managed panes settle to
    // `done`, which the wait command cannot express, and the old wait
    // burned its full 180s timeout on them.
    let sample = log[compact..]
        .find("pane list")
        .map(|offset| compact + offset)
        .expect("status sample after compact");
    let model = log
        .find("pane run w13:p1 /model claude-fable-5")
        .expect("model");
    let confirm = log
        .find("pane send-keys w13:p1 enter")
        .expect("model confirmation");
    assert!(compact < sample);
    assert!(sample < model);
    assert!(model < confirm);
}

#[test]
fn watch_appends_bridge_feed_events_for_drift_remediation_and_born_on_opus() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let drift_cwd = temp.path().join("drift-repo");
    let opus_cwd = temp.path().join("opus-repo");
    let state = temp.path().join("counterspell-state.json");
    let feed_dir = temp.path().join("feed");
    fs::create_dir_all(&drift_cwd).expect("drift cwd");
    fs::create_dir_all(&opus_cwd).expect("opus cwd");
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-drift",
        "drift-session",
        &drift_cwd,
        &["claude-fable-5", "claude-opus-4-8"],
    );
    write_transcript_with_models(
        &projects,
        "-Users-phaedrus-Development-opus",
        "opus-session",
        &opus_cwd,
        &["claude-opus-4-8"],
    );
    let config = write_config(temp.path(), "");
    let (fake_herdr, fixture) = fake_herdr(
        temp.path(),
        &[
            ("w13:p1", &drift_cwd, "claude", "idle", "drift"),
            ("w13:p2", &opus_cwd, "claude", "idle", "opus"),
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
        .env("HOME", temp.path())
        .env("COUNTERSPELL_FEED_DIR", &feed_dir)
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS", "0")
        .assert()
        .success();

    let feed_file = fs::read_dir(&feed_dir)
        .expect("feed dir")
        .next()
        .expect("feed file")
        .expect("feed entry")
        .path();
    let feed = fs::read_to_string(feed_file).expect("feed");
    assert!(feed.contains(r#""schema_version":"weave.remote_event.v1""#));
    assert!(feed.contains(r#""action":"model_drift_detected""#));
    assert!(feed.contains(r#""action":"compact_sent""#));
    assert!(feed.contains(r#""action":"model_switched""#));
    assert!(feed.contains(r#""action":"remediation_confirmed""#));
    assert!(feed.contains(r#""action":"session_ignored""#));
    assert!(feed.contains(r#""origin":"downgraded-from-fable""#));
    assert!(feed.contains(r#""origin":"born-on-opus""#));
    assert!(feed.contains(r#""session_id":"drift-session""#));
    assert!(feed.contains(r#""pane":"w13:p1""#));
    assert!(feed.contains(r#""from_model":"claude-fable-5""#));
    assert!(feed.contains(r#""to_model":"claude-opus-4-8""#));
    assert!(!feed.contains(drift_cwd.to_string_lossy().as_ref()));
    assert!(!feed.contains(opus_cwd.to_string_lossy().as_ref()));
    assert!(!feed.contains("message"));
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
        .env("HOME", temp.path())
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

#[test]
fn herdr_call_times_out_instead_of_hanging_forever() {
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
    let state = temp.path().join("state.json");
    let hanging_herdr = hanging_herdr(temp.path());

    let started = Instant::now();
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
        .env("COUNTERSPELL_HERDR_BIN", &hanging_herdr)
        .env("COUNTERSPELL_HERDR_TIMEOUT_MS", "200")
        .assert()
        .failure()
        .stderr(predicate::str::contains("timed out"));

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a hung herdr must not block the caller past its configured deadline"
    );
}

#[test]
fn rebind_fails_clearly_outside_a_herdr_managed_pane() {
    let temp = tempfile::tempdir().expect("tempdir");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("rebind")
        .env("HOME", temp.path())
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_SOCKET_PATH")
        .assert()
        .failure()
        .stderr(predicate::str::contains("HERDR_ENV"))
        .stderr(predicate::str::contains("herdr-managed pane"));
}

#[test]
fn rebind_fails_clearly_when_pane_id_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("rebind")
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env_remove("HERDR_PANE_ID")
        .env("HERDR_SOCKET_PATH", temp.path().join("herdr.sock"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("HERDR_PANE_ID"));
}

#[test]
fn rebind_sends_report_agent_session_matching_herdr_hook_shape() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("herdr.sock");
    let handle = spawn_fake_herdr_socket(&socket_path, r#"{"id":"x","result":{"ok":true}}"#);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("rebind")
        .arg("--session-id")
        .arg("session-xyz")
        .arg("--transcript-path")
        .arg("/repo/session-xyz.jsonl")
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", &socket_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("pane_id: w1:p1"))
        .stdout(predicate::str::contains("session_id: session-xyz"))
        .stdout(predicate::str::contains(
            "transcript_path: /repo/session-xyz.jsonl",
        ))
        .stdout(predicate::str::contains(r#""ok":true"#));

    let received = handle.join().expect("join fake herdr socket thread");
    let request: serde_json::Value =
        serde_json::from_str(received.trim()).expect("parse request as JSON");
    assert_eq!(request["method"], "pane.report_agent_session");
    assert_eq!(request["params"]["pane_id"], "w1:p1");
    assert_eq!(request["params"]["source"], "herdr:claude");
    assert_eq!(request["params"]["agent"], "claude");
    assert_eq!(request["params"]["agent_session_id"], "session-xyz");
    assert_eq!(
        request["params"]["agent_session_path"],
        "/repo/session-xyz.jsonl"
    );
    assert!(
        request["params"]["seq"].is_u64(),
        "seq must be a monotonic integer, got {:?}",
        request["params"]["seq"]
    );
    assert!(!request["id"].as_str().expect("id is a string").is_empty());
}

#[test]
fn rebind_discovers_session_from_live_transcript_for_cwd_without_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    let cwd = temp.path().join("adminifi");
    fs::create_dir_all(&cwd).expect("cwd");
    write_transcript(
        &projects,
        "-Users-phaedrus-Development-adminifi",
        "adminifi-session",
        &cwd,
        "claude-fable-5",
    );
    let socket_path = temp.path().join("herdr.sock");
    let handle = spawn_fake_herdr_socket(&socket_path, r#"{"result":{"ok":true}}"#);

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--recent-hours")
        .arg("999")
        .arg("rebind")
        .current_dir(&cwd)
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", &socket_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("session_id: adminifi-session"));

    let received = handle.join().expect("join fake herdr socket thread");
    let request: serde_json::Value =
        serde_json::from_str(received.trim()).expect("parse request as JSON");
    assert_eq!(request["params"]["agent_session_id"], "adminifi-session");
    assert!(
        request["params"]["agent_session_path"]
            .as_str()
            .expect("path is a string")
            .ends_with("adminifi-session.jsonl"),
        "discovered transcript path should point at the matched session file, got {:?}",
        request["params"]["agent_session_path"]
    );
}

#[test]
fn rebind_errors_clearly_when_no_transcript_matches_cwd_and_no_override_given() {
    let temp = tempfile::tempdir().expect("tempdir");
    let projects = temp.path().join("projects");
    fs::create_dir_all(&projects).expect("projects");
    let cwd = temp.path().join("empty-repo");
    fs::create_dir_all(&cwd).expect("cwd");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("rebind")
        .current_dir(&cwd)
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", temp.path().join("herdr.sock"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--session-id"))
        .stderr(predicate::str::contains("--transcript-path"));
}

#[test]
fn rebind_verify_confirms_pane_now_reports_the_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("herdr.sock");
    let handle = spawn_fake_herdr_socket(&socket_path, r#"{"result":{"ok":true}}"#);
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[(
            "w1:p1",
            Path::new("/nonexistent"),
            "claude",
            "idle",
            false,
            Some("session-xyz"),
        )],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("rebind")
        .arg("--session-id")
        .arg("session-xyz")
        .arg("--verify")
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", &socket_path)
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "verify: pane w1:p1 now reports session session-xyz",
        ));

    handle.join().expect("join fake herdr socket thread");
}

#[test]
fn rebind_verify_fails_when_pane_still_reports_a_different_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let socket_path = temp.path().join("herdr.sock");
    let handle = spawn_fake_herdr_socket(&socket_path, r#"{"result":{"ok":true}}"#);
    let (fake_herdr, fixture) = fake_herdr_with_sessions(
        temp.path(),
        &[(
            "w1:p1",
            Path::new("/nonexistent"),
            "claude",
            "idle",
            false,
            Some("stale-session"),
        )],
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("rebind")
        .arg("--session-id")
        .arg("session-xyz")
        .arg("--verify")
        .env("HOME", temp.path())
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "w1:p1")
        .env("HERDR_SOCKET_PATH", &socket_path)
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not report session session-xyz yet",
        ));

    handle.join().expect("join fake herdr socket thread");
}

/// A minimal fixture standing in for herdr's unix socket: accepts exactly one
/// connection, reads one newline-delimited JSON request line, writes back the
/// given response line, and returns the raw request line to the caller for
/// assertions. Mirrors the protocol `~/.claude/hooks/herdr-agent-state.sh`
/// speaks (and `rebind` reuses): connect, write request + "\n", best-effort
/// read one response line.
fn spawn_fake_herdr_socket(socket_path: &Path, response_line: &str) -> JoinHandle<String> {
    let listener = UnixListener::bind(socket_path).expect("bind fake herdr socket");
    let response_line = response_line.to_string();
    thread::spawn(move || {
        let (stream, _addr) = listener.accept().expect("accept fake herdr connection");
        let mut reader = BufReader::new(stream.try_clone().expect("clone fake herdr stream"));
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .expect("read request line from fake herdr client");
        let mut writer = stream;
        writeln!(writer, "{response_line}").expect("write fake herdr response");
        request_line
    })
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
    let panes = panes
        .iter()
        .map(|(pane_id, cwd, agent, status, label)| {
            (*pane_id, *cwd, *agent, *status, *label, false)
        })
        .collect::<Vec<_>>();
    fake_herdr_with_focus(temp_path, &panes)
}

fn fake_herdr_with_focus(
    temp_path: &Path,
    panes: &[(&str, &Path, &str, &str, &str, bool)],
) -> (PathBuf, PathBuf) {
    let fixture = temp_path.join("herdr.json");
    let panes = panes
        .iter()
        .map(|(pane_id, cwd, agent, status, label, focused)| {
            serde_json::json!({
                "pane_id": pane_id,
                "cwd": cwd,
                "foreground_cwd": cwd,
                "agent": agent,
                "agent_status": status,
                "label": label,
                "focused": focused
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
  if [ -n "$COUNTERSPELL_HERDR_LOG" ]; then
    printf '%s\n' "$*" >> "$COUNTERSPELL_HERDR_LOG"
  fi
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

/// (pane_id, cwd, agent, agent_status, focused, reported session id)
type SessionPaneFixture<'a> = (&'a str, &'a Path, &'a str, &'a str, bool, Option<&'a str>);

fn fake_herdr_with_sessions(
    temp_path: &Path,
    panes: &[SessionPaneFixture<'_>],
) -> (PathBuf, PathBuf) {
    let fixture = temp_path.join("herdr.json");
    let panes = panes
        .iter()
        .map(|(pane_id, cwd, agent, status, focused, session_id)| {
            let mut pane = serde_json::json!({
                "pane_id": pane_id,
                "cwd": cwd,
                "foreground_cwd": cwd,
                "agent": agent,
                "agent_status": status,
                "focused": focused
            });
            if let Some(session_id) = session_id {
                pane["agent_session"] = serde_json::json!({
                    "source": "herdr:claude",
                    "agent": agent,
                    "kind": "id",
                    "value": session_id
                });
            }
            pane
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
  if [ -n "$COUNTERSPELL_HERDR_LOG" ]; then
    printf '%s\n' "$*" >> "$COUNTERSPELL_HERDR_LOG"
  fi
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

fn hanging_herdr(temp_path: &Path) -> PathBuf {
    let fake_herdr = temp_path.join("hanging-herdr");
    fs::write(&fake_herdr, "#!/bin/sh\nsleep 999\n").expect("hanging herdr");
    chmod_exec(&fake_herdr);
    fake_herdr
}

fn fake_launchctl(temp_path: &Path, scheduled: bool) -> PathBuf {
    let fake_launchctl = temp_path.join("fake-launchctl");
    let print_exit = if scheduled { 0 } else { 3 };
    fs::write(
        &fake_launchctl,
        format!(
            r#"#!/bin/sh
if [ -n "$COUNTERSPELL_LAUNCHCTL_LOG" ]; then
  printf '%s\n' "$*" >> "$COUNTERSPELL_LAUNCHCTL_LOG"
fi
if [ "$1" = "print" ]; then
  case "$2" in
    *com.misty-step.counterspell.watch-arm) exit {print_exit} ;;
    *) exit 3 ;;
  esac
fi
exit 0
"#
        ),
    )
    .expect("fake launchctl");
    chmod_exec(&fake_launchctl);
    fake_launchctl
}

fn chmod_exec(path: &Path) {
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}
