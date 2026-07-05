use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::time::UNIX_EPOCH;

use crate::config::{
    add_target_to_config, config_path, default_config_text, describe_target_rule,
    ensure_config_file, initial_config, load_config, parse_config_file, selector_count,
    target_rule_from_parts, validate_targets,
};
use crate::dashboard;
use crate::defaults::DEFAULT_TARGET_MODEL;
use crate::feed::append_feed_events;
use crate::herdr::{annotate_herdr_pane, load_herdr_panes, matching_panes_for_session, pane_id};
use crate::indicators::{
    launch_agent_path, launch_agent_scheduled, load_launch_agent, swiftbar_plugin_path,
    watch_arm_launch_agent_path, write_launch_agent, write_swiftbar_plugin,
    write_watch_arm_launch_agent, LAUNCH_AGENT_LABEL, WATCH_ARM_LAUNCH_AGENT_LABEL,
};
use crate::model::FileConfig;
use crate::output::{print_status, print_status_json, print_watch};
use crate::remediation::detect_actionable_drift;
use crate::sessions::discover_recent_sessions;
use crate::status::{status_rows, watch_rows};
use crate::store::{load_store, save_store, state_path};
use crate::util::home_dir;

#[derive(Debug, Parser)]
#[command(name = "counterspell")]
#[command(about = "Watch Claude sessions and map them to Herdr panes.")]
#[command(version)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    /// Annotate watched Herdr panes with Counterspell metadata and exit.
    #[arg(long)]
    pub(crate) annotate_herdr: bool,

    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) state: Option<PathBuf>,

    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,

    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) projects_dir: Option<PathBuf>,

    #[arg(long, global = true, value_name = "HOURS")]
    pub(crate) recent_hours: Option<u64>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a config file.
    Init(InitArgs),
    /// Guided local setup for config and indicators.
    Setup(SetupArgs),
    /// Inspect local install, config, Herdr, and indicator state.
    Doctor(DoctorArgs),
    /// Manage configured targets for extra coverage.
    Target(TargetArgs),
    /// Install menu-bar and Herdr annotation indicators.
    InstallUi(InstallUiArgs),
    /// Serve a visible local dashboard for Counterspell status.
    Ui(UiArgs),
    /// Run one detection/gating pass over recent Claude sessions.
    Watch(WatchArgs),
    /// Show recent Claude sessions and their matching Herdr panes.
    Status(StatusArgs),
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Overwrite an existing config file.
    #[arg(long)]
    pub(crate) force: bool,

    /// Target exactly this transcript session id.
    #[arg(long, value_name = "SESSION_ID")]
    pub(crate) session_id: Option<String>,

    /// Target transcript project directory labels, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) project_pattern: Option<String>,

    /// Target session cwd values, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    pub(crate) cwd_pattern: Option<String>,

    /// Required when a selector is provided.
    #[arg(long, value_name = "MODEL")]
    pub(crate) target_model: Option<String>,
}

#[derive(Debug, Args)]
struct SetupArgs {
    /// Overwrite an existing config file.
    #[arg(long)]
    force: bool,

    /// Target exactly this transcript session id.
    #[arg(long, value_name = "SESSION_ID")]
    session_id: Option<String>,

    /// Target transcript project directory labels, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    project_pattern: Option<String>,

    /// Target session cwd values, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    cwd_pattern: Option<String>,

    /// Model to enforce for the selected target.
    #[arg(long, value_name = "MODEL", default_value = DEFAULT_TARGET_MODEL)]
    target_model: String,

    /// Install SwiftBar and Herdr annotation indicators.
    #[arg(long)]
    install_ui: bool,

    /// Load the Herdr annotation LaunchAgent after writing it.
    #[arg(long)]
    load_ui: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {}

#[derive(Debug, Args)]
struct TargetArgs {
    #[command(subcommand)]
    command: TargetCommand,
}

#[derive(Debug, Subcommand)]
enum TargetCommand {
    /// Add one configured target for extra coverage.
    Add(TargetAddArgs),
    /// List explicit opt-in targets.
    List,
}

#[derive(Debug, Args)]
struct TargetAddArgs {
    /// Target exactly this transcript session id.
    #[arg(long, value_name = "SESSION_ID")]
    session_id: Option<String>,

    /// Target transcript project directory labels, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    project_pattern: Option<String>,

