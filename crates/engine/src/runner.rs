//! Running duplicity as a subprocess.
//!
//! Ground rules baked in here:
//! - The duplicity binary is invoked with argument lists; **passphrases never
//!   appear on the command line** — they are passed via the `PASSPHRASE`
//!   environment variable, which duplicity (and gpg) read.
//! - stdout/stderr are captured separately so parsers work on clean stdout.
//! - Duplicity holds a lock on its target while running; a second concurrent
//!   run for the same target fails fast rather than hanging.

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// Where duplicity was actually invoked from (useful for logs and debugging).
pub struct DuplicityRun {
    pub exit_status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

impl DuplicityRun {
    pub fn success(&self) -> bool {
        self.exit_status.success()
    }
}

/// Locate and validate the duplicity binary.
pub fn duplicity_path() -> Result<std::path::PathBuf> {
    let path = which_duplicity().context(
        "duplicity not found on PATH — is it installed? (sudo apt install duplicity)",
    )?;
    Ok(path)
}

fn which_duplicity() -> Option<std::path::PathBuf> {
    let path_env = std::env::var_os("PATH")?;
    std::env::split_paths(&path_env)
        .map(|dir| dir.join("duplicity"))
        .find(|candidate| candidate.is_file())
}

/// Credential/env handling for a duplicity run.
///
/// Ground rules: secrets NEVER appear on the command line. Duplicity reads:
/// - `PASSPHRASE`   — the GPG passphrase for encrypted targets
/// - `BACKEND_PASSWORD` — login password for remote backends (ftp, etc.);
///   duplicity 3.x's preferred generic mechanism (FTP_PASSWORD is deprecated)
#[derive(Debug, Default, Clone)]
pub struct Credentials<'a> {
    /// GPG passphrase for the backup data.
    pub passphrase: Option<&'a str>,
    /// Login password for the remote backend.
    pub backend_password: Option<&'a str>,
    /// Extra duplicity options for this run (e.g. `--verbosity info`),
    /// placed before the action word on the command line.
    pub extra_args: Vec<String>,
    /// Any other environment variables to inject for this run.
    pub extra_env: Vec<(String, String)>,
}

