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
    /// Legacy crash marker from the pre-step-chain implementation. Kept
    /// readable so older state files still block duplicate compacts until
    /// the new state machine overwrites them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_compact_unix: Option<u64>,
    /// Durable remediation chain state. Persisted before each Herdr send, so
    /// repeated armed passes can resume or block from evidence instead of
    /// re-sending the same compact behind a long running turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) remediation_chain: Option<RemediationChainState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RemediationChainState {
    pub(crate) target_model: String,
    pub(crate) started_unix: u64,
    pub(crate) step_sent_unix: u64,
    pub(crate) step: RemediationStep,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recovery_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum RemediationStep {
    #[serde(rename = "interrupt_sent")]
    Interrupt,
    #[serde(rename = "compact_sent")]
    Compact,
    #[serde(rename = "switch_sent")]
    Switch,
    #[serde(rename = "continue_sent")]
    Continue,
}

impl RemediationStep {
    pub(crate) fn label(self) -> &'static str {
        match self {
            RemediationStep::Interrupt => "interrupt-sent",
            RemediationStep::Compact => "compact-sent",
            RemediationStep::Switch => "switch-sent",
            RemediationStep::Continue => "continue-sent",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptSession {
    pub(crate) session_id: String,
    pub(crate) project: String,
    pub(crate) cwd: Option<String>,
    pub(crate) last_event_at: DateTime<Utc>,
    pub(crate) latest_model: Option<String>,
    pub(crate) latest_model_at: Option<DateTime<Utc>>,
    pub(crate) latest_compact_at: Option<DateTime<Utc>>,
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
    /// Type the plain-handoff /compact command. Sequencing after this is
    /// evidence-based: a later pass must see the compact summary in the
    /// transcript before sending the model switch.
    Compact,
    /// Press Escape in the bound pane to end the current turn immediately
    /// (interrupt, not kill). A downgraded session must not keep burning the
    /// wrong model for the rest of a long turn.
    Interrupt,
    SwitchModel(String),
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateBlocker {
    NoPane,
    AmbiguousPane(usize),
    TranscriptActive,
    PaneBusy(String),
    Debounce,
    /// Legacy pending-compact marker from older state files.
    CompactPending,
    /// A remediation chain is in flight and not timed out. Blocks all
    /// duplicate sends until transcript evidence advances the chain or
    /// completes it.
    RemediationInFlight(RemediationStep),
    /// A chain timed out. The watch pass may attempt a deliberate recovery,
    /// but this marker is rendered so the event stream and CLI expose why a
    /// repeated send is not blind.
    RemediationTimedOut(RemediationStep),
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
    pub(crate) recovery_reason: Option<String>,
}
