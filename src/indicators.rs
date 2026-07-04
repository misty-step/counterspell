use anyhow::{bail, Context, Result};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::util::{home_dir, shell_param_default, xml_escape};

pub(crate) const LAUNCH_AGENT_LABEL: &str = "com.misty-step.counterspell.annotate-herdr";
pub(crate) const WATCH_ARM_LAUNCH_AGENT_LABEL: &str = "com.misty-step.counterspell.watch-arm";
const SWIFTBAR_PLUGIN: &str = include_str!("../extras/swiftbar/counterspell.5m.sh");

pub(crate) fn swiftbar_plugin_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("SwiftBar")
        .join("Plugins")
        .join("counterspell.5m.sh")
}

pub(crate) fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
}

pub(crate) fn watch_arm_launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{WATCH_ARM_LAUNCH_AGENT_LABEL}.plist"))
}

pub(crate) fn write_swiftbar_plugin(path: &Path, bin: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create SwiftBar plugin dir {}", parent.display()))?;
    }
    let bin_default = shell_param_default(bin);
    let script = SWIFTBAR_PLUGIN.replace(
        r#"COUNTERSPELL_BIN="${COUNTERSPELL_BIN:-counterspell}""#,
        &format!(r#"COUNTERSPELL_BIN="${{COUNTERSPELL_BIN:-{bin_default}}}""#),
    );
    fs::write(path, script).with_context(|| format!("write SwiftBar plugin {}", path.display()))?;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("read SwiftBar plugin metadata {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("chmod SwiftBar plugin {}", path.display()))?;
    Ok(())
}

pub(crate) fn write_launch_agent(path: &Path, bin: &Path, interval_secs: u64) -> Result<()> {
    write_counterspell_launch_agent(
        path,
        LAUNCH_AGENT_LABEL,
        &[
            bin.to_string_lossy().into_owned(),
            "--annotate-herdr".to_string(),
        ],
        "counterspell-annotate-herdr.log",
        "counterspell-annotate-herdr.err.log",
        interval_secs,
    )
}

pub(crate) fn write_watch_arm_launch_agent(
    path: &Path,
    bin: &Path,
    interval_secs: u64,
) -> Result<()> {
    write_counterspell_launch_agent(
        path,
        WATCH_ARM_LAUNCH_AGENT_LABEL,
        &[
            bin.to_string_lossy().into_owned(),
            "watch".to_string(),
            "--arm".to_string(),
        ],
        "counterspell-watch-arm.log",
        "counterspell-watch-arm.err.log",
        interval_secs,
    )
}

fn write_counterspell_launch_agent(
    path: &Path,
    label: &str,
    program_arguments: &[String],
    stdout_name: &str,
    stderr_name: &str,
    interval_secs: u64,
) -> Result<()> {
    if interval_secs == 0 {
        bail!("--interval-secs must be greater than zero");
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create LaunchAgents dir {}", parent.display()))?;
    }
    let home = home_dir()?;
    let stdout = home.join("Library").join("Logs").join(stdout_name);
    let stderr = home.join("Library").join("Logs").join(stderr_name);
    let program_arguments = program_arguments
        .iter()
        .map(|argument| format!("    <string>{}</string>", xml_escape(argument)))
        .collect::<Vec<_>>()
        .join("\n");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
{}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>{}</integer>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(label),
        program_arguments,
        interval_secs,
        xml_escape(&stdout.to_string_lossy()),
        xml_escape(&stderr.to_string_lossy())
    );
    fs::write(path, plist).with_context(|| format!("write LaunchAgent {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_launch_agent(path: &Path, label: &str) -> Result<()> {
    let launchctl = launchctl_bin();
    let uid_output = ProcessCommand::new("id")
        .arg("-u")
        .output()
        .context("run id -u for launchctl domain")?;
    if !uid_output.status.success() {
        bail!("id -u exited with {}", uid_output.status);
    }
    let uid = String::from_utf8_lossy(&uid_output.stdout)
        .trim()
        .to_string();
    let domain = format!("gui/{uid}");
    let _ = ProcessCommand::new(&launchctl)
        .args(["bootout", &domain, &path.to_string_lossy()])
        .output();
    let bootstrap = ProcessCommand::new(&launchctl)
        .args(["bootstrap", &domain, &path.to_string_lossy()])
        .output()
        .context("run launchctl bootstrap")?;
    if !bootstrap.status.success() {
        bail!(
            "launchctl bootstrap exited with {}: {}",
            bootstrap.status,
            String::from_utf8_lossy(&bootstrap.stderr)
        );
    }
    let service = format!("{domain}/{label}");
    let kickstart = ProcessCommand::new(&launchctl)
        .args(["kickstart", "-k", &service])
        .output()
        .context("run launchctl kickstart")?;
    if !kickstart.status.success() {
        bail!(
            "launchctl kickstart exited with {}: {}",
            kickstart.status,
            String::from_utf8_lossy(&kickstart.stderr)
        );
    }
    Ok(())
}

pub(crate) fn launch_agent_scheduled(label: &str) -> Result<bool> {
    let launchctl = launchctl_bin();
    let uid_output = ProcessCommand::new("id")
        .arg("-u")
        .output()
        .context("run id -u for launchctl domain")?;
    if !uid_output.status.success() {
        bail!("id -u exited with {}", uid_output.status);
    }
    let uid = String::from_utf8_lossy(&uid_output.stdout)
        .trim()
        .to_string();
    let service = format!("gui/{uid}/{label}");
    let output = ProcessCommand::new(launchctl)
        .args(["print", &service])
        .output()
        .context("run launchctl print")?;
    Ok(output.status.success())
}

fn launchctl_bin() -> OsString {
    env::var_os("COUNTERSPELL_LAUNCHCTL_BIN").unwrap_or_else(|| OsString::from("launchctl"))
}
