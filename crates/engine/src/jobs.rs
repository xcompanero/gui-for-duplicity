//! Canonical backup-job orchestration, shared by the GUI and the CLI.
//!
//! Everything that must be true regardless of front-end lives here: profile
//! loading, the restore-only guard, the encryption-change guard, credential
//! resolution via the Secret Service (KWallet/ksecretd), argument building,
//! running duplicity, and recording the chain's encryption state on success.

use crate::profile::Profile;
use crate::runner::{self, run_duplicity, Credentials};
use anyhow::{bail, Context, Result};
use std::time::Instant;

/// The result of a completed backup run.
#[derive(Debug)]
pub struct JobOutcome {
    pub success: bool,
    pub message: String,
}

/// A validated, ready-to-run backup job.
pub struct PreparedBackup {
    pub profile: Profile,
    pub args: Vec<String>,
    pub creds: Credentials<'static>,
}

fn format_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Validate a profile for a backup run and build everything the runner needs.
/// Deliberately cheap and side-effect free so front-ends can surface refusals
/// (read-only profiles, encryption changes, missing passphrases) immediately.
pub fn prepare_backup(profile_name: &str, force_full: bool) -> Result<PreparedBackup> {
    let profile = Profile::load_default(profile_name)
        .with_context(|| format!("loading profile {profile_name:?}"))?;

    if profile.read_only {
        bail!(
            "'{}' is a restore-only profile (frozen archive). Backups from it are disabled.",
            profile.name
        );
    }

    // Encryption changes are only safe at the start of a new chain.
    if let Some(last) = profile.last_backup_encrypt {
        if last != profile.encrypt && !force_full {
            bail!(
                "The encryption setting for '{}' changed since the last backup \
                 (target chain was created with encryption {}). \
                 An incremental cannot cross that boundary safely — start a new chain instead.",
                profile.name,
                if last { "ON" } else { "OFF" }
            );
        }
    }

    let creds = if profile.encrypt {
        let stored = crate::secrets::get_passphrase(&profile.target)
            .with_context(|| "looking up the passphrase in the Secret Service (KWallet)")?;
        let pass = match stored {
            Some(p) => p,
            None => bail!(
                "Profile '{}' has encryption enabled, but no passphrase is available.\n\
                 Save the GPG passphrase for this target in KWallet, or untick \
                 'Encrypt backups' if this target should be unencrypted.",
                profile.name
            ),
        };
        Credentials {
            // Bounded 'static leak: one owned string per profile per process.
            passphrase: Some(&*Box::leak(pass.into_boxed_str())),
            ..Default::default()
        }
    } else {
        Credentials::new()
    };

    let args = profile.backup_args(force_full);
    Ok(PreparedBackup { profile, args, creds })
}

/// Run a prepared backup to completion (blocking). On success the profile's
/// `last_backup_encrypt` is updated so future setting changes are caught,
/// retention/cleanup housekeeping runs (if configured), and a desktop
/// notification is sent for failures and for long-running successful jobs.
pub fn run_backup(
    prepared: &PreparedBackup,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<JobOutcome> {
    let started = Instant::now();
    let run = run_duplicity(&prepared.args, &prepared.creds, on_stderr_line)?;
    let success = run.success();
    if success {
        if let Ok(mut p) = Profile::load_default(&prepared.profile.name) {
            p.last_backup_encrypt = Some(prepared.profile.encrypt);
            let _ = p.save_default();
        }
    }

    // Housekeeping: prune old sets per the retention policy and clear
    // orphaned/incomplete leftovers. Only after a *successful* backup —
    // pruning on top of a failure could destroy the only good chain.
    let mut housekeeping = Vec::new();
    if success && !prepared.profile.retention.is_empty() {
        let target = &prepared.profile.target;
        housekeeping.push(format!(
            "Retention: removing sets older than {} …",
            prepared.profile.retention
        ));
        match runner::remove_older_than(
            target,
            &prepared.profile.retention,
            true,
            &prepared.creds,
            on_stderr_line,
        ) {
            Ok(r) if r.success() => housekeeping.push("Retention: done.".into()),
            Ok(r) => housekeeping.push(format!(
                "Retention: FAILED — {}",
                r.stderr.lines().last().unwrap_or_default()
            )),
            Err(e) => housekeeping.push(format!("Retention: FAILED — {e:#}")),
        }
        match runner::cleanup(target, true, &prepared.creds, on_stderr_line) {
            Ok(r) if r.success() => {
                let n = r.stdout.lines().count();
                housekeeping.push(format!("Cleanup: done ({n} line report)."));
            }
            Ok(r) => housekeeping.push(format!(
                "Cleanup: FAILED — {}",
                r.stderr.lines().last().unwrap_or_default()
            )),
            Err(e) => housekeeping.push(format!("Cleanup: FAILED — {e:#}")),
        }
    }
    for line in &housekeeping {
        if let Some(cb) = on_stderr_line {
            cb(line);
        }
    }

    // Desktop notification: always for failures; for successes only when the
    // run was long enough to be worth announcing.
    let elapsed = started.elapsed();
    if !success {
        crate::notify::send(
            "Backup FAILED",
            &format!("Profile '{}': see the log for details.", prepared.profile.name),
            true,
        );
    } else if elapsed >= std::time::Duration::from_secs(60) {
        crate::notify::send(
            "Backup finished",
            &format!(
                "Profile '{}' completed in {}.",
                prepared.profile.name,
                format_elapsed(elapsed)
            ),
            false,
        );
    }

    Ok(JobOutcome {
        success,
        message: if success {
            format!(
                "Backup finished successfully. (took {}){}",
                format_elapsed(elapsed),
                if housekeeping.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", housekeeping.join("\n"))
                }
            )
        } else {
            "Backup FAILED — see log above.".to_string()
        },
    })
}