    /// Target session cwd values, supports `*`.
    #[arg(long, value_name = "PATTERN")]
    cwd_pattern: Option<String>,

    /// Model to enforce for the selected target.
    #[arg(long, value_name = "MODEL", default_value = DEFAULT_TARGET_MODEL)]
    target_model: String,
}

#[derive(Debug, Args)]
struct InstallUiArgs {
    /// Do not install the SwiftBar/xbar plugin.
    #[arg(long)]
    no_swiftbar: bool,

    /// Do not install the Herdr annotation LaunchAgent.
    #[arg(long)]
    no_herdr_annotation: bool,

    /// Do not install the armed watch LaunchAgent.
    #[arg(long)]
    no_watch_arm: bool,

    /// Load the Herdr annotation LaunchAgent after writing it.
    #[arg(long)]
    load: bool,

    /// LaunchAgent interval in seconds (Herdr annotation agent).
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    interval_secs: u64,

    /// Armed watch LaunchAgent interval in seconds. Kept tight so a
    /// downgrade is answered while the downgraded turn is still running.
    #[arg(long, value_name = "SECONDS", default_value_t = 10)]
    watch_interval_secs: u64,
}

#[derive(Debug, Args)]
pub(crate) struct UiArgs {
    /// Local dashboard port.
    #[arg(long, value_name = "PORT", default_value_t = 8765)]
    pub(crate) port: u16,

    /// Do not open the browser after the server starts.
    #[arg(long)]
    pub(crate) no_open: bool,

    /// Serve one HTTP request, useful for tests and probes.
    #[arg(long, hide = true)]
    pub(crate) once: bool,
}

#[derive(Debug, Args)]
struct WatchArgs {
    /// Arm eligible compact/switch actions. Without this, watch is a dry-run.
    #[arg(long)]
    arm: bool,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Emit machine-readable JSON for indicator plugins and scripts.
    #[arg(long)]
    json: bool,
}

pub fn run_from_args() -> Result<()> {
    let cli = Cli::parse();
    run(cli)
}

pub fn run(cli: Cli) -> Result<()> {
    if cli.annotate_herdr {
        return annotate_herdr(&cli);
    }

    match &cli.command {
        Some(Commands::Init(args)) => init(&cli, args),
        Some(Commands::Setup(args)) => setup(&cli, args),
        Some(Commands::Doctor(args)) => doctor(&cli, args),
        Some(Commands::Target(args)) => target(&cli, args),
        Some(Commands::InstallUi(args)) => install_ui(args),
        Some(Commands::Ui(args)) => dashboard::serve_dashboard(&cli, args),
        Some(Commands::Watch(args)) => watch(&cli, args),
        Some(Commands::Status(args)) => status(&cli, args),
        None => bail!("missing command; run `counterspell --help`"),
    }
}

#[cfg(test)]
pub(crate) fn test_cli_with_config(config: PathBuf) -> Cli {
    Cli {
        annotate_herdr: false,
        state: None,
        config: Some(config),
        projects_dir: None,
        recent_hours: None,
        command: None,
    }
}

fn init(cli: &Cli, args: &InitArgs) -> Result<()> {
    let home = home_dir()?;
    let path = config_path(cli.config.clone(), &home);
    if path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to replace it",
            path.display()
        );
    }

    let config = initial_config(args)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    fs::write(&path, config).with_context(|| format!("write config {}", path.display()))?;
    println!("wrote {}", path.display());
    if args.target_model.is_none() {
        println!("Fable sessions are watched automatically; add [[targets]] only for overrides");
    }
    Ok(())
}

fn setup(cli: &Cli, args: &SetupArgs) -> Result<()> {
    let home = home_dir()?;
    let path = config_path(cli.config.clone(), &home);
    if path.exists() && args.force {
        fs::write(&path, default_config_text())
            .with_context(|| format!("write config {}", path.display()))?;
        println!("reset {}", path.display());
    } else {
        ensure_config_file(&path)?;
    }

    if selector_count(&args.session_id, &args.project_pattern, &args.cwd_pattern) > 0 {
        let target = target_rule_from_parts(
            args.session_id.clone(),
            args.project_pattern.clone(),
            args.cwd_pattern.clone(),
            args.target_model.clone(),
        )?;
        let added = add_target_to_config(&path, &target)?;
        if added {
            println!("added target {}", describe_target_rule(&target));
        } else {
            println!(
                "target already configured {}",
                describe_target_rule(&target)
            );
        }
    }

    if args.install_ui || args.load_ui {
        install_ui(&InstallUiArgs {
            no_swiftbar: false,
            no_herdr_annotation: false,
            no_watch_arm: false,
            load: args.load_ui,
            interval_secs: 60,
            watch_interval_secs: 10,
        })?;
    }

    println!("run `counterspell doctor` to verify local setup");
    Ok(())
}

