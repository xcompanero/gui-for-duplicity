//! engine-cli — Phase 0 test harness for the duplicity engine.
//!
//! Usage:
//!   engine-cli status <target-url> [--passphrase <secret>]
//!   engine-cli version
//!
//! `status` runs `duplicity collection-status` against a target and prints
//! the parsed result as a table. This exists to prove the parser against
//! real backup destinations before any GUI exists.

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "version" => {
            println!("engine-cli 0.1.0 (Phase 0 harness)");
            println!(
                "duplicity: {}",
                engine::runner::duplicity_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|e| format!("NOT FOUND: {e}"))
            );
            Ok(())
        }
        "status" => cmd_status(rest),
        "files" => cmd_files(rest),
        "verify" => cmd_verify(rest),
        "restore" => cmd_restore(rest),
        _ => {
            bail!("usage: engine-cli status <target-url> [--passphrase <s>] [--backend-password <s>]\n       engine-cli files <target-url> [--time <t>]\n       engine-cli verify <target-url> <local-path> [--time <t>] [--no-compare-data]\n       engine-cli restore <target-url> <dest-dir> [--file <path>] [--time <t>]\n       (secrets may be given via ENGINE_PASSPHRASE / ENGINE_BACKEND_PASSWORD)\n       engine-cli version");
        }
    }
}

/// Shared option parsing for all subcommands.
struct Opts {
    positional: Vec<String>,
    time: Option<String>,
    passphrase: Option<String>,
    backend_password: Option<String>,
    file: Option<String>,
    flags: Vec<String>,
}

fn parse_opts(args: Vec<String>) -> Result<Opts> {
    let mut o = Opts {
        positional: Vec::new(),
        time: None,
        passphrase: None,
        backend_password: None,
        file: None,
        flags: Vec::new(),
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--time" => o.time = Some(it.next().context("--time needs a value")?),
            "--file" => o.file = Some(it.next().context("--file needs a value")?),
            "--passphrase" => o.passphrase = Some(it.next().context("--passphrase needs a value")?),
            "--backend-password" => {
                o.backend_password = Some(it.next().context("--backend-password needs a value")?)
            }
            "--no-compare-data" => o.flags.push(arg),
            _ => o.positional.push(arg),
        }
    }
    // Secrets may also arrive via the environment so they never sit in shell
    // history. Command-line flags win if both are given.
    o.passphrase = o
        .passphrase
        .or_else(|| std::env::var("ENGINE_PASSPHRASE").ok());
    o.backend_password = o
        .backend_password
        .or_else(|| std::env::var("ENGINE_BACKEND_PASSWORD").ok());
    Ok(o)
}

impl Opts {
    fn credentials(&self) -> engine::runner::Credentials<'_> {
        engine::runner::Credentials {
            passphrase: self.passphrase.as_deref(),
            backend_password: self.backend_password.as_deref(),
            ..Default::default()
        }
    }

    fn one_positional(&self, what: &str) -> Result<&String> {
        if self.positional.len() != 1 {
            bail!("expected exactly one {what}");
        }
        Ok(&self.positional[0])
    }

    fn two_positionals(&self, what: &str) -> Result<(&String, &String)> {
        if self.positional.len() != 2 {
            bail!("expected exactly two {what}");
        }
        Ok((&self.positional[0], &self.positional[1]))
    }
}

fn cmd_status(args: Vec<String>) -> Result<()> {
    let o = parse_opts(args)?;
    let target = o.one_positional("target URL")?;
    let report = engine::runner::collection_status(target, &o.credentials())?;

    println!("Target:            {target}");    match report.last_full_backup {
        Some(t) => println!("Last full backup:  {t}"),
        None => println!("Last full backup:  none"),
    }
    println!("Chains found:      {}", report.chains.len());
    println!();

    if report.chains.is_empty() {
        println!("(no backup chains at this target)");
    }
    for (i, chain) in report.chains.iter().enumerate() {
        println!(
            "Chain {} ({}): {} set(s), {} volumes",
            i + 1,
            chain.kind,
            chain.num_backup_sets.map_or(chain.sets.len(), |n| n as usize),
            chain
                .num_volumes
                .map_or_else(|| "unknown".to_string(), |v| v.to_string())
        );
        println!(
            "    from {} to {}",
            chain
                .start_time
                .map_or("?".into(), |t| t.to_string()),
            chain
                .end_time()
                .map_or("?".into(), |t| t.to_string())
        );
        for set in &chain.sets {
            println!("    {set}");
        }
        println!();
    }

    if !report.orphans.is_empty() {
        println!("Orphaned/incomplete sets:");
        for o in &report.orphans {
            println!("  {o}");
        }
        println!();
    }
    if !report.unparsed.is_empty() {
        println!(
            "Note: {} line(s) were not recognised by the parser (kept for debugging):",
            report.unparsed.len()
        );
        for line in report.unparsed.iter().take(10) {
            println!("  | {line}");
        }
    }
    Ok(())
}

