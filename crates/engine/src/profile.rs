//! Backup profiles.
//!
//! A profile is the complete, declarative description of one backup job:
//! what to back up, where to back it up to, what to skip, and how duplicity
//! should behave. Every duplicity invocation the engine ever makes is derived
//! from a profile — nothing is hard-coded ad hoc.
//!
//! Profiles persist as individual TOML files in the user's config directory
//! (typically `~/.config/gui-for-duplicity/profiles/*.toml`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One backup job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Human-readable name; also used as the profile file name.
    pub name: String,
    /// Source directories to back up.
    pub sources: Vec<PathBuf>,
    /// Duplicity target URL, e.g. `file:///mnt/backup/dup` or `sftp://user@host/dir`.
    pub target: String,
    /// Glob patterns of things to exclude (relative to the sources).
    #[serde(default)]
    pub excludes: Vec<String>,
    /// Glob patterns to re-include after an exclude (relative to the sources).
    #[serde(default)]
    pub includes: Vec<String>,
    /// Extra raw options passed through to duplicity (e.g. `--volsize=500`).
    #[serde(default)]
    pub extra_options: Vec<String>,
    /// GPG-related options, e.g. `--encrypt-key=0123ABC` for asymmetric encryption.
    #[serde(default)]
    pub gpg_options: Vec<String>,
    /// How long to keep backups, as the argument to `remove-older-than`
    /// (e.g. `6M`). Empty means "never prune automatically".
    #[serde(default)]
    pub retention: String,
    /// Whether backup data is GPG-encrypted (a passphrase is then required).
    /// Defaults to `true` — the secure default; profiles created before this
    /// field existed are treated as encrypted.
    #[serde(default = "default_true")]
    pub encrypt: bool,
    /// Encryption state of the target's chain at the last successful backup,
    /// recorded so that changing `encrypt` mid-chain can be caught (a switch
    /// requires starting a new chain, not an incremental).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_backup_encrypt: Option<bool>,
}

fn default_true() -> bool {
    true
}

impl Profile {
    /// The standard location for profiles on Linux:
    /// `$XDG_CONFIG_HOME/gui-for-duplicity/profiles` (typically
    /// `~/.config/gui-for-duplicity/profiles`).
    pub fn default_profiles_dir() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                home.join(".config")
            });
        base.join("gui-for-duplicity").join("profiles")
    }

    /// Save this profile to the default profiles directory
    /// (`<dir>/<name>.toml`).
    pub fn save_default(&self) -> Result<PathBuf> {
        let dir = Self::default_profiles_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.toml", self.name));
        self.save(&path)?;
        Ok(path)
    }

    /// Load a profile by name from the default directory.
    pub fn load_default(name: &str) -> Result<Self> {
        Self::load(&Self::default_profiles_dir().join(format!("{name}.toml")))
    }

    /// List profile names available in the default directory.
    pub fn list_default() -> Vec<String> {
        let dir = Self::default_profiles_dir();
        let mut names = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    if let Some(stem) = path.file_stem() {
                        names.push(stem.to_string_lossy().to_string());
                    }
                }
            }
        }
        names.sort();
        names
    }

    /// Load a profile from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading profile {}", path.display()))?;
        let profile: Profile = toml::from_str(&text)
            .with_context(|| format!("parsing profile {}", path.display()))?;
        Ok(profile)
    }

    /// Save this profile as TOML at `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing profile {}", path.display()))?;
        Ok(())
    }

    /// Build the argument list for a `duplicity backup` (or a forced new
    /// full chain when `force_full`) — without the leading `duplicity`
    /// program name. Passphrases are NEVER placed here — they travel via
    /// the process environment, see [`crate::runner`].
    pub fn backup_args(&self, force_full: bool) -> Vec<String> {
        let mut args = Vec::new();
        for inc in &self.includes {
            args.push("--include".into());
            args.push(inc.clone());
        }
        for exc in &self.excludes {
            args.push("--exclude".into());
            args.push(exc.clone());
        }
        if !self.encrypt {
            args.push("--no-encryption".into());
        }
        args.extend(self.gpg_options.iter().cloned());
        args.extend(self.extra_options.iter().cloned());
        // duplicity takes sources then target, sources last.
        if force_full {
            args.push("full".into());
        } else {
            args.push("backup".into());
        }
        for src in &self.sources {
            args.push(src.display().to_string());
        }
        args.push(self.target.clone());
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_roundtrip_toml() {
        let profile = Profile {
            name: "home-docs".into(),
            sources: vec![PathBuf::from("/home/aidan/Documents")],
            target: "file:///mnt/backup/dup".into(),
            excludes: vec!["**/.cache/**".into()],
            includes: vec![],
            extra_options: vec!["--volsize=500".into()],
            gpg_options: vec![],
            retention: "6M".into(),
            encrypt: true,
            last_backup_encrypt: None,
        };
        let text = toml::to_string_pretty(&profile).unwrap();
        let back: Profile = toml::from_str(&text).unwrap();
        assert_eq!(back.name, "home-docs");
        assert_eq!(back.retention, "6M");
        assert!(back.encrypt);
        assert_eq!(back.backup_args(false).last().unwrap(), "file:///mnt/backup/dup");
    }

    #[test]
    fn unencrypted_profile_adds_no_encryption_flag() {
        let profile = Profile {
            name: "t".into(),
            sources: vec![PathBuf::from("/a")],
            target: "file:///x".into(),
            excludes: vec![],
            includes: vec![],
            extra_options: vec![],
            gpg_options: vec![],
            retention: String::new(),
            encrypt: false,
            last_backup_encrypt: None,
        };
        let args = profile.backup_args(false);
        assert!(args.contains(&"--no-encryption".to_string()));
        assert_eq!(args[args.len() - 3], "backup");

        // Forced new chain uses the "full" action instead.
        let full = profile.backup_args(true);
        assert_eq!(full[full.len() - 3], "full");

        // And the TOML default for a legacy file without the field is
        // *encrypted* (secure default):
        let legacy: Profile =
            toml::from_str("name = 'old'\ntarget = 'file:///x'\nsources = []").unwrap();
        assert!(legacy.encrypt);
    }

    #[test]
    fn backup_args_order_sources_last() {
        let profile = Profile {
            name: "t".into(),
            sources: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            target: "file:///x".into(),
            excludes: vec!["**/tmp/**".into()],
            includes: vec![],
            extra_options: vec![],
            gpg_options: vec![],
            retention: String::new(),
            encrypt: true,
            last_backup_encrypt: None,
        };
        let args = profile.backup_args(false);
        assert_eq!(&args[0..3], &["--exclude", "**/tmp/**", "backup"]);
        assert_eq!(&args[3..5], &["/a", "/b"]);
        assert_eq!(args[5], "file:///x");
    }
}
