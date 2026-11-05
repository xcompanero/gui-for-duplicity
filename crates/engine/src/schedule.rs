//! Scheduled backups via systemd user timers.
//!
//! Per profile we generate two units in `~/.config/systemd/user/`:
//!
//! - `gfd-<profile>.service` — a oneshot service running
//!   `engine-cli backup <profile>` (fully headless; passphrase comes from
//!   the Secret Service, available on the user session bus).
//! - `gfd-<profile>.timer` — a Persistent timer with the profile's
//!   OnCalendar expression, so missed runs (machine off) run at next boot.
//!
//! Enabling/disabling goes through `systemctl --user`, the standard,
//! session-integrated way — visible in KDE's startup services and
//! manageable with plain systemd tooling.

use crate::profile::Profile;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// The engine-cli binary the service will invoke. In development this is the
/// debug build; a packaged install will point at the installed path.
pub fn engine_cli_path() -> Result<PathBuf> {
    // Locate ourselves: the CLI binary sits next to whatever invoked us when
    // run from the target dir; fall back to the workspace target dir.
    let current = std::env::current_exe()?;
    if current.file_name().is_some_and(|f| f == "engine-cli") {
        return Ok(current);
    }
    // GUI process running from target/debug — the CLI sits beside it.
    let sibling = current
        .parent()
        .map(|d| d.join("engine-cli"))
        .context("no parent dir for current executable")?;
    if sibling.is_file() {
        Ok(sibling)
    } else {
        bail!(
            "engine-cli not found next to {} — build the workspace first (cargo build)",
            current.display()
        )
    }
}

/// systemd-safe unit identifier for a profile name.
pub fn unit_stem(profile_name: &str) -> String {
    let mut stem: String = profile_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse repeats and trim dashes for tidy unit names.
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    while stem.starts_with('-') || stem.starts_with('.') {
        stem.remove(0);
    }
    while stem.ends_with('-') {
        stem.pop();
    }
    if stem.is_empty() {
        stem = "profile".to_string();
    }
    format!("gfd-{stem}")
}

fn user_units_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("no HOME set")?;
    Ok(home.join(".config").join("systemd").join("user"))
}

pub fn service_path(profile_name: &str) -> Result<PathBuf> {
    Ok(user_units_dir()?.join(format!("{}.service", unit_stem(profile_name))))
}

pub fn timer_path(profile_name: &str) -> Result<PathBuf> {
    Ok(user_units_dir()?.join(format!("{}.timer", unit_stem(profile_name))))
}

/// Write (or refresh) the service+timer units for a profile.
pub fn write_units(profile: &Profile) -> Result<()> {
    if profile.read_only {
        bail!("'{}' is restore-only; scheduling is disabled", profile.name);
    }
    let dir = user_units_dir()?;
    std::fs::create_dir_all(&dir)?;

    let cli = engine_cli_path()?.display().to_string();
    let stem = unit_stem(&profile.name);

    let service = format!(
        "[Unit]\n\
         Description=GUI For Duplicity backup: {name}\n\
         Documentation=man:duplicity(1)\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         # User services run with the session bus available; be explicit anyway.\n\
         Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus\n\
         ExecStart={cli} backup {name}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        name = profile.name,
        cli = cli,
    );
    std::fs::write(dir.join(format!("{stem}.service")), service)?;

    let timer = format!(
        "[Unit]\n\
         Description=Scheduled backup: {name}\n\
         \n\
         [Timer]\n\
         OnCalendar={cal}\n\
         Persistent=true\n\
         Unit={stem}.service\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        name = profile.name,
        cal = profile.schedule,
        stem = stem,
    );
    std::fs::write(dir.join(format!("{stem}.timer")), timer)?;
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("running systemctl --user (is systemd running your session?)")?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let errtext = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        bail!("systemctl --user {} failed: {}", args.join(" "), errtext.trim());
    }
    Ok(text)
}

/// Reload unit files, then enable and start the timer for a profile.
pub fn enable(profile_name: &str) -> Result<()> {
    let stem = unit_stem(profile_name);
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{stem}.timer")])?;
    Ok(())
}

/// Stop and disable the timer for a profile (units are left in place).
pub fn disable(profile_name: &str) -> Result<()> {
    let stem = unit_stem(profile_name);
    systemctl(&["disable", "--now", &format!("{stem}.timer")])?;
    systemctl(&["daemon-reload"])?;
    Ok(())
}

/// Current scheduling state for a profile.
#[derive(Debug, Default, serde::Serialize)]
pub struct ScheduleStatus {
    /// The OnCalendar expression from the profile (may be set but disabled).
    pub calendar: Option<String>,
    /// The timer is enabled in systemd.
    pub enabled: bool,
    /// Next scheduled run, as reported by systemd.
    pub next_run: Option<String>,
}

pub fn status(profile: &Profile) -> Result<ScheduleStatus> {
    let stem = unit_stem(&profile.name);
    let enabled = systemctl(&["is-enabled", &format!("{stem}.timer")])
        .map(|s| s.trim().to_string())
        .map(|s| s == "enabled")
        .unwrap_or(false);

    let mut next_run = None;
    // list-timers output: NEXT LEFT LAST PASSED UNIT ...
    if let Ok(timers) = systemctl(&["list-timers", &format!("{stem}.timer"), "--no-legend"]) {
        if let Some(first) = timers.lines().next() {
            next_run = Some(
                first
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
    }

    Ok(ScheduleStatus {
        calendar: if profile.schedule.is_empty() {
            None
        } else {
            Some(profile.schedule.clone())
        },
        enabled,
        next_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_stems_are_safe_and_stable() {
        assert_eq!(unit_stem("playground"), "gfd-playground");
        assert_eq!(unit_stem("Magrathea"), "gfd-magrathea");
        assert_eq!(unit_stem("Home Docs!!"), "gfd-home-docs");
        assert_eq!(unit_stem("!!!"), "gfd-profile");
    }
}
