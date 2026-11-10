# Changelog

## v0.1.0 — first public release (2026-11-08)

Initial public release of **GUI For Duplicity** — a Rust desktop front end
for the Duplicity backup engine.

### Features

- **Profiles** as declarative TOML: sources, any duplicity target URL
  (sftp, ftp, file, S3, B2, …), include/exclude rules, GPG options,
  retention, schedule.
- **Backups** — manual or scheduled via systemd user timers
  (`Persistent=true`, so missed runs catch up after downtime).
- **Verbose live logs** — every duplicity invocation streams into the GUI.
- **Snapshot browser** — the complete file tree of any backup set,
  including historical states ("browse as of…"), with a parsed-index cache
  that makes large archives instant.
- **Safe restores** — to a chosen destination or the original location;
  duplicity's never-overwrite guarantee surfaced honestly.
- **Verification** — full data comparison against the source with a
  classified difference report (mtime-only / new-file / content) and a
  per-area summary of where changes happened.
- **Secrets** — passphrases stored in the desktop secret service
  (KWallet/ksecretd on KDE, gnome-keyring on GNOME), passed to duplicity
  via the environment only.
- **Retention housekeeping** — automatic `remove-older-than` and `cleanup`
  after successful backups, with a dry-run preview.
- **Notifications** — success and failure alerts on the desktop.
- **Headless CLI** (`engine-cli`) — backup, verify, restore, status, file
  listings, schedules and cache management from a terminal or scripts.
- **Debian package** build (`scripts/build-deb.sh`).

### Platform

- Built and tested on Kubuntu 26.04 (KDE Plasma), with backup targets on a
  Synology NAS (sftp/ftp) and a plain Ubuntu server (sftp).
- Uses desktop-agnostic freedesktop interfaces (Secret Service, systemd
  user units, freedesktop notifications) — other desktop environments are
  expected to work but are not yet exercised.
- License: GPL-3.0-or-later.
