use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{Cli, InitArgs};
use crate::defaults::{
    DEFAULT_DEBOUNCE_SECONDS, DEFAULT_RECENT_HOURS, DEFAULT_TRANSCRIPT_QUIET_SECONDS,
};
use crate::model::{Config, FileConfig, TargetRule};
use crate::util::{home_dir, parse_env_u64};

pub(crate) fn initial_config(args: &InitArgs) -> Result<String> {
    let selector_count = selector_count(&args.session_id, &args.project_pattern, &args.cwd_pattern);

    if args.target_model.is_some() && selector_count != 1 {
        bail!("set exactly one of --session-id, --project-pattern, or --cwd-pattern with --target-model");
    }
    if args.target_model.is_none() && selector_count != 0 {
        bail!("--target-model is required when a target selector is provided");
    }

    let mut config = default_config_text();

    if let Some(target_model) = &args.target_model {
        let target = target_rule_from_parts(
            args.session_id.clone(),
            args.project_pattern.clone(),
            args.cwd_pattern.clone(),
            target_model.clone(),
        )?;
        config.push_str(&target_to_toml(&target));
    }

    Ok(config)
}

pub(crate) fn default_config_text() -> String {
    format!(
        "recent_hours = {DEFAULT_RECENT_HOURS}\ntranscript_quiet_seconds = {DEFAULT_TRANSCRIPT_QUIET_SECONDS}\ndebounce_seconds = {DEFAULT_DEBOUNCE_SECONDS}\n\n# Fable sessions are watched automatically. Add [[targets]] only for overrides.\n"
    )
}

pub(crate) fn ensure_config_file(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    fs::write(path, default_config_text())
        .with_context(|| format!("write config {}", path.display()))
}

pub(crate) fn target_rule_from_parts(
    session_id: Option<String>,
    project_pattern: Option<String>,
    cwd_pattern: Option<String>,
    target_model: String,
) -> Result<TargetRule> {
    if selector_count(&session_id, &project_pattern, &cwd_pattern) != 1 {
        bail!("set exactly one of --session-id, --project-pattern, or --cwd-pattern");
    }
    if target_model.trim().is_empty() {
        bail!("--target-model cannot be empty");
    }

    Ok(TargetRule {
        session_id,
        project_pattern,
        cwd_pattern,
        target_model,
    })
}

pub(crate) fn selector_count(
    session_id: &Option<String>,
    project_pattern: &Option<String>,
    cwd_pattern: &Option<String>,
) -> usize {
    [
        session_id.is_some(),
        project_pattern.is_some(),
        cwd_pattern.is_some(),
    ]
    .into_iter()
    .filter(|selected| *selected)
    .count()
}

pub(crate) fn add_target_to_config(path: &Path, target: &TargetRule) -> Result<bool> {
    let raw = parse_config_file(path)?;
    let existing_targets = validate_targets(raw.targets)?;
    if let Some(existing) = existing_targets
        .iter()
        .find(|existing| same_target_selector(existing, target))
    {
        if existing.target_model == target.target_model {
            return Ok(false);
        }
        bail!(
            "target selector already exists with target_model {}; edit {} to change it",
            existing.target_model,
            path.display()
        );
    }

    let mut raw =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    if !raw.ends_with('\n') {
        raw.push('\n');
    }
    raw.push_str(&target_to_toml(target));
    fs::write(path, raw).with_context(|| format!("write config {}", path.display()))?;
    let reparsed = parse_config_file(path)?;
    validate_targets(reparsed.targets)?;
    Ok(true)
}

pub(crate) fn remove_session_target_from_config(path: &Path, session_id: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let mut raw = parse_config_file(path)?;
    let before = raw.targets.len();
    raw.targets
        .retain(|target| target.session_id.as_deref() != Some(session_id));
    if raw.targets.len() == before {
        return Ok(false);
    }

    validate_targets(raw.targets.clone())?;
    fs::write(path, file_config_to_toml(&raw))
        .with_context(|| format!("write config {}", path.display()))?;
    Ok(true)
}

