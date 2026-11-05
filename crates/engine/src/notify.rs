//! Desktop notifications via `notify-send` (the freedesktop notification
//! protocol, served by Plasma's notification centre).
//!
//! Kept deliberately thin: if `notify-send` is missing or the call fails,
//! notifications are silently skipped — a backup tool must never fail
//! *because* it tried to tell you about itself.

use std::process::{Command, Stdio};

/// Send a desktop notification. `urgent` sets the urgency hint.
/// `spawn()` fails silently if notify-send is missing — that's fine.
pub fn send(title: &str, body: &str, urgent: bool) {
    let _ = Command::new("notify-send")
        .arg(title)
        .arg(body)
        .args(["--app-name=GUI For Duplicity"])
        .arg(if urgent { "--urgency=critical" } else { "--urgency=normal" })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    /// A missing notify-send must be tolerated silently (no panic); when it
    /// exists this test may briefly display a notification on the desktop.
    #[test]
    fn missing_or_present_never_panics() {
        super::send("GUI For Duplicity test", "ignore me", false);
    }
}
