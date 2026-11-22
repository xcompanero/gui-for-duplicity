# DEMO.md — Linux User Group show-and-tell kit

*Prepared 2026-11-07. Everything needed to present GUI For Duplicity to a
LUG audience: runbook, talking points, war stories, Q&A prep, handout.*

---

## 1. The elevator pitch (memorise this)

> "When Déjà Dup abandoned the Duplicity backup engine for Restic, we lost
> the things that made backups feel trustworthy: verbose output, visible
> progress, easy verification, and restores that don't fight you. So I
> built my own front end — in Rust, driving duplicity through its command
> line, with no Python in the GUI at all. It's been running nightly on two
> machines for a week: scheduled backups, retention, notifications, honest
> verification, and a file browser that time-travels through two years of
> history. Tonight I'll show you all of it."

## 2. Pre-demo checklist (do BEFORE the meeting)

- [ ] **Rehearse once the same day.** Live demos decay overnight.
- [ ] Demo profiles ready and tiny: `playground` (source with a couple of
      files, local target) — everything live is run on it. Nothing long or
      risky is ever run in front of an audience.
- [ ] **the frozen archive's listing cache is warm** (browse it once before leaving
      home) — the time-travel demo must be instant, never a 57-minute parse.
- [ ] Have the **screenshot folder** on a USB stick and/or open in an image
      viewer as the fallback if the live demo misbehaves.
- [ ] Known-quirk check before leaving home: the app's sidebar shows the
      build tag — if it's missing or old, rebuild + reinstall
      (`cargo build --release && ./scripts/install.sh`).
- [ ] Optional props: the NAS reachable over the venue LAN? (Probably not —
      demo everything that needs the NAS from the cached/browser views, or
      skip live NAS demos. Local playground covers the live-action slots.)
- [ ] Decide in advance: is the repo public yet? If someone asks "where's
      the code?" the honest answer is either a URL or "it'll be on GitHub
      shortly — find me afterwards and I'll note your interest."

## 3. The demo runbook (~10 minutes)

| # | Beat | Show | Time |
|---|---|---|---|
| 1 | The pitch | §1 above | 1–2 min |
| 2 | Profiles are data | Sidebar; open `playground.toml` in Kate next to the GUI — "declarative, versionable, human-readable" | 1 min |
| 3 | Status view | the frozen archive: chains, primary/secondary badges, volume counts, dates — "the anatomy of two years of backups" | 1–2 min |
| 4 | Time-travel browser | "⌛ browse as of…" on any set; walk the tree; filter box | 2 min |
| 5 | Live backup | Playground → Back up now → live log streams → success notification pops | 1–2 min |
| 6 | Live restore | Browser → ⤓ on a file → destination dialog → confirm → restore → "Open folder" | 1–2 min |
| 7 | Verify | Playground → Verify against source → "VERIFIED — N files, 0 differences" | 1 min |
| 8 | Close | The trust ladder + the lessons (§5) | 1 min |

**If something misbehaves live:** say "this is why we have screenshots",
switch to the image viewer, and keep talking — the war stories (§5) work
better than the demo anyway.

## 4. The architecture slide (one diagram, even on a whiteboard)

```
Tauri GUI (Rust + webview)     engine-cli (headless — what systemd runs)
        \                        /
         ▼                      ▼
        ┌──────────────────────────┐
        │   engine (Rust library)  │
        │  profiles · commands ·   │
        │  parsers · caches ·      │
        │  secrets · jobs ·        │
        │  systemd timers · notify │
        └────────────┬─────────────┘
                     ▼  (subprocess, argv + env only — no Python API)
              duplicity (Python)
                     ▼
          targets: NAS (sftp/ftp), disk, S3, …
```

Key points to make:
- **No Python in the GUI** — duplicity is driven purely via CLI; all
  duplicity knowledge lives in ONE library crate, with GUI and CLI as thin
  clients (the same code path powers the button and the 4 a.m. cron-style run).
- **Secrets never touch argv** — environment variables + the desktop secret
  service (KWallet/ksecretd), unlocked at login via PAM.
- **Logs are not a disk concern** — journald self-vacuums; the tool's caches
  are the real story (and are bounded).

## 5. The war stories (pick two or three — they carry the talk)

