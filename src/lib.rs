mod cli;
mod config;
mod dashboard;
mod dashboard_render;
mod defaults;
mod events;
mod herdr;
mod indicators;
mod master;
mod model;
mod output;
mod rebind;
mod remediation;
mod sessions;
mod status;
mod store;
mod util;

pub mod api;

pub use cli::{run, run_from_args, Cli};

#[cfg(test)]
pub(crate) use chrono::{DateTime, Duration, Utc};
#[cfg(test)]
pub(crate) use config::validate_targets;
#[cfg(test)]
pub(crate) use herdr::HerdrPane;
#[cfg(test)]
pub(crate) use model::{
    Config, FileConfig, GateBlocker, ModelDrift, PlannedAction, SessionState, TargetRule,
    TranscriptSession, WatchArmDaemonStatus, WatchStore,
};
#[cfg(test)]
pub(crate) use rebind::{build_report_request, resolve_target_session};
#[cfg(test)]
pub(crate) use remediation::{
    describe_gate, detect_drift, format_target_match, gate_decision_for_matches, remediation_plan,
    status_state, target_for_session,
};
#[cfg(test)]
pub(crate) use sessions::parse_transcript_file;
#[cfg(test)]
pub(crate) use status::status_rows;
#[cfg(test)]
pub(crate) use std::{fs::File, path::PathBuf};

#[cfg(test)]
mod tests;
