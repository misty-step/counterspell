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
struct HerdrWorkspaceList {
    result: HerdrWorkspaceListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrTabList {
    result: HerdrTabListResult,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrPaneListResult {
    #[serde(default)]
    panes: Vec<HerdrPane>,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrWorkspaceListResult {
    #[serde(default)]
    workspaces: Vec<HerdrWorkspace>,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrTabListResult {
    #[serde(default)]
    tabs: Vec<HerdrTab>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrPane {
    #[serde(default)]
    pub(crate) pane_id: String,
    #[serde(default)]
    pub(crate) workspace_id: String,
    #[serde(default)]
    pub(crate) tab_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) foreground_cwd: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) agent_status: Option<String>,
    #[serde(default)]
    pub(crate) focused: bool,
    pub(crate) title: Option<String>,
    pub(crate) custom_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrWorkspace {
    #[serde(default)]
    pub(crate) workspace_id: String,
    pub(crate) label: Option<String>,
    pub(crate) number: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HerdrTab {
    #[serde(default)]
    pub(crate) tab_id: String,
    pub(crate) label: Option<String>,
    pub(crate) number: Option<u64>,
}

pub(crate) fn load_herdr_panes() -> Result<Vec<HerdrPane>> {
    let output = run_herdr_args(&["pane", "list"])?;
    parse_herdr_panes(&output.stdout)
}

pub(crate) fn load_herdr_workspaces() -> Result<Vec<HerdrWorkspace>> {
    let output = run_herdr_args(&["workspace", "list"])?;
    parse_herdr_workspaces(&output.stdout)
}

pub(crate) fn load_herdr_tabs(workspace_id: &str) -> Result<Vec<HerdrTab>> {
    let output = run_herdr_args(&["tab", "list", "--workspace", workspace_id])?;
    parse_herdr_tabs(&output.stdout)
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

fn parse_herdr_workspaces(bytes: &[u8]) -> Result<Vec<HerdrWorkspace>> {
    let workspace_list: HerdrWorkspaceList =
        serde_json::from_slice(bytes).context("parse `herdr workspace list` JSON")?;
    Ok(workspace_list.result.workspaces)
}

fn parse_herdr_tabs(bytes: &[u8]) -> Result<Vec<HerdrTab>> {
    let tab_list: HerdrTabList =
        serde_json::from_slice(bytes).context("parse `herdr tab list` JSON")?;
    Ok(tab_list.result.tabs)
}

fn pane_cwds(pane: &HerdrPane) -> impl Iterator<Item = &str> {
    [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
        .into_iter()
        .flatten()
}