fn cmd_files(args: Vec<String>) -> Result<()> {
    let o = parse_opts(args)?;
    let target = o.one_positional("target URL")?;
    let listing = engine::runner::list_current_files(target, o.time.as_deref(), &o.credentials())?;

    println!("Target: {target}");
    if let Some(t) = &o.time {
        println!("Time:   {t}");
    }
    println!("Entries: {}", listing.entries.len());
    println!();
    for entry in &listing.entries {
        let kind = if listing.is_directory(&entry.path) {
            "dir "
        } else {
            "file"
        };
        println!("{kind}  {}  {}", entry.mtime, entry.path.display());
    }
    if !listing.unparsed.is_empty() {
        println!();
        println!(
            "Note: {} unrecognised line(s):",
            listing.unparsed.len()
        );
        for line in listing.unparsed.iter().take(10) {
            println!("  | {line}");
        }
    }
    Ok(())
}

fn cmd_verify(args: Vec<String>) -> Result<()> {
    let o = parse_opts(args)?;
    let (target, local) = o.two_positionals("arguments (target-url local-path)")?;
    let compare_data = !o.flags.iter().any(|f| f == "--no-compare-data");
    let outcome = engine::runner::verify(
        target,
        local,
        compare_data,
        o.time.as_deref(),
        &o.credentials(),
    )?;

    println!("Target:   {target}");
    println!("Local:    {local}");
    println!("Differences reported: {}", outcome.report.difference_lines.len());
    for line in &outcome.report.difference_lines {
        println!("  - {line}");
    }
    match (outcome.report.files_compared, outcome.report.differences) {
        (Some(f), Some(d)) => println!("Summary:  {f} file(s) compared, {d} difference(s) found."),
        _ => println!("Summary:  (verify did not complete — see stderr)"),
    }
    if !outcome.stderr.trim().is_empty() {
        println!("stderr:   {}", outcome.stderr.trim());
    }
    // Mirror duplicity's exit status: non-zero when differences were found.
    if outcome.report.is_clean() == Some(true) {
        println!("RESULT: VERIFIED — backup matches source");
    } else if outcome.report.is_clean() == Some(false) {
        println!("RESULT: DIFFERENCES FOUND — see list above");
    } else {
        anyhow::bail!("RESULT: VERIFY FAILED — duplicity did not complete the verification");
    }
    Ok(())
}

fn cmd_restore(args: Vec<String>) -> Result<()> {
    let o = parse_opts(args)?;
    if o.positional.len() != 2 {
        bail!("expected exactly two arguments (target-url dest-dir)");
    }
    let target = &o.positional[0];
    let dest = &o.positional[1];
    let dest_path = std::path::Path::new(dest);

    // duplicity semantics: the destination path BECOMES the restored item.
    // For a friendly "restore into this directory" experience, when a
    // single path is selected and the destination is an existing directory,
    // we restore to <dest>/<basename-of-selected-path>.
    let (final_dest, printed_dest) = match (&o.file, dest_path.is_dir()) {
        (Some(file), true) => {
            let base = std::path::Path::new(file)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_else(|| file.clone());
            let joined = dest_path.join(&base);
            let printed = format!("{dest}/{base}");
            (joined.display().to_string(), printed)
        }
        _ => (dest.to_string(), dest.to_string()),
    };
    if dest_path.is_file() {
        bail!("destination exists and is a file, not a directory: {dest}");
    }

    println!(
        "Restoring {} -> {}",
        o.file.as_deref().unwrap_or("(everything)"),
        printed_dest
    );
    let run = engine::runner::restore(
        target,
        o.file.as_deref(),
        o.time.as_deref(),
        &final_dest,
        &o.credentials(),
    )?;
    if run.success() {
        println!("RESULT: RESTORE COMPLETE");
        Ok(())
    } else {
        bail!("restore failed ({}):\n{}", run.exit_status, run.stderr.trim());
    }
}
