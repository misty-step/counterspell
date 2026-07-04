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
    /// Set when a `/compact` was queued into a still-working pane (fast
    /// path). The follow-up pass sends the bare model switch once the pane
    /// is idle, then clears this.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelDrift {
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedAction {
    Compact,
    /// Fast path: type the compact command into a still-working pane. It
    /// queues in the composer and executes the moment the current turn
    /// ends. No wait, no follow-up switch in the same pass.
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
    /// A fast-path compact is queued in the pane and has not completed yet.
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
