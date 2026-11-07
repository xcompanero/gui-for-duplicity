//! Desktop notifications via `notify-send` (the freedesktop notification
//! protocol, served by Plasma's notification centre).
//!
//! Kept deliberately thin: if `notify-send` is missing or the call fails,
//! notifications are silently skipped — a backup tool must never fail
//! *because* it tried to tell you about itself.

use std::process::{Command, Stdio};

/// Send a desktop notification. `urgent` sets the urgency hint.
/// A missing or failing `notify-send` is logged to stderr (visible in the
/// journal for scheduled runs) but never propagates — a backup must never
/// fail *because* it tried to speak.
pub fn send(title: &str, body: &str, urgent: bool) {
    let urgency = if urgent { "critical" } else { "normal" };
    match Command::new("notify-send")
        .arg(title)
        .arg(body)
        .args(["--app-name=GUI For Duplicity"])
        .arg(format!("--urgency={urgency}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => eprintln!("[notify] sent ({urgency}): {title}"),
        Err(e) => eprintln!("[notify] FAILED to launch notify-send: {e}"),
    }
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
