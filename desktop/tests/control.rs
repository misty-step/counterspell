//! Control round-trips exercised against ISOLATED dirs only — never the real
//! `~/.counterspell/disarmed` marker or `~/.counterspell/config.toml`. Proves
//! the two write surfaces the desktop app drives (`set_master`,
//! `set_session_enabled`) flip exactly the state the daemon reads.
//!
//! One `#[test]` on purpose: it sets process-global env overrides, so keeping
//! it single avoids cross-test env races.

use std::fs;

#[test]
fn control_surfaces_round_trip_against_isolated_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let marker = temp.path().join("disarmed");
    let config = temp.path().join("config.toml");
    // Safe: the only test in this binary; env is process-global but uncontended.
    std::env::set_var("COUNTERSPELL_DISARM_MARKER", &marker);
    std::env::set_var("COUNTERSPELL_CONFIG", &config);

    // Master switch: absent marker == ENABLED (the pre-master-switch default).
    assert!(!marker.exists(), "marker should start absent");

    // Disarm writes the marker (flag only — no launchctl).
    counterspell::api::set_master(false).expect("disarm");
    assert!(marker.exists(), "disarm must write the marker");

    // Re-arm removes it.
    counterspell::api::set_master(true).expect("re-arm");
    assert!(!marker.exists(), "re-arm must remove the marker");

    // Per-session override: enabling adds an explicit session_id target.
    let session = "sess-isolated-abc";
    counterspell::api::set_session_enabled(session, true).expect("enable session target");
    let written = fs::read_to_string(&config).expect("config written");
    assert!(
        written.contains(session),
        "config must carry the session target"
    );
    assert!(written.contains("[[targets]]"), "must write a target block");

    // Disabling removes it again.
    counterspell::api::set_session_enabled(session, false).expect("disable session target");
    let after = fs::read_to_string(&config).expect("config still present");
    assert!(
        !after.contains(session),
        "config must drop the session target"
    );

    std::env::remove_var("COUNTERSPELL_DISARM_MARKER");
    std::env::remove_var("COUNTERSPELL_CONFIG");
}
