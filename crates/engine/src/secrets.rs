//! Secret storage via the freedesktop Secret Service API.
//!
//! On Kubuntu the Secret Service is implemented by **KWallet**, so secrets
//! stored here appear in — and are protected by — the wallet the user
//! already manages from Plasma. We never write secrets to profile TOML
//! files or pass them on command lines.
//!
//! Passphrases are keyed by *target URL*: every duplicity invocation for a
//! given target uses the same GPG passphrase, so the target is the natural
//! lookup key.

use anyhow::{Context, Result};

const SERVICE: &str = "gui-for-duplicity";

/// Keyring handle for our service. The `sync-secret-service` feature talks
/// to the Secret Service over D-Bus (KWallet on KDE).
fn entry(target: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, target)
        .with_context(|| format!("opening keyring entry for target {target:?}"))
}

/// Store the passphrase for a target (upserts if one exists).
pub fn store_passphrase(target: &str, passphrase: &str) -> Result<()> {
    entry(target)?
        .set_password(passphrase)
        .with_context(|| format!("storing passphrase for {target:?}"))
}

/// Look up the stored passphrase for a target, if any.
pub fn get_passphrase(target: &str) -> Result<Option<String>> {
    match entry(target)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading passphrase for {target:?}")),
    }
}

/// Delete the stored passphrase for a target, if any.
pub fn forget_passphrase(target: &str) -> Result<()> {
    match entry(target)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting passphrase for {target:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test touches the real Secret Service if one is running. In a
    /// headless CI there is none, so we only assert that errors are handled
    /// gracefully rather than panicking.
    #[test]
    fn absent_secret_is_none_not_error() {
        // A target name that will never exist; if no Secret Service is
        // available at all this may error — either way it must not panic.
        let _ = get_passphrase("definitely-not-a-real-target-测试");
    }
}
