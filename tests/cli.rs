use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn watch_records_current_codex_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).expect("workspace");
    let state = temp.path().join("sessions.json");
    let canonical_cwd = cwd.canonicalize().expect("canonical cwd");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(&cwd)
        .env("CODEX_THREAD_ID", "session-123")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .success()
        .stdout(predicate::str::contains("watching session session-123"))
        .stdout(predicate::str::contains(
            canonical_cwd.display().to_string(),
        ));
}

#[test]
fn status_maps_watched_session_to_herdr_pane_by_cwd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).expect("workspace");
    let state = temp.path().join("sessions.json");
    let canonical_cwd = cwd.canonicalize().expect("canonical cwd");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(&cwd)
        .env("CODEX_THREAD_ID", "session-123")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .success();

    let fixture = temp.path().join("herdr.json");
    let herdr_json = serde_json::json!({
        "id": "cli:pane:list",
        "result": {
            "type": "pane_list",
            "panes": [
                {
                    "pane_id": "pane-1",
                    "cwd": canonical_cwd,
                    "foreground_cwd": canonical_cwd,
                    "agent_session": {
                        "value": "session-123"
                    },
                    "agent": "codex",
                    "agent_status": "working",
                    "label": "counterspell"
                }
            ]
        }
    });
    fs::write(&fixture, herdr_json.to_string()).expect("fixture");

    let fake_herdr = temp.path().join("fake-herdr");
    fs::write(
        &fake_herdr,
        "#!/bin/sh\ncat \"$COUNTERSPELL_HERDR_FIXTURE\"\n",
    )
    .expect("fake herdr");
    let mut perms = fs::metadata(&fake_herdr).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_herdr, perms).expect("chmod");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("watched sessions"))
        .stdout(predicate::str::contains("session-123"))
        .stdout(predicate::str::contains("pane-1"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("working"));
}

#[test]
fn status_is_empty_when_no_sessions_are_watched() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = temp.path().join("sessions.json");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", "/bin/false")
        .assert()
        .success()
        .stdout(predicate::str::contains("no watched sessions"));
}

#[test]
fn status_fails_when_herdr_mapping_fails_for_watched_sessions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = temp.path().join("sessions.json");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(temp.path())
        .env("CODEX_THREAD_ID", "session-789")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .success();

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", "/bin/false")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "load Herdr panes for watched-session status",
        ));
}

#[test]
fn status_reports_not_open_when_watched_cwd_has_no_live_pane() {
    let temp = tempfile::tempdir().expect("tempdir");
    let watched_cwd = temp.path().join("watched");
    let other_cwd = temp.path().join("other");
    fs::create_dir(&watched_cwd).expect("watched cwd");
    fs::create_dir(&other_cwd).expect("other cwd");
    let state = temp.path().join("sessions.json");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(&watched_cwd)
        .env("CODEX_THREAD_ID", "session-000")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .success();

    let (fake_herdr, fixture) = fake_herdr_with_session(
        temp.path(),
        &other_cwd.canonicalize().unwrap(),
        "other-session",
    );

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("status")
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .assert()
        .success()
        .stdout(predicate::str::contains("session-000"))
        .stdout(predicate::str::contains("not-open"));
}

#[test]
fn watch_accepts_relative_state_file_in_current_directory() {
    let temp = tempfile::tempdir().expect("tempdir");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg("sessions.json")
        .arg("watch")
        .current_dir(temp.path())
        .env("CODEX_THREAD_ID", "session-456")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .success()
        .stdout(predicate::str::contains("watching session session-456"));

    assert!(temp.path().join("sessions.json").exists());
}

#[test]
fn watch_uses_herdr_agent_session_when_codex_env_is_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).expect("workspace");
    let state = temp.path().join("sessions.json");
    let canonical_cwd = cwd.canonicalize().expect("canonical cwd");
    let (fake_herdr, fixture) = fake_herdr_with_session(temp.path(), &canonical_cwd, "session-abc");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(&cwd)
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("HERDR_PANE_ID")
        .assert()
        .success()
        .stdout(predicate::str::contains("watching session session-abc"));
}

#[test]
fn watch_fails_when_current_codex_session_cannot_be_found() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = temp.path().join("sessions.json");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(temp.path())
        .env("COUNTERSPELL_HERDR_BIN", "/bin/false")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("HERDR_PANE_ID")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not determine current Codex session from environment or Herdr",
        ));
}

#[test]
fn watch_fails_when_current_herdr_pane_has_no_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    fs::create_dir(&cwd).expect("workspace");
    let state = temp.path().join("sessions.json");
    let canonical_cwd = cwd.canonicalize().expect("canonical cwd");

    let fixture = temp.path().join("herdr.json");
    let herdr_json = serde_json::json!({
        "id": "cli:pane:list",
        "result": {
            "type": "pane_list",
            "panes": [
                {
                    "pane_id": "pane-current",
                    "cwd": canonical_cwd,
                    "foreground_cwd": canonical_cwd,
                    "agent": "codex",
                    "agent_status": "working",
                    "label": "current"
                },
                {
                    "pane_id": "pane-other",
                    "cwd": canonical_cwd,
                    "foreground_cwd": canonical_cwd,
                    "agent_session": {
                        "value": "other-session"
                    },
                    "agent": "codex",
                    "agent_status": "working",
                    "label": "other"
                }
            ]
        }
    });
    fs::write(&fixture, herdr_json.to_string()).expect("fixture");

    let fake_herdr = temp.path().join("fake-herdr");
    fs::write(
        &fake_herdr,
        "#!/bin/sh\ncat \"$COUNTERSPELL_HERDR_FIXTURE\"\n",
    )
    .expect("fake herdr");
    let mut perms = fs::metadata(&fake_herdr).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_herdr, perms).expect("chmod");

    Command::cargo_bin("counterspell")
        .expect("binary")
        .arg("--state")
        .arg(&state)
        .arg("watch")
        .current_dir(&cwd)
        .env("COUNTERSPELL_HERDR_BIN", &fake_herdr)
        .env("COUNTERSPELL_HERDR_FIXTURE", &fixture)
        .env("HERDR_PANE_ID", "pane-current")
        .env_remove("COUNTERSPELL_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "current Herdr pane pane-current does not expose agent_session.value",
        ));
}

fn fake_herdr_with_session(
    temp_path: &std::path::Path,
    cwd: &std::path::Path,
    session_id: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let fixture = temp_path.join("herdr.json");
    let herdr_json = serde_json::json!({
        "id": "cli:pane:list",
        "result": {
            "type": "pane_list",
            "panes": [
                {
                    "pane_id": "pane-1",
                    "cwd": cwd,
                    "foreground_cwd": cwd,
                    "agent_session": {
                        "value": session_id
                    },
                    "agent": "codex",
                    "agent_status": "working",
                    "label": "counterspell"
                }
            ]
        }
    });
    fs::write(&fixture, herdr_json.to_string()).expect("fixture");

    let fake_herdr = temp_path.join("fake-herdr");
    fs::write(
        &fake_herdr,
        "#!/bin/sh\ncat \"$COUNTERSPELL_HERDR_FIXTURE\"\n",
    )
    .expect("fake herdr");
    let mut perms = fs::metadata(&fake_herdr).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_herdr, perms).expect("chmod");
    (fake_herdr, fixture)
}
