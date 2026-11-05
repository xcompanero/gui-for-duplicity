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
) -> Result<crate::collection::CollectionReport> {
    let run = run_duplicity(
        &["collection-status".to_string(), target.to_string()],
        credentials,
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
) -> Result<crate::files::FileListing> {
    let mut args = vec!["list-current-files".to_string()];
    if let Some(t) = time {
        args.push("--time".into());
        args.push(t.to_string());
    }
    args.push(target.to_string());
    let run = run_duplicity(&args, credentials)?;
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
pub fn verify(
    target: &str,
    local_path: &str,
    compare_data: bool,
    time: Option<&str>,
    credentials: &Credentials<'_>,
) -> Result<VerifyOutcome> {
    let mut args: Vec<String> = vec!["verify".into()];
    if compare_data {
        args.push("--compare-data".into());
    }
    if let Some(t) = time {
        args.push("--time".into());
        args.push(t.to_string());
    }
    args.push(target.to_string());
    args.push(local_path.to_string());
    let run = run_duplicity(&args, credentials)?;
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
    run_duplicity(&args, credentials)
}

/// Low-level runner: spawn duplicity with `args`, capture output, inject
/// credentials via environment only.
pub fn run_duplicity(args: &[String], credentials: &Credentials<'_>) -> Result<DuplicityRun> {
    let duplicity = duplicity_path()?;
    let mut cmd = Command::new(&duplicity);
    cmd.args(args)
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
    use std::io::Read;
    let mut stdout_handle = child.stdout.take().expect("stdout piped");
    let mut stderr_handle = child.stderr.take().expect("stderr piped");
    let stdout = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_handle.read_to_string(&mut buf);
        buf
    });
    let stderr = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_handle.read_to_string(&mut buf);
        buf
    });
    let exit_status = child.wait()?;
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    Ok(DuplicityRun { exit_status, stdout, stderr })
}
