use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::indicators::ensure_watch_arm_loaded;
use crate::util::home_dir;

/// Presence of this marker file is the single global switch: while it
/// exists, `watch --arm` may not plan or execute any remediation, no matter
/// what the per-session `[[targets]]` config says. Absence means
/// counterspell is allowed to act (the pre-master-switch default, so
/// existing installs and tests are unaffected until someone opts in to
/// disabling). A plain file, not a config field, so a human — or a crashed
/// process — can flip it with `rm`/`touch` alone, and the hot path can
/// check it with a single `Path::exists`.
pub(crate) fn marker_path(marker_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = marker_arg {
        return Ok(path);
    }
    if let Some(path) = env::var_os("COUNTERSPELL_DISARM_MARKER") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".counterspell").join("disarmed"))
}

pub(crate) fn is_disarmed(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn state_label(disarmed: bool) -> &'static str {
    if disarmed {
        "DISABLED"
    } else {
        "ENABLED"
    }
}

fn disarm(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create disarm marker dir {}", parent.display()))?;
    }
    fs::write(
        path,
        format!("disarmed at {}\n", chrono::Utc::now().to_rfc3339()),
    )
    .with_context(|| format!("write disarm marker {}", path.display()))
}

fn arm(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("remove disarm marker {}", path.display()))
        }
    }
}

pub(crate) struct EnableOutcome {
    /// Whether a watch-arm LaunchAgent plist was found and (re)loaded. False
    /// means none is installed yet — a valid state (e.g. before
    /// `counterspell install-ui` has ever run); the flag still flips.
    pub(crate) launch_agent_loaded: bool,
}

/// Single code path for turning counterspell back on, shared by the CLI
/// CLI-only: `counterspell enable` at a real terminal. Order matters: the
/// LaunchAgent is (re)loaded FIRST, while the marker file still exists, so a
/// slow or failing load can never race a tick against an already-cleared
/// flag; the marker only comes off once the agent is confirmed loaded (or
/// confirmed absent). Deliberately NOT used by the dashboard route — see
/// `enable_flag_only`.
pub(crate) fn enable(marker_path: &Path, home: &Path) -> Result<EnableOutcome> {
    let launch_agent_loaded = ensure_watch_arm_loaded(home)?;
    arm(marker_path)?;
    Ok(EnableOutcome {
        launch_agent_loaded,
    })
}

/// Dashboard-only: flips the marker without touching launchd at all. A
/// browser-triggered POST must never be able to reach `launchctl` — that
/// reach-through is exactly how a single missed `$HOME` override during this
/// feature's own live verification reloaded the real watch-arm LaunchAgent
/// against real sessions. The dashboard toggle is pause/resume for an
/// already-loaded daemon; reviving a cold (unloaded/launchd-disabled) one is
/// a deliberate terminal action (`counterspell enable`, or `install-ui
/// --load`), never a click.
pub(crate) fn enable_flag_only(marker_path: &Path) -> Result<()> {
    arm(marker_path)
}

/// Single code path for turning counterspell off, shared by the CLI
/// `disable` command and the dashboard's `/master/disable` route. Deliberately
/// touches nothing but the marker file — no launchctl call — so disabling is
/// instant even from a browser-triggered dashboard request.
pub(crate) fn disable(marker_path: &Path) -> Result<()> {
    disarm(marker_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_marker_is_not_disarmed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("nested").join("disarmed");
        assert!(!is_disarmed(&marker));
    }

    #[test]
    fn disarm_then_arm_round_trips() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("nested").join("disarmed");

        disarm(&marker).expect("disarm");
        assert!(is_disarmed(&marker));
        assert!(marker.exists());

        arm(&marker).expect("arm");
        assert!(!is_disarmed(&marker));
    }

    #[test]
    fn arm_is_idempotent_when_marker_absent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let marker = temp.path().join("disarmed");
        arm(&marker).expect("arm on absent marker must not error");
        assert!(!is_disarmed(&marker));
    }

    #[test]
    fn state_label_matches_disarmed_flag() {
        assert_eq!(state_label(true), "DISABLED");
        assert_eq!(state_label(false), "ENABLED");
    }

    #[test]
    fn marker_path_uses_explicit_arg_when_given() {
        let temp = tempfile::tempdir().expect("tempdir");
        let explicit = temp.path().join("explicit-marker");
        let resolved = marker_path(Some(explicit.clone())).expect("marker path");
        assert_eq!(resolved, explicit);
    }
}