fn doctor(cli: &Cli, _args: &DoctorArgs) -> Result<()> {
    let home = home_dir()?;
    let config_path = config_path(cli.config.clone(), &home);
    let state_path = state_path(cli.state.clone())?;
    let config = load_config(cli)?;
    let mut failures = Vec::new();

    println!("counterspell doctor");
    let binary_path = env::current_exe().ok();
    println!(
        "binary: {}",
        binary_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    match binary_path
        .as_ref()
        .map(|path| binary_freshness(path))
        .transpose()?
    {
        Some(BinaryFreshness::Fresh {
            binary_unix,
            repo_head_unix,
        }) => println!(
            "binary freshness: ok (binary mtime {binary_unix} >= repo HEAD {repo_head_unix})"
        ),
        Some(BinaryFreshness::Stale {
            binary_unix,
            repo_head_unix,
        }) => {
            println!(
                "binary freshness: stale (binary mtime {binary_unix} < repo HEAD {repo_head_unix})"
            );
            failures.push("installed binary is older than repo HEAD/latest release".to_string());
        }
        Some(BinaryFreshness::ReleaseFresh {
            current_version,
            latest_version,
        }) => println!(
            "binary freshness: ok (version {current_version} >= latest release {latest_version})"
        ),
        Some(BinaryFreshness::ReleaseStale {
            current_version,
            latest_version,
        }) => {
            println!(
                "binary freshness: stale (version {current_version} < latest release {latest_version})"
            );
            failures.push("installed binary is older than latest release".to_string());
        }
        Some(BinaryFreshness::Unknown(reason)) => {
            println!("binary freshness: unknown ({reason})");
        }
        None => println!("binary freshness: unknown (current executable unavailable)"),
    }
    println!(
        "config: {} ({})",
        config_path.display(),
        if config_path.exists() {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "state: {} ({})",
        state_path.display(),
        if state_path.exists() {
            "present"
        } else {
            "missing"
        }
    );
    println!(
        "projects: {} ({})",
        config.projects_dir.display(),
        if config.projects_dir.exists() {
            "present"
        } else {
            "missing"
        }
    );
    println!("targets: {}", config.targets.len());

    match load_herdr_panes() {
        Ok(panes) => {
            let now = Utc::now();
            let store = load_store(&state_path)?;
            let sessions = discover_recent_sessions(&config, now)?;
            let rows = status_rows(&sessions, &panes, &store, &config, now);
            println!("herdr: reachable ({} pane(s))", panes.len());
            println!(
                "sessions: total={} watched={} ignored={} mapped={} live-pane-only={}",
                rows.len(),
                rows.iter().filter(|row| row.watch == "watched").count(),
                rows.iter().filter(|row| row.watch == "ignored").count(),
                rows.iter()
                    .filter(|row| row.pane != "not-open" && !row.session_id.starts_with("pane:"))
                    .count(),
                rows.iter()
                    .filter(|row| row.project == "herdr-live-pane")
                    .count()
            );
        }
        Err(error) => {
            println!("herdr: unreachable ({error:#})");
        }
    }

    let swiftbar = swiftbar_plugin_path(&home);
    let launch_agent = launch_agent_path(&home);
    let watch_arm_agent = watch_arm_launch_agent_path(&home);
    println!(
        "swiftbar: {} ({})",
        swiftbar.display(),
        if swiftbar.exists() {
            "installed"
        } else {
            "missing"
        }
    );
    println!(
        "herdr annotation agent: {} ({})",
        launch_agent.display(),
        if launch_agent.exists() {
            "installed"
        } else {
            "missing"
        }
    );

    let watch_arm_installed = watch_arm_agent.exists();
    let watch_arm_scheduled = launch_agent_scheduled(WATCH_ARM_LAUNCH_AGENT_LABEL)
        .map(|scheduled| {
            if scheduled {
                "scheduled".to_string()
            } else {
                failures.push("armed watch daemon is not scheduled".to_string());
                "not scheduled".to_string()
            }
        })
        .unwrap_or_else(|error| {
            failures.push("armed watch daemon is not scheduled".to_string());
            format!("schedule unknown ({error:#})")
        });
    println!(
        "watch-arm agent: {} ({}, {})",
        watch_arm_agent.display(),
        if watch_arm_installed {
            "installed"
        } else {
            "missing"
        },
        watch_arm_scheduled
    );

    if !watch_arm_installed
        && !failures
            .iter()
            .any(|failure| failure == "armed watch daemon is not scheduled")
    {
        failures.push("armed watch daemon is not scheduled".to_string());
    }

    if !failures.is_empty() {
        bail!("doctor failed: {}", failures.join("; "));
    }

    Ok(())
}

fn target(cli: &Cli, args: &TargetArgs) -> Result<()> {
    let home = home_dir()?;
    let path = config_path(cli.config.clone(), &home);
    match &args.command {
        TargetCommand::Add(args) => {
            ensure_config_file(&path)?;
            let target = target_rule_from_parts(
                args.session_id.clone(),
                args.project_pattern.clone(),
                args.cwd_pattern.clone(),
                args.target_model.clone(),
            )?;
            let added = add_target_to_config(&path, &target)?;
            if added {
                println!("added target {}", describe_target_rule(&target));
            } else {
                println!(
                    "target already configured {}",
                    describe_target_rule(&target)
                );
            }
        }
        TargetCommand::List => {
            let raw = if path.exists() {
                parse_config_file(&path)?
            } else {
                FileConfig::default()
            };
            let targets = validate_targets(raw.targets)?;
            if targets.is_empty() {
                println!("no targets configured");
            } else {
                for target in targets {
                    println!("{}", describe_target_rule(&target));
                }
            }
        }
    }

    Ok(())
}

fn install_ui(args: &InstallUiArgs) -> Result<()> {
    let home = home_dir()?;
    let bin = env::current_exe().context("resolve current counterspell binary")?;

    if !args.no_swiftbar {
        let plugin_path = swiftbar_plugin_path(&home);
        write_swiftbar_plugin(&plugin_path, &bin)?;
        println!("installed SwiftBar plugin {}", plugin_path.display());
    }

    if !args.no_herdr_annotation {
        let launch_agent_path = launch_agent_path(&home);
        write_launch_agent(&launch_agent_path, &bin, args.interval_secs)?;
        println!(
            "installed Herdr annotation LaunchAgent {}",
            launch_agent_path.display()
        );
        if args.load {
            load_launch_agent(&launch_agent_path, LAUNCH_AGENT_LABEL)?;
            println!("loaded {LAUNCH_AGENT_LABEL}");
        }
    }

    if !args.no_watch_arm {
        let watch_arm_path = watch_arm_launch_agent_path(&home);
        write_watch_arm_launch_agent(&watch_arm_path, &bin, args.watch_interval_secs)?;
        println!(
            "installed watch-arm LaunchAgent {}",
            watch_arm_path.display()
        );
        if args.load {
            load_launch_agent(&watch_arm_path, WATCH_ARM_LAUNCH_AGENT_LABEL)?;
            println!("loaded {WATCH_ARM_LAUNCH_AGENT_LABEL}");
        }
    }

    Ok(())
}

fn watch(cli: &Cli, args: &WatchArgs) -> Result<()> {
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
    let (rows, store_changed, feed_events) = watch_rows(
        &sessions,
        &panes,
        &mut store,
        &config,
        now,
        args.arm,
        Some(&state_path),
    )?;
    if store_changed {
        save_store(&state_path, &store)?;
    }
    append_feed_events(&feed_events, now)?;
    print_watch(&rows);
    Ok(())
}

fn status(cli: &Cli, args: &StatusArgs) -> Result<()> {
    let config = load_config(cli)?;
    let now = Utc::now();
    let store = load_store(&state_path(cli.state.clone())?)?;
    let sessions = discover_recent_sessions(&config, now)?;
    let panes = load_herdr_panes().context("load Herdr panes for session status")?;
    let rows = status_rows(&sessions, &panes, &store, &config, now);
    if args.json {
        print_status_json(&rows, &store, now)?;
    } else if rows.is_empty() {
        println!("no recent sessions");
    } else {
        print_status(&rows);
    }
    Ok(())
}

fn annotate_herdr(cli: &Cli) -> Result<()> {
    let config = load_config(cli)?;
    let now = Utc::now();
    let store = load_store(&state_path(cli.state.clone())?)?;
    let sessions = discover_recent_sessions(&config, now)?;
    let panes = load_herdr_panes().context("load Herdr panes for annotation")?;
    let mut annotations = BTreeMap::new();

    for session in &sessions {
        let Some(target) = crate::remediation::target_for_session(session, &config) else {
            continue;
        };
        let matching_panes =
            matching_panes_for_session(&session.session_id, session.cwd.as_deref(), &panes);
        let title = format!("Counterspell: {}", target.target_model);
        let state = store.sessions.get(&session.session_id);
        let status = detect_actionable_drift(session, &target.target_model, state)
            .map(|drift| format!("drift {}->{}", drift.from, drift.to))
            .unwrap_or_else(|| "watched".to_string());

        for pane in matching_panes {
            if pane.agent.as_deref() != Some("claude") {
                continue;
            }
            annotations
                .entry(pane_id(pane).to_string())
                .or_insert_with(|| (title.clone(), status.clone()));
        }
    }

    let annotated = annotations.len();
    for (pane_id, (title, status)) in annotations {
        annotate_herdr_pane(&pane_id, &title, &status)?;
    }

    println!("annotated {annotated} Herdr pane(s)");
    Ok(())
}

#[derive(Debug)]
enum BinaryFreshness {
    Fresh {
        binary_unix: u64,
        repo_head_unix: u64,
    },
    Stale {
        binary_unix: u64,
        repo_head_unix: u64,
    },
    ReleaseFresh {
        current_version: String,
        latest_version: String,
    },
    ReleaseStale {
        current_version: String,
        latest_version: String,
    },
    Unknown(String),
}

fn binary_freshness(path: &std::path::Path) -> Result<BinaryFreshness> {
    let binary_unix = fs::metadata(path)
        .with_context(|| format!("read binary metadata {}", path.display()))?
        .modified()
        .with_context(|| format!("read binary mtime {}", path.display()))?
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("convert binary mtime {}", path.display()))?
        .as_secs();

    if let Some(repo_head_unix) = repo_head_unix() {
        if binary_unix < repo_head_unix {
            return Ok(BinaryFreshness::Stale {
                binary_unix,
                repo_head_unix,
            });
        }
        return Ok(BinaryFreshness::Fresh {
            binary_unix,
            repo_head_unix,
        });
    }

    let Some(latest_version) = latest_release_version() else {
        return Ok(BinaryFreshness::Unknown(
            "repo HEAD and latest release unavailable".to_string(),
        ));
    };
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    if version_less_than(&current_version, &latest_version).unwrap_or(false) {
        Ok(BinaryFreshness::ReleaseStale {
            current_version,
            latest_version,
        })
    } else {
        Ok(BinaryFreshness::ReleaseFresh {
            current_version,
            latest_version,
        })
    }
}

fn repo_head_unix() -> Option<u64> {
    if let Ok(value) = env::var("COUNTERSPELL_REPO_HEAD_UNIX") {
        let value = value.trim();
        if value.eq_ignore_ascii_case("none") {
            return None;
        }
        if let Ok(parsed) = value.parse::<u64>() {
            return Some(parsed);
        }
        return None;
    }

    let output = ProcessCommand::new("git")
        .args([
            "-C",
            env!("CARGO_MANIFEST_DIR"),
            "log",
            "-1",
            "--format=%ct",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn latest_release_version() -> Option<String> {
    if let Ok(value) = env::var("COUNTERSPELL_LATEST_RELEASE_VERSION") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
        return None;
    }

    let output = ProcessCommand::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "2",
            "https://api.github.com/repos/misty-step/counterspell/releases/latest",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn version_less_than(current: &str, latest: &str) -> Option<bool> {
    let current = parse_version(current)?;
    let latest = parse_version(latest)?;
    Some(current < latest)
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let start = value.find(|character: char| character.is_ascii_digit())?;
    let core = value[start..].split(['-', '+']).next().unwrap_or_default();
    let mut parts = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.is_empty() {
        return None;
    }
    while parts.len() < 3 {
        parts.push(0);
    }
    Some(parts)
}
