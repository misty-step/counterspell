use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_VERSION: u8 = 1;

#[derive(Debug, Parser)]
#[command(name = "counterspell")]
#[command(about = "Watch Codex sessions and map them to Herdr panes.")]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    state: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Record the current session and cwd in the watch list.
    Watch,
    /// Show watched sessions and their matching Herdr panes.
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchStore {
    version: u8,
    sessions: BTreeMap<String, WatchedSession>,
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
struct WatchedSession {
    session_id: String,
    session_source: String,
    cwd: String,
    watched_at_unix: u64,
}

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
struct HerdrPane {
    #[serde(default)]
    pane_id: String,
    cwd: Option<String>,
    foreground_cwd: Option<String>,
    agent_session: Option<HerdrAgentSession>,
    agent: Option<String>,
    agent_status: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct HerdrAgentSession {
    value: Option<String>,
}

#[derive(Debug)]
struct StatusRow {
    session_id: String,
    cwd: String,
    panes: String,
    agents: String,
    states: String,
    labels: String,
    watched_at_unix: String,
}

pub fn run_from_args() -> Result<()> {
    let cli = Cli::parse();
    run(cli)
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Watch => watch(cli.state),
        Commands::Status => status(cli.state),
    }
}

fn watch(state_arg: Option<PathBuf>) -> Result<()> {
    let state_path = state_path(state_arg)?;
    let cwd = normalized_current_dir()?;
    let (session_id, session_source) = current_session_id(&cwd)?;
    let mut store = load_store(&state_path)?;
    let watched_at_unix = unix_time_now()?;

    let session = WatchedSession {
        session_id: session_id.clone(),
        session_source,
        cwd: cwd.clone(),
        watched_at_unix,
    };
    store.sessions.insert(session_id.clone(), session);
    save_store(&state_path, &store)?;

    println!("watching session {session_id}");
    println!("cwd {cwd}");
    println!("state {}", state_path.display());
    Ok(())
}

fn status(state_arg: Option<PathBuf>) -> Result<()> {
    let state_path = state_path(state_arg)?;
    let store = load_store(&state_path)?;

    if store.sessions.is_empty() {
        println!("no watched sessions");
        return Ok(());
    }

    let panes = load_herdr_panes().context("load Herdr panes for watched-session status")?;
    let rows = status_rows(&store, &panes);
    print_status(&rows);
    Ok(())
}

fn state_path(state_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = state_arg {
        return Ok(path);
    }
    if let Some(path) = env::var_os("COUNTERSPELL_STATE") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("HOME is not set and --state was not provided")?;
    Ok(PathBuf::from(home)
        .join(".counterspell")
        .join("sessions.json"))
}

fn normalized_current_dir() -> Result<String> {
    let cwd = env::current_dir().context("read current directory")?;
    Ok(normalize_path(&cwd))
}

fn current_session_id(cwd: &str) -> Result<(String, String)> {
    for key in [
        "COUNTERSPELL_SESSION_ID",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
    ] {
        if let Some(value) = non_empty_env(key) {
            return Ok((value, key.to_string()));
        }
    }

    if let Some(session) = current_session_from_herdr(cwd)
        .context("could not determine current Codex session from environment or Herdr")?
    {
        return Ok(session);
    }

    bail!(
        "could not determine current Codex session; set COUNTERSPELL_SESSION_ID, CODEX_THREAD_ID, or CODEX_SESSION_ID"
    )
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn load_store(path: &Path) -> Result<WatchStore> {
    if !path.exists() {
        return Ok(WatchStore::default());
    }

    let raw =
        fs::read_to_string(path).with_context(|| format!("read state file {}", path.display()))?;
    let mut store: WatchStore = serde_json::from_str(&raw)
        .with_context(|| format!("parse state file {}", path.display()))?;
    store.version = STORE_VERSION;
    Ok(store)
}

fn save_store(path: &Path, store: &WatchStore) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create state directory {}", parent.display()))?;
    }

    let encoded = serde_json::to_string_pretty(store).context("encode state file")?;
    let temp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp_path, encoded)
        .with_context(|| format!("write temporary state file {}", temp_path.display()))?;
    fs::rename(&temp_path, path)
        .with_context(|| format!("replace state file {}", path.display()))?;
    Ok(())
}

fn load_herdr_panes() -> Result<Vec<HerdrPane>> {
    let herdr_bin =
        env::var_os("COUNTERSPELL_HERDR_BIN").unwrap_or_else(|| OsString::from("herdr"));
    let output = ProcessCommand::new(&herdr_bin)
        .args(["pane", "list"])
        .output()
        .with_context(|| format!("run {}", PathBuf::from(&herdr_bin).display()))?;

    if !output.status.success() {
        bail!(
            "{} exited with {}",
            PathBuf::from(&herdr_bin).display(),
            output.status
        );
    }

    parse_herdr_panes(&output.stdout)
}