impl Credentials<'_> {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Run `duplicity collection-status <target>` and parse its output.
pub fn collection_status(
    target: &str,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<crate::collection::CollectionReport> {
    let run = run_duplicity(
        &["collection-status".to_string(), target.to_string()],
        credentials,
        on_stderr_line,
    )?;
    if !run.success() {
        anyhow::bail!(
            "duplicity collection-status failed ({}):\n{}",
            run.exit_status,
            run.stderr.trim()
        );
    }
    crate::collection::parse(&run.stdout)
}

/// Run `duplicity list-current-files <target>` and parse its output.
/// `time` is an optional duplicity `--time` value (e.g. `2026-11-05T12:44:24`).
pub fn list_current_files(
    target: &str,
    time: Option<&str>,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<crate::files::FileListing> {
    let mut args = vec!["list-current-files".to_string()];
    if let Some(t) = time {
        args.push("--time".into());
        args.push(t.to_string());
    }
    args.push(target.to_string());
    let run = run_duplicity(&args, credentials, on_stderr_line)?;
    if !run.success() {
        anyhow::bail!(
            "duplicity list-current-files failed ({}):\n{}",
            run.exit_status,
            run.stderr.trim()
        );
    }
    crate::files::parse(&run.stdout)
}

/// Outcome of a `duplicity verify` run. Note: `success == false` typically
/// means *differences were found* — that is a report, not a crash.
pub struct VerifyOutcome {
    pub report: crate::verify::VerifyReport,
    pub success: bool,
    pub stderr: String,
}

/// Run `duplicity verify [--compare-data] [--time T] <target> <local_path>`.
/// `selection` carries the profile's include/exclude rules so the verify
/// checks exactly the file set the backup covers — without them, excluded
/// files flood the difference report.
pub fn verify(
    target: &str,
    local_path: &str,
    compare_data: bool,
    time: Option<&str>,
    selection: &[String],
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<VerifyOutcome> {
    let mut args: Vec<String> = selection.to_vec();
    args.push("verify".into());
    if compare_data {
        args.push("--compare-data".into());
    }
    if let Some(t) = time {
        args.push("--time".into());
        args.push(t.to_string());
    }
    args.push(target.to_string());
    args.push(local_path.to_string());
    let run = run_duplicity(&args, credentials, on_stderr_line)?;
    let report = crate::verify::parse(&run.stdout)?;
    Ok(VerifyOutcome {
        success: run.success(),
        report,
        stderr: run.stderr,
    })
}

/// Run a selective restore:
/// `duplicity restore [--path-to-restore P] [--time T] <target> <dest>`.
/// (`--path-to-restore` was called `--file-to-restore` before duplicity 2.0.)
/// Returns the raw run (restore output is progress-style; Phase 1 will
/// stream it). The destination should be a fresh, empty directory.
pub fn restore(
    target: &str,
    path_to_restore: Option<&str>,
    time: Option<&str>,
    dest: &str,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<DuplicityRun> {
    let mut args: Vec<String> = vec!["restore".into()];
    if let Some(p) = path_to_restore {
        args.push("--path-to-restore".into());
        args.push(p.to_string());
    }
    if let Some(t) = time {
        args.push("--time".into());
        args.push(t.to_string());
    }
    args.push(target.to_string());
    args.push(dest.to_string());
    run_duplicity(&args, credentials, on_stderr_line)
}

/// Outcome of a restore performed "into" a destination directory.
#[derive(Debug)]
pub struct RestoreOutcome {
    pub success: bool,
    /// True when the restore was not attempted/completed because the
    /// destination path already exists (duplicity refuses to overwrite).
    pub refused_existing: bool,
    pub final_dest: String,
    pub stderr: String,
}

fn dest_refusal(out: &str) -> bool {
    out.contains("Will not overwrite") || out.contains("is not empty or is current_dir")
}

/// Restore a path (or the whole snapshot when `path` is `None`) *into*
/// `dest_dir`, preserving the item's own name: restoring `sub/c.txt` into
/// `/tmp/x` yields `/tmp/x/c.txt`.
///
/// Safety behaviour (verified empirically against duplicity 3.0.x):
/// - an existing destination FILE is never overwritten — duplicity refuses
///   ("Will not overwrite"); we pre-check and refuse even earlier;
/// - a directory restore merges into an existing directory without touching
///   unrelated content.
pub fn restore_into_dir(
    target: &str,
    path: Option<&str>,
    time: Option<&str>,
    dest_dir: &str,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<RestoreOutcome> {
    let dest = std::path::Path::new(dest_dir);
    if !dest.is_dir() {
        std::fs::create_dir_all(dest)
            .with_context(|| format!("creating destination directory {dest_dir}"))?;
    }

    let final_dest = match path {
        Some(p) => {
            let base = std::path::Path::new(p)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .with_context(|| format!("no file name in restore path {p:?}"))?;
            dest.join(base)
        }
        None => dest.to_path_buf(),
    };

    // Pre-check: duplicity refuses to overwrite; fail fast and clearly.
    if final_dest.exists() {
        return Ok(RestoreOutcome {
            success: false,
            refused_existing: true,
            final_dest: final_dest.display().to_string(),
            stderr: format!(
                "destination {} already exists — duplicity never overwrites; \
                 choose a different destination or remove the existing item first",
                final_dest.display()
            ),
        });
    }
    // A full (no --path-to-restore) restore needs an empty destination.
    if path.is_none() {
        let empty = std::fs::read_dir(dest)?.next().is_none();
        if !empty {
            return Ok(RestoreOutcome {
                success: false,
                refused_existing: true,
                final_dest: final_dest.display().to_string(),
                stderr: "full restore needs an empty destination directory".into(),
            });
        }
    }

    let run = restore(
        target,
        path,
        time,
        &final_dest.display().to_string(),
        credentials,
        on_stderr_line,
    )?;
    let out = format!("{}{}", run.stdout, run.stderr);
    Ok(RestoreOutcome {
        success: run.success(),
        refused_existing: !run.success() && dest_refusal(&out),
        final_dest: final_dest.display().to_string(),
        stderr: run.stderr,
    })
}

/// Run `duplicity remove-older-than <retention> [--force] <target>`.
/// Without `force` this is a dry run listing what would be removed.
pub fn remove_older_than(
    target: &str,
    retention: &str,
    force: bool,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<DuplicityRun> {
    let mut args = vec!["remove-older-than".to_string(), retention.to_string()];
    if force {
        args.push("--force".into());
    }
    args.push(target.to_string());
    run_duplicity(&args, credentials, on_stderr_line)
}

/// Run `duplicity cleanup [--force] <target>` — removes orphaned and
/// incomplete backup sets.
pub fn cleanup(
    target: &str,
    force: bool,
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<DuplicityRun> {
    let mut args = vec!["cleanup".to_string()];
    if force {
        args.push("--force".into());
    }
    args.push(target.to_string());
    run_duplicity(&args, credentials, on_stderr_line)
}

/// Low-level runner: spawn duplicity with `args`, capture output, inject
/// credentials via environment only.
pub fn run_duplicity(
    args: &[String],
    credentials: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<DuplicityRun> {
    let duplicity = duplicity_path()?;
    let mut cmd = Command::new(&duplicity);
    let mut full_args: Vec<String> = credentials.extra_args.clone();
    full_args.extend(args.iter().cloned());
    cmd.args(&full_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Duplicity 3.x quirk (observed): even for --no-encryption targets, read
    // actions attempt a GPG passphrase prompt, which crashes any
    // non-interactive process. Always supplying PASSPHRASE prevents that;
    // for genuinely encrypted targets a placeholder simply fails cleanly
    // with a GPG error instead of hanging.
    cmd.env(
        "PASSPHRASE",
        credentials.passphrase.unwrap_or("duplicity-gui-placeholder"),
    );
    // duplicity's temp workspace defaults into /tmp — which on modern
    // Ubuntu is tmpfs (wiped at every shutdown/reboot, killing mid-run
    // backups with FileNotFoundError ENOENT). Give it a persistent,
    // user-owned workspace instead: never tmpfiles-cleaned, survives
    // reboots and logouts.
    let tmp_root = {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
                home.join(".cache")
            });
        base.join("gui-for-duplicity").join("tmp")
    };
    if let Err(e) = std::fs::create_dir_all(&tmp_root) {
        eprintln!("[runner] warning: could not create persistent tempdir {tmp_root:?}: {e} — falling back to /tmp");
    } else {
        cmd.env("TMPDIR", &tmp_root);
    }

    if let Some(pass) = credentials.backend_password {
        cmd.env("BACKEND_PASSWORD", pass);
    }
    for (k, v) in &credentials.extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", duplicity.display()))?;

    // Read stdout/stderr concurrently to avoid pipe deadlock on large output.
    // stderr is streamed line-by-line through a channel so a listener (the
    // Phase 1 live log pane; the CLI's --live flag) sees messages in real time.
    use std::io::{BufRead, Read};
    let mut stdout_handle = child.stdout.take().expect("stdout piped");
    let stderr_handle = child.stderr.take().expect("stderr piped");
    let stdout = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_handle.read_to_string(&mut buf);
        buf
    });
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let stderr_reader = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr_handle);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stderr_buf = String::new();
    while let Ok(line) = rx.recv() {
        if let Some(cb) = on_stderr_line {
            cb(&line);
        }
        stderr_buf.push_str(&line);
        stderr_buf.push('\n');
    }
    let _ = stderr_reader.join();
    let stdout = stdout.join().unwrap_or_default();

    let exit_status = child.wait()?;

    Ok(DuplicityRun { exit_status, stdout, stderr: stderr_buf })
}
