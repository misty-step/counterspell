use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use crate::util::normalize_path;

#[derive(Debug, Clone, Deserialize)]
struct HerdrPaneList {
    result: HerdrPaneListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrPaneListResult {
    #[serde(default)]
    panes: Vec<HerdrPane>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrPane {
    #[serde(default)]
    pub(crate) pane_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) foreground_cwd: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) agent_status: Option<String>,
}

pub(crate) fn load_herdr_panes() -> Result<Vec<HerdrPane>> {
    let output = run_herdr_args(&["pane", "list"])?;
    parse_herdr_panes(&output.stdout)
}

pub(crate) fn run_herdr_args(args: &[&str]) -> Result<std::process::Output> {
    let herdr_bin =
        env::var_os("COUNTERSPELL_HERDR_BIN").unwrap_or_else(|| OsString::from("herdr"));
    let output = ProcessCommand::new(&herdr_bin)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "run {}; Herdr must be installed and running for pane discovery/injection",
                PathBuf::from(&herdr_bin).display()
            )
        })?;

    if !output.status.success() {
        bail!(
            "{} {:?} exited with {}; Herdr must be running and reachable",
            PathBuf::from(&herdr_bin).display(),
            args,
            output.status
        );
    }

    Ok(output)
}

pub(crate) fn annotate_herdr_pane(pane_id: &str, title: &str, status: &str) -> Result<()> {
    run_herdr_args(&[
        "pane",
        "report-metadata",
        pane_id,
        "--source",
        "counterspell",
        "--title",
        title,
        "--custom-status",
        status,
        "--ttl-ms",
        "300000",
    ])
    .with_context(|| format!("annotate Herdr pane {pane_id}"))?;
    Ok(())
}

pub(crate) fn matching_panes_for_cwd<'a>(cwd: &str, panes: &'a [HerdrPane]) -> Vec<&'a HerdrPane> {
    let normalized_cwd = normalize_path(cwd);
    let mut matches = panes
        .iter()
        .filter(|pane| pane.agent.as_deref() == Some("claude"))
        .filter(|pane| pane_cwds(pane).any(|pane_cwd| normalize_path(pane_cwd) == normalized_cwd))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| pane_id(left).cmp(pane_id(right)));
    matches
}

pub(crate) fn pane_id(pane: &HerdrPane) -> &str {
    if pane.pane_id.is_empty() {
        "unknown"
    } else {
        pane.pane_id.as_str()
    }
}

fn parse_herdr_panes(bytes: &[u8]) -> Result<Vec<HerdrPane>> {
    let pane_list: HerdrPaneList =
        serde_json::from_slice(bytes).context("parse `herdr pane list` JSON")?;
    Ok(pane_list.result.panes)
}

fn pane_cwds(pane: &HerdrPane) -> impl Iterator<Item = &str> {
    [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
        .into_iter()
        .flatten()
}