1. **The 900-pound gorilla.** My first "production" backup profile nearly
   included a 120 GB project directory — of which 102 GB was one `media/` folder,
   and 7.6 GB was *the backup tool's own build directory*. Measure before
   you trust a glob.
2. **Sync is not backup.** The project tree syncs to a Nextcloud server — great
   for hardware failure, useless against ransomware and deletion storms,
   which propagate. Sync covers gradual risks; backups cover sudden ones.
   You want both.
3. **The 57-minute listing.** Duplicity re-parses 1.7 GB of signatures on
   *every* `list-current-files`. The fix: cache the parsed *index*, validate
   it cheaply against the chain state — 57 minutes → 1 second. Lesson:
   cache the expensive *result*, validate it cheaply.
4. **The fixes that never reached the screen.** Tauri's build only
   re-embeds frontend assets when config files change — my UI fixes sat in
   git for days while the binary served stale ones. And then a CSS rule
   (`display: flex` overriding the `hidden` attribute) made banners
   un-hideable. Two independent bugs, each masking the other.
5. **Where are my secrets?** KDE's ksecretd serves the Secret Service API
   and writes into KWallet storage, unlocked at login by PAM — so KWallet
   Manager looks empty to the casual eye. When your password manager and
   your secret daemon are different programs, read the D-Bus tree.
6. **The dotfiles illusion.** A 7,000-entry backup listing sorted
   alphabetically starts with 900 dot-entries — "it only backed up my
   dotfiles!" — while 6,200 real files followed. Always summarise before
   sampling.

## 6. Anticipated questions (rehearse answers)

- **"Why not just use Déjà Dup / borg / restic / Kup?"** — Déjà Dup dropped
  duplicity; the others are different engines. The point is keeping
  duplicity's mature, multi-backend, GPG-native engine behind a usable
  interface — and restoring *my* workflow.
- **"Why Rust, why not Python?"** — duplicity is already Python; the GUI
  adds value by being a different layer: fast, typed parsing of its
  output, Tauri desktop integration, and no third Python interpreter
  problem on the desktop.
- **"Is it on GitHub?"** — see §2 checklist; have your answer ready.
- **"What about encryption?"** — GPG symmetric (passphrase in KWallet) or
  asymmetric keys; the profile records the encryption state and refuses
  mid-chain changes (they corrupt archives).
- **"How do you handle the passphrase for a 4 a.m. scheduled run?"** —
  the desktop secret service, unlocked at login by PAM; the systemd unit
  explicitly points at the session bus.
- **"Doesn't duplicity's cache eat the disk?"** — yes, and that's duplicity
  (measure: 5.5 GB here). Our index cache is bounded (30 files / 100 MB,
  oldest-first eviction). Journald vacuums itself.
- **"Restore to original location — isn't that dangerous?"** — duplicity
  *never* overwrites: it refuses if the destination exists (verified
  empirically). Restores can only add, never clobber.
- **"How much did it cost / how long?"** — a hobby project: evenings over
  about a week, zero cost, and a week of daily-driver shakedown.

## 7. One-page handout (copy into the LUG's shared notes)

**GUI For Duplicity** — a Rust desktop front end for the Duplicity backup
engine. Profiles as TOML; scheduled backups via systemd user timers;
verbose live logs; snapshot browsing with point-in-time history; safe
restores (duplicity never overwrites); data verification with difference
reports; retention housekeeping; desktop notifications; secrets in
KWallet. GUI + headless CLI sharing one engine library.

- Stack: Rust, Tauri v2, systemd user units, duplicity 3.x (any backend).
- Build: `cargo build --release` + `./scripts/install.sh` (or a .deb via
  `./scripts/build-deb.sh`).
- License: GPL-3.0-or-later.
- Lessons for any Linux user: sync is not backup; verify your backups;
  measure before you trust a glob; keep one credential per job; write
  things down (this project's notes survived three debugging sessions that
  would otherwise have been re-lived).

## 8. Follow-ups to offer the room

- "Find me afterwards if you'd like the repo link when it's published."
- "Ask me about the Synology FTP path puzzle if you have 10 minutes."
- Anyone running duplicity by hand will recognise the CLI — the engine's
  command surface (`engine-cli status/files/verify/restore/schedule`) maps
  one-to-one onto duplicity's own verbs.