pub(crate) fn describe_target_rule(target: &TargetRule) -> String {
    let selector = if let Some(session_id) = &target.session_id {
        format!("session_id={session_id}")
    } else if let Some(project_pattern) = &target.project_pattern {
        format!("project_pattern={project_pattern}")
    } else if let Some(cwd_pattern) = &target.cwd_pattern {
        format!("cwd_pattern={cwd_pattern}")
    } else {
        "selector=<invalid>".to_string()
    };
    format!("{selector} -> {}", target.target_model)
}

pub(crate) fn load_config(cli: &Cli) -> Result<Config> {
    resolve_config(
        cli.config.clone(),
        cli.projects_dir.clone(),
        cli.recent_hours,
    )
}

/// The Cli-free core of [`load_config`], so non-CLI consumers (the desktop
/// app's library API) can resolve the same effective `Config` from optional
/// overrides plus the identical env/config/default precedence, without
/// synthesizing a fake `Cli`.
pub(crate) fn resolve_config(
    config_override: Option<PathBuf>,
    projects_dir_override: Option<PathBuf>,
    recent_hours_override: Option<u64>,
) -> Result<Config> {
    let home = home_dir()?;
    let mut raw = FileConfig::default();
    let config_path = config_path(config_override, &home);

    if config_path.exists() {
        raw = parse_config_file(&config_path)?;
    }

    let projects_dir = projects_dir_override
        .or_else(|| env::var_os("COUNTERSPELL_PROJECTS_DIR").map(PathBuf::from))
        .or(raw.projects_dir)
        .unwrap_or_else(|| home.join(".claude").join("projects"));
    let recent_hours = recent_hours_override
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

pub(crate) fn validate_targets(targets: Vec<TargetRule>) -> Result<Vec<TargetRule>> {
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

pub(crate) fn parse_config_file(path: &Path) -> Result<FileConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse config {}", path.display()))
}

pub(crate) fn config_path(config_arg: Option<PathBuf>, home: &Path) -> PathBuf {
    if let Some(path) = config_arg {
        return path;
    }
    if let Some(path) = env::var_os("COUNTERSPELL_CONFIG") {
        return PathBuf::from(path);
    }
    home.join(".counterspell").join("config.toml")
}

fn same_target_selector(left: &TargetRule, right: &TargetRule) -> bool {
    left.session_id == right.session_id
        && left.project_pattern == right.project_pattern
        && left.cwd_pattern == right.cwd_pattern
}

fn target_to_toml(target: &TargetRule) -> String {
    let mut config = "\n[[targets]]\n".to_string();
    if let Some(session_id) = &target.session_id {
        config.push_str(&format!("session_id = \"{}\"\n", escape_toml(session_id)));
    }
    if let Some(project_pattern) = &target.project_pattern {
        config.push_str(&format!(
            "project_pattern = \"{}\"\n",
            escape_toml(project_pattern)
        ));
    }
    if let Some(cwd_pattern) = &target.cwd_pattern {
        config.push_str(&format!("cwd_pattern = \"{}\"\n", escape_toml(cwd_pattern)));
    }
    config.push_str(&format!(
        "target_model = \"{}\"\n",
        escape_toml(&target.target_model)
    ));
    config
}

fn file_config_to_toml(config: &FileConfig) -> String {
    let mut raw = String::new();

    if let Some(projects_dir) = &config.projects_dir {
        raw.push_str(&format!(
            "projects_dir = \"{}\"\n",
            escape_toml(&projects_dir.to_string_lossy())
        ));
    }
    raw.push_str(&format!(
        "recent_hours = {}\n",
        config.recent_hours.unwrap_or(DEFAULT_RECENT_HOURS)
    ));
    raw.push_str(&format!(
        "transcript_quiet_seconds = {}\n",
        config
            .transcript_quiet_seconds
            .unwrap_or(DEFAULT_TRANSCRIPT_QUIET_SECONDS)
    ));
    raw.push_str(&format!(
        "debounce_seconds = {}\n\n",
        config.debounce_seconds.unwrap_or(DEFAULT_DEBOUNCE_SECONDS)
    ));
    raw.push_str(
        "# Fable sessions are watched automatically. Add [[targets]] only for overrides.\n",
    );

    for target in &config.targets {
        raw.push_str(&target_to_toml(target));
    }

    raw
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
