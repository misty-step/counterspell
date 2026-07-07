use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::defaults::STORE_VERSION;

#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) projects_dir: PathBuf,
    pub(crate) recent_hours: u64,
    pub(crate) targets: Vec<TargetRule>,
    pub(crate) transcript_quiet_seconds: u64,
    pub(crate) debounce_seconds: u64,
}

/// The OTHER axis of "is counterspell live," independent of the master
/// switch flag: whether the watch-arm LaunchAgent is actually installed and
/// scheduled to tick at all. The flag alone can lie by omission — "flag says
/// enabled" means nothing if the daemon was never (re)loaded. The dashboard
/// must show both axes so that combination is never silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchArmDaemonStatus {
    /// No plist on disk — `counterspell install-ui`/`setup` has never run.
    NotInstalled,
    /// Plist exists but launchd isn't running it (unloaded and/or
    /// persistently `launchctl disable`d).
    NotScheduled,
    /// Loaded and scheduled: it will actually tick.
    Scheduled,
}

impl WatchArmDaemonStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            WatchArmDaemonStatus::NotInstalled => "not installed",
            WatchArmDaemonStatus::NotScheduled => "not scheduled",
            WatchArmDaemonStatus::Scheduled => "scheduled",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FileConfig {
    pub(crate) projects_dir: Option<PathBuf>,
    pub(crate) recent_hours: Option<u64>,
    #[serde(default)]
    pub(crate) targets: Vec<TargetRule>,
    pub(crate) transcript_quiet_seconds: Option<u64>,
    pub(crate) debounce_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct TargetRule {
    pub(crate) session_id: Option<String>,
    pub(crate) project_pattern: Option<String>,
    pub(crate) cwd_pattern: Option<String>,
    pub(crate) target_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetMatch {
    pub(crate) target_model: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WatchStore {
    pub(crate) version: u8,
    pub(crate) sessions: BTreeMap<String, SessionState>,
}

impl Default for WatchStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionState {
    pub(crate) session_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) last_action_unix: Option<u64>,
    /// In-flight/crash marker. Persisted to disk BEFORE an interrupt-driven
    /// remediation chain starts typing, cleared when the chain completes (or
    /// when drift is gone). While set and unexpired, no second chain may
    /// fire at the same session — that is the double-Escape guard. If the
    /// daemon dies mid-chain the marker expires and the ordinary idle path
    /// (compact-then-switch, which is safe to repeat) recovers the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_compact_unix: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptSession {
    pub(crate) session_id: String,
    pub(crate) project: String,
    pub(crate) cwd: Option<String>,
    pub(crate) last_event_at: DateTime<Utc>,
    pub(crate) latest_model: Option<String>,
    pub(crate) latest_model_at: Option<DateTime<Utc>>,
    pub(crate) model_history: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StatusRow {
    pub(crate) session_id: String,
    pub(crate) project: String,
    pub(crate) cwd: String,
    pub(crate) pane: String,
    pub(crate) agent: String,
    pub(crate) state: String,
    pub(crate) watch: String,
    pub(crate) target: String,
    pub(crate) model: String,
    pub(crate) drift: String,
    pub(crate) updated: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct WatchRow {
    pub(crate) session_id: String,
    pub(crate) pane: String,
    pub(crate) model: String,
    pub(crate) target: String,
    pub(crate) gate: String,
    pub(crate) actions: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct StatusOutput<'a> {
    pub(crate) summary: StatusSummary,
    pub(crate) rows: &'a [StatusRow],
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatusSummary {
    pub(crate) total: usize,
    pub(crate) watched: usize,
    pub(crate) ignored: usize,
    pub(crate) mapped: usize,
    pub(crate) live_panes: usize,
    pub(crate) last_trigger_event: Option<String>,
    pub(crate) last_trigger_unix: Option<u64>,
    /// Global master switch: true means `watch --arm` is refusing to act
    /// regardless of drift or targets.
    pub(crate) master_disarmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelDrift {
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedAction {
    /// Idle path: type /compact and hold until the pane is idle again
    /// before the follow-up switch goes out.
    Compact,
    /// Fast path opener: press Escape in a working pane to end the current
    /// turn immediately (interrupt, not kill). A downgraded session must
    /// not keep burning the wrong model for the rest of a long turn —
    /// 2026-07-04 postmortem. The follow-up idle wait is best-effort only;
    /// the rest of the chain is queue-safe by design.
    Interrupt,
    /// Fast path: type /compact WITHOUT waiting for it to run. Composer
    /// inputs execute in FIFO order at turn end, so the switch typed right
    /// behind this executes after the compact — on the small post-compact
    /// context, dialog-free — no matter how busy the pane is. Never gate
    /// the chain on observing pane state between steps: status reporting
    /// lags and queued messages steal idle windows (2026-07-04, twice).
    QueueCompact,
    SwitchModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateBlocker {
    NoPane,
    AmbiguousPane(usize),
    TranscriptActive,
    PaneBusy(String),
    Debounce,
    /// An interrupt-driven remediation chain is (or may be) in flight for
    /// this session — persisted before the chain starts typing. Blocks a
    /// second chain from double-firing until it completes or expires.
    CompactPending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateDecision {
    pub(crate) blockers: Vec<GateBlocker>,
}

impl GateDecision {
    pub(crate) fn is_allowed(&self) -> bool {
        self.blockers.is_empty()
    }
}

#[derive(Debug)]
pub(crate) struct RemediationPlan {
    pub(crate) gate: GateDecision,
    pub(crate) actions: Vec<PlannedAction>,
}
