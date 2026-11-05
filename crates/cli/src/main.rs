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
        "status" => cmd_status(args.collect()),
        _ => {
            bail!("usage: engine-cli status <target-url> [--passphrase <secret>]\n       engine-cli version");
        }
    }
}

fn cmd_status(args: Vec<String>) -> Result<()> {
    let mut target: Option<String> = None;
    let mut passphrase: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--passphrase" => passphrase = Some(it.next().context("--passphrase needs a value")?),
            _ => {
                if target.is_some() {
                    bail!("unexpected extra argument: {arg}");
                }
                target = Some(arg);
            }
        }
    }
    let target = target.context("no target URL given (e.g. file:///mnt/backup/dup)")?;

    let report = engine::runner::collection_status(&target, passphrase.as_deref(), &[])?;

    println!("Target:            {target}");
    match report.last_full_backup {
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
            "Chain {}: full at {}, {} incremental(s), ends at {}",
            i + 1,
            chain.full.time,
            chain.incrementals.len(),
            chain.end_time()
        );
        for inc in &chain.incrementals {
            println!("    Inc: {inc}");
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
