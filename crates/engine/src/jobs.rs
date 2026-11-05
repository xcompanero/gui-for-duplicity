//! Canonical backup-job orchestration, shared by the GUI and the CLI.
//!
//! Everything that must be true regardless of front-end lives here: profile
//! loading, the restore-only guard, the encryption-change guard, credential
//! resolution via the Secret Service (KWallet/ksecretd), argument building,
//! running duplicity, and recording the chain's encryption state on success.

use crate::profile::Profile;
use crate::runner::{run_duplicity, Credentials};
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
/// `last_backup_encrypt` is updated so future setting changes are caught.
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
    Ok(JobOutcome {
        success,
        message: if success {
            format!(
                "Backup finished successfully. (took {})",
                format_elapsed(started.elapsed())
            )
        } else {
            "Backup FAILED — see log above.".to_string()
        },
    })
}
