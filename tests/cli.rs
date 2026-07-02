use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        .stdout(predicate::str::contains("ignored"))
        .stdout(predicate::str::contains("no-target"))
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

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--projects-dir")
        .arg(&projects)
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", "/definitely/not/herdr")
        .assert()
        .success()
        .stdout(predicate::str::contains("no recent sessions"));
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
fn watch_ignores_drift_without_explicit_target() {
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
        .stdout(predicate::str::contains("ignored:no-target"))
        .stdout(predicate::str::contains("compact then switch").not());
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
        "#!/bin/sh\ncat \"$COUNTERSPELL_HERDR_FIXTURE\"\n",
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