fn parse_herdr_panes(bytes: &[u8]) -> Result<Vec<HerdrPane>> {
    let pane_list: HerdrPaneList =
        serde_json::from_slice(bytes).context("parse `herdr pane list` JSON")?;
    Ok(pane_list.result.panes)
}

fn current_session_from_herdr(cwd: &str) -> Result<Option<(String, String)>> {
    let panes = load_herdr_panes()?;
    let matching_panes = matching_panes_for_cwd(cwd, &panes);

    if let Some(current_pane_id) = non_empty_env("HERDR_PANE_ID") {
        let Some(current_pane) = matching_panes
            .iter()
            .find(|pane| pane_id(pane) == current_pane_id)
        else {
            bail!("current Herdr pane {current_pane_id} was not found for cwd {cwd}");
        };

        let Some(session) = pane_session(current_pane) else {
            bail!("current Herdr pane {current_pane_id} does not expose agent_session.value");
        };

        return Ok(Some((
            session.to_string(),
            format!("herdr pane list:{current_pane_id}"),
        )));
    }

    Ok(matching_panes
        .iter()
        .find_map(|pane| pane_session(pane).map(|session| (session, pane_id(pane))))
        .map(|(session, pane_id)| (session.to_string(), format!("herdr pane list:{pane_id}"))))
}

fn pane_session(pane: &HerdrPane) -> Option<&str> {
    pane.agent_session
        .as_ref()
        .and_then(|session| session.value.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn status_rows(store: &WatchStore, panes: &[HerdrPane]) -> Vec<StatusRow> {
    store
        .sessions
        .values()
        .map(|session| {
            let matching_panes = matching_panes_for_cwd(&session.cwd, panes);
            let panes = if matching_panes.is_empty() {
                "not-open".to_string()
            } else {
                join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
            };
            StatusRow {
                session_id: session.session_id.clone(),
                cwd: session.cwd.clone(),
                panes,
                agents: join_or_dash(
                    matching_panes
                        .iter()
                        .filter_map(|pane| pane.agent.as_deref()),
                ),
                states: join_or_dash(
                    matching_panes
                        .iter()
                        .filter_map(|pane| pane.agent_status.as_deref()),
                ),
                labels: join_or_dash(
                    matching_panes
                        .iter()
                        .filter_map(|pane| pane.label.as_deref()),
                ),
                watched_at_unix: session.watched_at_unix.to_string(),
            }
        })
        .collect()
}

fn matching_panes_for_cwd<'a>(cwd: &str, panes: &'a [HerdrPane]) -> Vec<&'a HerdrPane> {
    let normalized_cwd = normalize_path(cwd);
    let mut matches = panes
        .iter()
        .filter(|pane| pane_cwds(pane).any(|pane_cwd| normalize_path(pane_cwd) == normalized_cwd))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| pane_id(left).cmp(pane_id(right)));
    matches
}

fn pane_cwds(pane: &HerdrPane) -> impl Iterator<Item = &str> {
    [pane.cwd.as_deref(), pane.foreground_cwd.as_deref()]
        .into_iter()
        .flatten()
}

fn pane_id(pane: &HerdrPane) -> &str {
    if pane.pane_id.is_empty() {
        "unknown"
    } else {
        pane.pane_id.as_str()
    }
}

fn join_or_dash<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut unique = values
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    unique.sort();
    unique.dedup();

    if unique.is_empty() {
        "-".to_string()
    } else {
        unique.join(",")
    }
}

fn print_status(rows: &[StatusRow]) {
    println!("watched sessions");

    let headers = [
        "SESSION", "CWD", "PANE", "AGENT", "STATE", "LABEL", "WATCHED",
    ];
    let widths = column_widths(rows, &headers);
    print_row(&headers, &widths);
    print_row(
        &widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>(),
        &widths,
    );

    for row in rows {
        print_row(
            &[
                row.session_id.as_str(),
                row.cwd.as_str(),
                row.panes.as_str(),
                row.agents.as_str(),
                row.states.as_str(),
                row.labels.as_str(),
                row.watched_at_unix.as_str(),
            ],
            &widths,
        );
    }
}

fn column_widths(rows: &[StatusRow], headers: &[&str; 7]) -> [usize; 7] {
    let mut widths = headers.map(str::len);
    for row in rows {
        let cells = [
            row.session_id.as_str(),
            row.cwd.as_str(),
            row.panes.as_str(),
            row.agents.as_str(),
            row.states.as_str(),
            row.labels.as_str(),
            row.watched_at_unix.as_str(),
        ];
        for (index, cell) in cells.iter().enumerate() {
            widths[index] = widths[index].max(cell.len());
        }
    }
    widths
}

fn print_row<T: AsRef<str>>(cells: &[T], widths: &[usize; 7]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            print!("  ");
        }
        print!("{:<width$}", cell.as_ref(), width = widths[index]);
    }
    println!();
}

fn normalize_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalized.to_string_lossy().into_owned()
}

fn unix_time_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}
