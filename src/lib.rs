use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::SystemTime;

const STORE_VERSION: u8 = 2;
const DEFAULT_RECENT_HOURS: u64 = 72;
const DEFAULT_TRANSCRIPT_QUIET_SECONDS: u64 = 30;
const DEFAULT_DEBOUNCE_SECONDS: u64 = 300;

#[derive(Debug, Parser)]
#[command(name = "counterspell")]
#[command(about = "Watch Claude sessions and map them to Herdr panes.")]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    state: Option<PathBuf>,

    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, value_name = "PATH")]
    projects_dir: Option<PathBuf>,

    #[arg(long, global = true, value_name = "HOURS")]
    recent_hours: Option<u64>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run one detection/gating pass over recent Claude sessions.
    Watch,
    /// Show recent Claude sessions and their matching Herdr panes.
    Status,
}

#[derive(Debug, Clone)]
struct Config {
    projects_dir: PathBuf,
    recent_hours: u64,
    targets: Vec<TargetRule>,
    transcript_quiet_seconds: u64,
    debounce_seconds: u64,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    projects_dir: Option<PathBuf>,
    recent_hours: Option<u64>,
    #[serde(default)]
    targets: Vec<TargetRule>,
    transcript_quiet_seconds: Option<u64>,
    debounce_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct TargetRule {
    session_id: Option<String>,
    project_pattern: Option<String>,
    cwd_pattern: Option<String>,
    target_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetMatch {
    target_model: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WatchStore {
    version: u8,
    sessions: BTreeMap<String, SessionState>,
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
struct SessionState {
    session_id: String,
    cwd: Option<String>,
    last_action_unix: Option<u64>,
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
    agent: Option<String>,
    agent_status: Option<String>,
}

#[derive(Debug, Clone)]
struct TranscriptSession {
    session_id: String,
    project: String,
    cwd: Option<String>,
    last_event_at: DateTime<Utc>,
    latest_model: Option<String>,
    model_history: Vec<String>,
}

#[derive(Debug)]
struct StatusRow {
    session_id: String,
    project: String,
    cwd: String,
    pane: String,
    agent: String,
    state: String,
    watch: String,
    target: String,
    model: String,
    drift: String,
    updated: String,
}

#[derive(Debug)]
struct WatchRow {
    session_id: String,
    pane: String,
    model: String,
    target: String,
    gate: String,
    actions: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelDrift {
    from: String,
    to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlannedAction {
    Compact,
    SwitchModel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateBlocker {
    NoPane,
    TranscriptActive,
    PaneBusy(String),
    Debounce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GateDecision {
    blockers: Vec<GateBlocker>,
}

impl GateDecision {
    fn is_allowed(&self) -> bool {
        self.blockers.is_empty()
    }
}

pub fn run_from_args() -> Result<()> {
    let cli = Cli::parse();
    run(cli)
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Watch => watch(&cli),
        Commands::Status => status(&cli),
    }
}

fn watch(cli: &Cli) -> Result<()> {
    let config = load_config(cli)?;
    let now = Utc::now();
    let state_path = state_path(cli.state.clone())?;
    let mut store = load_store(&state_path)?;
    let sessions = discover_recent_sessions(&config, now)?;

    if sessions.is_empty() {
        println!("no recent sessions");
        return Ok(());
    }

    let panes = load_herdr_panes().context("load Herdr panes for watch")?;
    let (rows, store_changed) = watch_rows(&sessions, &panes, &mut store, &config, now);
    if store_changed {
        save_store(&state_path, &store)?;
    }
    print_watch(&rows);
    Ok(())
}

fn status(cli: &Cli) -> Result<()> {
    let config = load_config(cli)?;
    let now = Utc::now();
    let store = load_store(&state_path(cli.state.clone())?)?;
    let sessions = discover_recent_sessions(&config, now)?;

    if sessions.is_empty() {
        println!("no recent sessions");
        return Ok(());
    }

    let panes = load_herdr_panes().context("load Herdr panes for session status")?;
    let rows = status_rows(&sessions, &panes, &store, &config, now);
    print_status(&rows);
    Ok(())
}

fn load_config(cli: &Cli) -> Result<Config> {
    let home = home_dir()?;
    let mut raw = FileConfig::default();
    let config_path = config_path(cli.config.clone(), &home);

    if config_path.exists() {
        raw = parse_config_file(&config_path)?;
    }

    let projects_dir = cli
        .projects_dir
        .clone()
        .or_else(|| env::var_os("COUNTERSPELL_PROJECTS_DIR").map(PathBuf::from))
        .or(raw.projects_dir)
        .unwrap_or_else(|| home.join(".claude").join("projects"));
    let recent_hours = cli
        .recent_hours
        .or_else(|| parse_env_u64("COUNTERSPELL_RECENT_HOURS"))
        .or(raw.recent_hours)
        .unwrap_or(DEFAULT_RECENT_HOURS);
    let transcript_quiet_seconds = parse_env_u64("COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS")
        .or(raw.transcript_quiet_seconds)
        .unwrap_or(DEFAULT_TRANSCRIPT_QUIET_SECONDS);
    let debounce_seconds = parse_env_u64("COUNTERSPELL_DEBOUNCE_SECONDS")
        .or(raw.debounce_seconds)
        .unwrap_or(DEFAULT_DEBOUNCE_SECONDS);

    Ok(Config {
        projects_dir,
        recent_hours,
        targets: validate_targets(raw.targets)?,
        transcript_quiet_seconds,
        debounce_seconds,
    })
}

fn validate_targets(targets: Vec<TargetRule>) -> Result<Vec<TargetRule>> {
    for target in &targets {
        if target.target_model.trim().is_empty() {
            bail!("target entry is missing target_model");
        }

        let selector_count = [
            target.session_id.is_some(),
            target.project_pattern.is_some(),
            target.cwd_pattern.is_some(),
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();

        if selector_count != 1 {
            bail!(
                "target entry must set exactly one of session_id, project_pattern, or cwd_pattern"
            );
        }
    }

    Ok(targets)
}

fn parse_config_file(path: &Path) -> Result<FileConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse config {}", path.display()))
}

fn config_path(config_arg: Option<PathBuf>, home: &Path) -> PathBuf {
    if let Some(path) = config_arg {
        return path;
    }
    if let Some(path) = env::var_os("COUNTERSPELL_CONFIG") {
        return PathBuf::from(path);
    }
    home.join(".counterspell").join("config.toml")
}

fn state_path(state_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = state_arg {
        return Ok(path);
    }
    if let Some(path) = env::var_os("COUNTERSPELL_STATE") {
        return Ok(PathBuf::from(path));
    }
    Ok(home_dir()?.join(".counterspell").join("sessions.json"))
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn parse_env_u64(key: &str) -> Option<u64> {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create state dir {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(store).context("serialize state file")?;
    fs::write(path, raw).with_context(|| format!("write state file {}", path.display()))
}

fn discover_recent_sessions(config: &Config, now: DateTime<Utc>) -> Result<Vec<TranscriptSession>> {
    let cutoff = now - Duration::hours(config.recent_hours as i64);
    let mut sessions = Vec::new();

    for project_entry in fs::read_dir(&config.projects_dir)
        .with_context(|| format!("read projects dir {}", config.projects_dir.display()))?
    {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project = project_label(&project_path);

        for session_entry in fs::read_dir(&project_path)
            .with_context(|| format!("read project dir {}", project_path.display()))?
        {
            let session_entry = session_entry?;
            let path = session_entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let metadata = session_entry
                .metadata()
                .with_context(|| format!("read metadata {}", path.display()))?;
            let modified_at = system_time_to_utc(metadata.modified()?);
            if modified_at < cutoff {
                continue;
            }

            match parse_transcript_file(&path, project.clone(), modified_at) {
                Ok(session) => sessions.push(session),
                Err(error) => eprintln!("warning: skipped {}: {error:#}", path.display()),
            }
        }
    }

    sessions.sort_by(|left, right| {
        right
            .last_event_at
            .cmp(&left.last_event_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(sessions)
}

fn parse_transcript_file(
    path: &Path,
    project: String,
    file_modified_at: DateTime<Utc>,
) -> Result<TranscriptSession> {
    let file = File::open(path).with_context(|| format!("open transcript {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut cwd = None;
    let mut last_event_at = None;
    let mut latest_model = None;
    let mut model_history = Vec::new();

    for line in reader.lines() {
        let line = line.with_context(|| format!("read transcript {}", path.display()))?;
        let value: Value =
            serde_json::from_str(&line).with_context(|| format!("parse {}", path.display()))?;

        if let Some(value_session_id) = value.get("sessionId").and_then(Value::as_str) {
            session_id = value_session_id.to_string();
        }
        if let Some(value_cwd) = value.get("cwd").and_then(Value::as_str) {
            cwd = Some(value_cwd.to_string());
        }
        if let Some(timestamp) = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
        {
            last_event_at = Some(timestamp);
        }
        if let Some(model) = transcript_model(&value) {
            if model_history.last() != Some(&model) {
                model_history.push(model.clone());
            }
            latest_model = Some(model);
        }
    }

    Ok(TranscriptSession {
        session_id,
        project,
        cwd,
        last_event_at: last_event_at.unwrap_or(file_modified_at),
        latest_model,
        model_history,
    })
}

fn transcript_model(value: &Value) -> Option<String> {
    value
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/message/model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn system_time_to_utc(value: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(value)
}

fn project_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-project")
        .to_string()
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

fn status_rows(
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    store: &WatchStore,
    config: &Config,
    now: DateTime<Utc>,
) -> Vec<StatusRow> {
    let mut used_panes = BTreeSet::new();
    let mut rows = sessions
        .iter()
        .map(|session| {
            let matching_panes = session
                .cwd
                .as_deref()
                .map(|cwd| matching_panes_for_cwd(cwd, panes))
                .unwrap_or_default();
            for pane in &matching_panes {
                used_panes.insert(pane_id(pane).to_string());
            }
            let pane = if matching_panes.is_empty() {
                "not-open".to_string()
            } else {
                join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
            };
            let target = target_for_session(session, config);
            let drift = target
                .as_ref()
                .map(|target| {
                    detect_drift(session, &target.target_model)
                        .map(|drift| format!("{}->{}", drift.from, drift.to))
                        .unwrap_or_else(|| "ok".to_string())
                })
                .unwrap_or_else(|| "ignored".to_string());
            let state = store.sessions.get(&session.session_id);
            let gate = gate_decision(session, matching_panes.first().copied(), state, config, now);

            StatusRow {
                session_id: short_session(&session.session_id),
                project: session.project.clone(),
                cwd: session.cwd.clone().unwrap_or_else(|| "-".to_string()),
                pane,
                agent: join_or_dash(
                    matching_panes
                        .iter()
                        .filter_map(|pane| pane.agent.as_deref()),
                ),
                state: status_state(&matching_panes, &gate),
                watch: if target.is_some() {
                    "watched".to_string()
                } else {
                    "ignored".to_string()
                },
                target: target
                    .as_ref()
                    .map(format_target_match)
                    .unwrap_or_else(|| "no-target".to_string()),
                model: session
                    .latest_model
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                drift,
                updated: human_age(session.last_event_at, now),
            }
        })
        .collect::<Vec<_>>();

    for pane in panes {
        if pane.agent.as_deref() != Some("claude") || used_panes.contains(pane_id(pane)) {
            continue;
        }

        rows.push(StatusRow {
            session_id: format!("pane:{}", pane_id(pane)),
            project: "herdr-live-pane".to_string(),
            cwd: pane
                .cwd
                .clone()
                .or_else(|| pane.foreground_cwd.clone())
                .unwrap_or_else(|| "-".to_string()),
            pane: pane_id(pane).to_string(),
            agent: pane.agent.clone().unwrap_or_else(|| "-".to_string()),
            state: pane
                .agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            watch: "ignored".to_string(),
            target: "no-transcript-target".to_string(),
            model: "-".to_string(),
            drift: "ignored".to_string(),
            updated: "live".to_string(),
        });
    }

    rows
}

fn watch_rows(
    sessions: &[TranscriptSession],
    panes: &[HerdrPane],
    store: &mut WatchStore,
    config: &Config,
    now: DateTime<Utc>,
) -> (Vec<WatchRow>, bool) {
    let mut store_changed = false;
    let rows = sessions
        .iter()
        .map(|session| {
            let matching_panes = session
                .cwd
                .as_deref()
                .map(|cwd| matching_panes_for_cwd(cwd, panes))
                .unwrap_or_default();
            let state = store.sessions.get(&session.session_id);
            let plan =
                remediation_plan(session, matching_panes.first().copied(), state, config, now);
            if !plan.actions.is_empty() {
                store.sessions.insert(
                    session.session_id.clone(),
                    SessionState {
                        session_id: session.session_id.clone(),
                        cwd: session.cwd.clone(),
                        last_action_unix: Some(now.timestamp().try_into().unwrap_or(0)),
                    },
                );
                store_changed = true;
            }
            let target = target_for_session(session, config);
            WatchRow {
                session_id: short_session(&session.session_id),
                pane: if matching_panes.is_empty() {
                    "not-open".to_string()
                } else {
                    join_or_dash(matching_panes.iter().map(|pane| pane_id(pane)))
                },
                model: session
                    .latest_model
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
                target: target
                    .as_ref()
                    .map(format_target_match)
                    .unwrap_or_else(|| "ignored:no-target".to_string()),
                gate: describe_gate(&plan.gate),
                actions: describe_actions(&plan.actions),
            }
        })
        .collect();

    (rows, store_changed)
}

#[derive(Debug)]
struct RemediationPlan {
    gate: GateDecision,
    actions: Vec<PlannedAction>,
}

fn remediation_plan(
    session: &TranscriptSession,
    pane: Option<&HerdrPane>,
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> RemediationPlan {
    let target = target_for_session(session, config);
    let gate = gate_decision(session, pane, state, config, now);
    let actions = if let Some(target) = target {
        if detect_drift(session, &target.target_model).is_some() && gate.is_allowed() {
            vec![
                PlannedAction::Compact,
                PlannedAction::SwitchModel(target.target_model),
            ]
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    RemediationPlan { gate, actions }
}

fn target_for_session(session: &TranscriptSession, config: &Config) -> Option<TargetMatch> {
    for target in &config.targets {
        if target
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id == session.session_id)
        {
            return Some(TargetMatch {
                target_model: target.target_model.clone(),
                reason: "session_id".to_string(),
            });
        }

        if target
            .project_pattern
            .as_deref()
            .is_some_and(|pattern| wildcard_match(pattern, &session.project))
        {
            return Some(TargetMatch {
                target_model: target.target_model.clone(),
                reason: format!("project:{}", target.project_pattern.as_deref().unwrap()),
            });
        }

        if let Some(cwd) = session.cwd.as_deref() {
            if target
                .cwd_pattern
                .as_deref()
                .is_some_and(|pattern| wildcard_match(pattern, cwd))
            {
                return Some(TargetMatch {
                    target_model: target.target_model.clone(),
                    reason: format!("cwd:{}", target.cwd_pattern.as_deref().unwrap()),
                });
            }
        }
    }

    None
}

fn format_target_match(target: &TargetMatch) -> String {
    format!("{} ({})", target.target_model, target.reason)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut remaining = value;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        let Some(stripped) = remaining.strip_prefix(first) else {
            return false;
        };
        remaining = stripped;
    }

    for part in parts
        .iter()
        .skip(1)
        .take(parts.len().saturating_sub(2))
        .filter(|part| !part.is_empty())
    {
        let Some(index) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[index + part.len()..];
    }

    if let Some(last) = parts.last().filter(|part| !part.is_empty()) {
        remaining.ends_with(last)
    } else {
        true
    }
}

fn detect_drift(session: &TranscriptSession, desired_model: &str) -> Option<ModelDrift> {
    let latest = session.latest_model.as_ref()?;
    if latest == desired_model {
        return None;
    }

    let (from, to) = if session
        .model_history
        .iter()
        .any(|model| model == desired_model)
    {
        (desired_model.to_string(), latest.clone())
    } else {
        (latest.clone(), desired_model.to_string())
    };

    Some(ModelDrift { from, to })
}

fn gate_decision(
    session: &TranscriptSession,
    pane: Option<&HerdrPane>,
    state: Option<&SessionState>,
    config: &Config,
    now: DateTime<Utc>,
) -> GateDecision {
    let mut blockers = Vec::new();

    if now - session.last_event_at < Duration::seconds(config.transcript_quiet_seconds as i64) {
        blockers.push(GateBlocker::TranscriptActive);
    }

    match pane {
        Some(pane) if pane.agent_status.as_deref() == Some("idle") => {}
        Some(pane) => blockers.push(GateBlocker::PaneBusy(
            pane.agent_status
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )),
        None => blockers.push(GateBlocker::NoPane),
    }

    if let Some(last_action_unix) = state.and_then(|state| state.last_action_unix) {
        if let Some(last_action_at) = unix_to_utc(last_action_unix) {
            if now - last_action_at < Duration::seconds(config.debounce_seconds as i64) {
                blockers.push(GateBlocker::Debounce);
            }
        }
    }

    GateDecision { blockers }
}

fn unix_to_utc(value: u64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(value as i64, 0)
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

fn normalize_path(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalized.to_string_lossy().into_owned()
}

fn status_state(panes: &[&HerdrPane], gate: &GateDecision) -> String {
    if panes.is_empty() {
        return "not-open".to_string();
    }
    if gate.is_allowed() {
        return "idle".to_string();
    }
    describe_gate(gate)
}

fn describe_gate(gate: &GateDecision) -> String {
    if gate.is_allowed() {
        return "allowed".to_string();
    }

    gate.blockers
        .iter()
        .map(|blocker| match blocker {
            GateBlocker::NoPane => "no-pane".to_string(),
            GateBlocker::TranscriptActive => "transcript-active".to_string(),
            GateBlocker::PaneBusy(state) => format!("pane-{state}"),
            GateBlocker::Debounce => "debounce".to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn describe_actions(actions: &[PlannedAction]) -> String {
    if actions.is_empty() {
        return "-".to_string();
    }

    actions
        .iter()
        .map(|action| match action {
            PlannedAction::Compact => "compact".to_string(),
            PlannedAction::SwitchModel(model) => format!("switch:{model}"),
        })
        .collect::<Vec<_>>()
        .join(" then ")
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

fn short_session(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn human_age(value: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = now - value;
    if age.num_seconds() < 60 {
        return format!("{}s ago", age.num_seconds().max(0));
    }
    if age.num_minutes() < 60 {
        return format!("{}m ago", age.num_minutes());
    }
    if age.num_hours() < 48 {
        return format!("{}h ago", age.num_hours());
    }
    format!("{}d ago", age.num_days())
}

fn print_status(rows: &[StatusRow]) {
    println!("recent sessions");

    let headers = [
        "SESSION", "PROJECT", "CWD", "PANE", "AGENT", "STATE", "WATCH", "TARGET", "MODEL", "DRIFT",
        "UPDATED",
    ];
    let widths = status_widths(rows, &headers);
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
                row.project.as_str(),
                row.cwd.as_str(),
                row.pane.as_str(),
                row.agent.as_str(),
                row.state.as_str(),
                row.watch.as_str(),
                row.target.as_str(),
                row.model.as_str(),
                row.drift.as_str(),
                row.updated.as_str(),
            ],
            &widths,
        );
    }
}

fn print_watch(rows: &[WatchRow]) {
    println!("watch pass");

    let headers = ["SESSION", "PANE", "MODEL", "TARGET", "GATE", "ACTIONS"];
    let widths = watch_widths(rows, &headers);
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
                row.pane.as_str(),
                row.model.as_str(),
                row.target.as_str(),
                row.gate.as_str(),
                row.actions.as_str(),
            ],
            &widths,
        );
    }
}

fn status_widths(rows: &[StatusRow], headers: &[&str; 11]) -> [usize; 11] {
    let mut widths = headers.map(str::len);
    for row in rows {
        let cells = [
            row.session_id.as_str(),
            row.project.as_str(),
            row.cwd.as_str(),
            row.pane.as_str(),
            row.agent.as_str(),
            row.state.as_str(),
            row.watch.as_str(),
            row.target.as_str(),
            row.model.as_str(),
            row.drift.as_str(),
            row.updated.as_str(),
        ];
        widen(&mut widths, &cells);
    }
    widths
}

fn watch_widths(rows: &[WatchRow], headers: &[&str; 6]) -> [usize; 6] {
    let mut widths = headers.map(str::len);
    for row in rows {
        let cells = [
            row.session_id.as_str(),
            row.pane.as_str(),
            row.model.as_str(),
            row.target.as_str(),
            row.gate.as_str(),
            row.actions.as_str(),
        ];
        widen(&mut widths, &cells);
    }
    widths
}

fn widen(widths: &mut [usize], cells: &[&str]) {
    for (index, cell) in cells.iter().enumerate() {
        widths[index] = widths[index].max(cell.len());
    }
}

fn print_row<T: AsRef<str>>(cells: &[T], widths: &[usize]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            print!("  ");
        }
        print!("{:<width$}", cell.as_ref(), width = widths[index]);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_config() -> Config {
        Config {
            projects_dir: PathBuf::from("/tmp/projects"),
            recent_hours: 72,
            targets: vec![TargetRule {
                session_id: Some("session-1".to_string()),
                project_pattern: None,
                cwd_pattern: None,
                target_model: "claude-fable-5".to_string(),
            }],
            transcript_quiet_seconds: 30,
            debounce_seconds: 300,
        }
    }

    fn test_session(now: DateTime<Utc>) -> TranscriptSession {
        TranscriptSession {
            session_id: "session-1".to_string(),
            project: "project".to_string(),
            cwd: Some("/repo".to_string()),
            last_event_at: now - Duration::seconds(60),
            latest_model: Some("claude-opus-4-1".to_string()),
            model_history: vec!["claude-fable-5".to_string(), "claude-opus-4-1".to_string()],
        }
    }

    fn idle_pane() -> HerdrPane {
        HerdrPane {
            pane_id: "pane-1".to_string(),
            cwd: Some("/repo".to_string()),
            foreground_cwd: Some("/repo".to_string()),
            agent: Some("claude".to_string()),
            agent_status: Some("idle".to_string()),
        }
    }

    #[test]
    fn drift_detection_reads_fable_to_opus_from_transcript_jsonl() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session-1.jsonl");
        let mut file = File::create(&path).expect("create transcript");
        writeln!(
            file,
            r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:00:00Z","cwd":"/repo","message":{{"model":"claude-fable-5"}}}}"#
        )
        .expect("write fable");
        writeln!(
            file,
            r#"{{"type":"assistant","sessionId":"session-1","timestamp":"2026-07-02T12:01:00Z","cwd":"/repo","message":{{"model":"claude-opus-4-1"}}}}"#
        )
        .expect("write opus");

        let session = parse_transcript_file(&path, "project".to_string(), Utc::now()).unwrap();
        let drift = detect_drift(&session, "claude-fable-5").expect("drift");

        assert_eq!(
            drift,
            ModelDrift {
                from: "claude-fable-5".to_string(),
                to: "claude-opus-4-1".to_string()
            }
        );
    }

    #[test]
    fn unattended_gate_requires_quiet_transcript_idle_pane_and_debounce() {
        let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut config = test_config();
        config.transcript_quiet_seconds = 30;
        config.debounce_seconds = 300;
        let pane = idle_pane();
        let quiet_session = test_session(now);

        assert!(gate_decision(&quiet_session, Some(&pane), None, &config, now).is_allowed());

        let mut active_session = quiet_session.clone();
        active_session.last_event_at = now - Duration::seconds(5);
        assert_eq!(
            gate_decision(&active_session, Some(&pane), None, &config, now).blockers,
            vec![GateBlocker::TranscriptActive]
        );

        let mut busy_pane = pane.clone();
        busy_pane.agent_status = Some("working".to_string());
        assert_eq!(
            gate_decision(&quiet_session, Some(&busy_pane), None, &config, now).blockers,
            vec![GateBlocker::PaneBusy("working".to_string())]
        );

        let state = SessionState {
            session_id: "session-1".to_string(),
            cwd: Some("/repo".to_string()),
            last_action_unix: Some((now - Duration::seconds(60)).timestamp() as u64),
        };
        assert_eq!(
            gate_decision(&quiet_session, Some(&pane), Some(&state), &config, now).blockers,
            vec![GateBlocker::Debounce]
        );
    }

    #[test]
    fn drift_plan_sequences_compact_then_switch() {
        let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let config = test_config();
        let session = test_session(now);
        let pane = idle_pane();

        let plan = remediation_plan(&session, Some(&pane), None, &config, now);

        assert_eq!(
            plan.actions,
            vec![
                PlannedAction::Compact,
                PlannedAction::SwitchModel("claude-fable-5".to_string())
            ]
        );
    }

    #[test]
    fn unconfigured_drift_is_observed_but_not_armed() {
        let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut config = test_config();
        config.targets.clear();
        let session = test_session(now);
        let pane = idle_pane();

        let plan = remediation_plan(&session, Some(&pane), None, &config, now);

        assert!(detect_drift(&session, "claude-fable-5").is_some());
        assert!(plan.actions.is_empty());
        assert_eq!(target_for_session(&session, &config), None);
    }

    #[test]
    fn status_marks_unconfigured_sessions_ignored_not_ok() {
        let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut config = test_config();
        config.targets.clear();
        let session = test_session(now);
        let pane = idle_pane();
        let store = WatchStore::default();

        let rows = status_rows(&[session], &[pane], &store, &config, now);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].watch, "ignored");
        assert_eq!(rows[0].target, "no-target");
        assert_eq!(rows[0].model, "claude-opus-4-1");
        assert_eq!(rows[0].drift, "ignored");
    }

    #[test]
    fn target_reason_renders_without_debug_quotes() {
        let now = DateTime::parse_from_rfc3339("2026-07-02T12:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let config = Config {
            targets: vec![TargetRule {
                session_id: None,
                project_pattern: Some("project*".to_string()),
                cwd_pattern: None,
                target_model: "claude-fable-5".to_string(),
            }],
            ..test_config()
        };
        let session = test_session(now);

        let target = target_for_session(&session, &config).expect("target");

        assert_eq!(
            format_target_match(&target),
            "claude-fable-5 (project:project*)"
        );
    }

    #[test]
    fn config_parsing_reads_counterspell_toml() {
        let raw = r#"
projects_dir = "/tmp/claude-projects"
recent_hours = 12
transcript_quiet_seconds = 45
debounce_seconds = 600

[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"
"#;
        let parsed: FileConfig = toml::from_str(raw).expect("config");

        assert_eq!(
            parsed.projects_dir,
            Some(PathBuf::from("/tmp/claude-projects"))
        );
        assert_eq!(parsed.recent_hours, Some(12));
        assert_eq!(
            parsed.targets,
            vec![TargetRule {
                session_id: None,
                project_pattern: Some("-Users-phaedrus-Development-adminifi*".to_string()),
                cwd_pattern: None,
                target_model: "claude-fable-5".to_string()
            }]
        );
        assert_eq!(parsed.transcript_quiet_seconds, Some(45));
        assert_eq!(parsed.debounce_seconds, Some(600));
    }

    #[test]
    fn config_rejects_global_target_without_selector() {
        let raw = r#"
[[targets]]
target_model = "claude-fable-5"
"#;
        let parsed: FileConfig = toml::from_str(raw).expect("config");
        assert!(validate_targets(parsed.targets).is_err());
    }
}
