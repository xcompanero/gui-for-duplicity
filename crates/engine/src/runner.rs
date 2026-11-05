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

/// Run `duplicity collection-status <target>` and parse its output.
///
/// `passphrase` (if the target is passphrase-encrypted and gpg-agent is not
/// caching the key) is injected into the child's environment only.
pub fn collection_status(
    target: &str,
    passphrase: Option<&str>,
    extra_env: &[(&str, &str)],
) -> Result<crate::collection::CollectionReport> {
    let run = run_duplicity(
        &["collection-status".to_string(), target.to_string()],
        passphrase,
        extra_env,
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

/// Low-level runner: spawn duplicity with `args`, capture output, inject
/// optional passphrase via environment.
pub fn run_duplicity(
    args: &[String],
    passphrase: Option<&str>,
    extra_env: &[(&str, &str)],
) -> Result<DuplicityRun> {
    let duplicity = duplicity_path()?;
    let mut cmd = Command::new(&duplicity);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // stdin is null, so an interactive passphrase prompt would fail fast
    // rather than hang — pass one via `passphrase` or let gpg-agent supply it.
    if let Some(pass) = passphrase {
        cmd.env("PASSPHRASE", pass);
    } else {
        cmd.env_remove("PASSPHRASE");
    }
    for (k, v) in extra_env {
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
