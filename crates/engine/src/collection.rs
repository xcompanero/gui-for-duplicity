//! Parsing of `duplicity collection-status` output.
//!
//! `collection-status` asks a backup destination "what backups do you have?"
//! and prints a human-readable report. This module turns that text into
//! structured data for the GUI.
//!
//! Two output formats are supported:
//!
//! 1. **Duplicity 3.x** (verified against real output, see
//!    `fixtures/collection-status-real-2.txt`): chains are introduced by
//!    `Secondary chain N of M:` or `Found primary backup chain with matching
//!    signature chain:`, followed by `Chain start time:`, `Chain end time:`,
//!    set/volume counts, and a table of `Full`/`Incremental` rows.
//!
//! 2. **Legacy** (older duplicity releases): `Chain: Backup chain (full: ...)`
//!    followed by `Full:` / `Inc:` lines.
//!
//! The parser is deliberately tolerant: lines it doesn't recognise are kept
//! in [`CollectionReport::unparsed`] rather than rejected, because output
//! varies across versions and backend types. Real-world fixtures drive the
//! unit tests.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// One volume-set in a backup chain: a full backup or an incremental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SetKind {
    Full,
    Inc,
}

impl std::fmt::Display for SetKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetKind::Full => write!(f, "Full"),
            SetKind::Inc => write!(f, "Inc"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupSet {
    pub kind: SetKind,
    pub time: DateTime<Local>,
    /// Number of volumes in this set, when the report supplies it
    /// (duplicity 3.x table format).
    pub volumes: Option<u64>,
}

impl std::fmt::Display for BackupSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.time)?;
        if let Some(v) = self.volumes {
            write!(f, " ({v} volumes)")?;
        }
        Ok(())
    }
}

/// Whether a chain is the *primary* chain (the active one whose signature
/// chain matches — i.e. the one duplicity will extend on the next backup)
/// or a *secondary* chain (an older full and its incrementals, kept by
/// retention policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ChainKind {
    Primary,
    Secondary,
}

impl std::fmt::Display for ChainKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainKind::Primary => write!(f, "primary"),
            ChainKind::Secondary => write!(f, "secondary"),
        }
    }
}

/// A chain: one full backup plus zero or more incrementals on top of it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Chain {
    pub kind: ChainKind,
    /// When the chain's full backup was taken (also its first set).
    pub start_time: Option<DateTime<Local>>,
    /// When the chain ends (last incremental), as stated by duplicity.
    pub end_time: Option<DateTime<Local>>,
    /// "Number of contained backup sets", as stated.
    pub num_backup_sets: Option<u64>,
    /// "Total number of contained volumes", as stated.
    pub num_volumes: Option<u64>,
    /// The individual backup sets, oldest first.
    pub sets: Vec<BackupSet>,
}

impl Chain {
    fn empty() -> Chain {
        Chain {
            kind: ChainKind::Secondary,
            start_time: None,
            end_time: None,
            num_backup_sets: None,
            num_volumes: None,
            sets: Vec::new(),
        }
    }

    /// Time of the most recent state this chain can restore to
    /// (last incremental, or the full backup if there are none).
    pub fn end_time(&self) -> Option<DateTime<Local>> {
        self.end_time.or_else(|| self.sets.last().map(|s| s.time))
    }

    /// The chain's full backup, if one was recorded.
    pub fn full(&self) -> Option<&BackupSet> {
        self.sets.iter().find(|s| s.kind == SetKind::Full)
    }
}

/// The parsed result of a `collection-status` run.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CollectionReport {
    /// "Last full backup date" from the header; `None` if it printed `none`.
    pub last_full_backup: Option<DateTime<Local>>,
    /// All backup chains found at the destination.
    pub chains: Vec<Chain>,
    /// Orphaned or incomplete sets mentioned in the report (raw descriptions).
    pub orphans: Vec<String>,
    /// Lines the parser did not recognise — kept for debugging and for
    /// extending the parser against new fixtures.
    pub unparsed: Vec<String>,
}

/// Informational lines duplicity 3.x emits around the report that carry no
/// structural meaning. Recognising them keeps `unparsed` clean so that
/// genuinely unknown lines stand out.
const KNOWN_INFO_PREFIXES: &[&str] = &[
    "Collection Status",
    "Archive directory:",
    "Connecting with backend:",
    "No encryption keys configured.",
    "Skipping passphrase request",
    "No backup chains with active signatures found",
    "No orphaned or incomplete backup sets found.",
    "No orphaned or incomplete backup chains found.",
    "Type of backup set:",
    "LFTP version is",
];

/// Parse the complete stdout of `duplicity collection-status`.
pub fn parse(text: &str) -> Result<CollectionReport> {
    let mut report = CollectionReport::default();
    let mut current: Option<Chain> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("---") {
            continue;
        }
        if KNOWN_INFO_PREFIXES.iter().any(|p| line.starts_with(p)) {
            continue;
        }
        // "Found 0 secondary backup chain(s)." — count lines end in "chain(s)."
        // (must not swallow "Found primary backup chain with matching
        // signature chain:", which starts a chain!)
        if line.starts_with("Found ") && line.ends_with("chain(s).") {
            continue;
        }

        // Header line: "Last full backup date: Sat Nov  1 04:00:00 2025"
        if let Some(rest) = line.strip_prefix("Last full backup date:") {
            let rest = rest.trim();
            report.last_full_backup = if rest.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(parse_date(rest).with_context(|| {
                    format!("parsing 'Last full backup date' value: {rest:?}")
                })?)
            };
            continue;
        }

        // duplicity 3.x: "Secondary chain 1 of 2:"
        if line.starts_with("Secondary chain") {
            flush_chain(&mut report, &mut current);
            current = Some(Chain {
                kind: ChainKind::Secondary,
                ..Chain::empty()
            });
            continue;
        }

        // duplicity 3.x: "Found primary backup chain with matching signature chain:"
        if line.starts_with("Found primary backup chain") {
            flush_chain(&mut report, &mut current);
            current = Some(Chain {
                kind: ChainKind::Primary,
                ..Chain::empty()
            });
            continue;
        }

        // duplicity 3.x per-chain fields
        if let Some(rest) = line.strip_prefix("Chain start time:") {
            set_time(current.as_mut(), "start", rest.trim(), |c, t| c.start_time = Some(t))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Chain end time:") {
            set_time(current.as_mut(), "end", rest.trim(), |c, t| c.end_time = Some(t))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Number of contained backup sets:") {
            if let Some(chain) = current.as_mut() {
                chain.num_backup_sets = rest.trim().parse().ok();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Total number of contained volumes:") {
            if let Some(chain) = current.as_mut() {
                chain.num_volumes = rest.trim().parse().ok();
            }
            continue;
        }

        // duplicity 3.x table rows, e.g.
        // "                Full         Thu Apr  2 09:05:52 2026              1149"
        // "         Incremental         Fri Apr  3 09:14:07 2026                 1"
        if line.starts_with("Incremental ") {
            push_table_row(current.as_mut(), SetKind::Inc, line)?;
            continue;
        }
        if line.starts_with("Full ") {
            push_table_row(current.as_mut(), SetKind::Full, line)?;
            continue;
        }

        // Legacy format: "    Chain: Backup chain (full: Sat Nov  1 04:00:00 2025)"
        if let Some(rest) = line.strip_prefix("Chain:") {
            flush_chain(&mut report, &mut current);
            let rest = rest.trim();
            let full_time = extract_chain_full_date(rest)?;
            current = Some(Chain {
                kind: ChainKind::Primary,
                start_time: Some(full_time),
                sets: vec![BackupSet { kind: SetKind::Full, time: full_time, volumes: None }],
                ..Chain::empty()
            });
            continue;
        }

        // Legacy set lines: "         Full: Sat Nov  1 04:00:00 2025"
        if let Some(rest) = line.strip_prefix("Full:") {
            push_set(current.as_mut(), SetKind::Full, rest.trim())?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Inc:") {
            push_set(current.as_mut(), SetKind::Inc, rest.trim())?;
            continue;
        }

        if line.contains("Orphaned set") || line.contains("Incomplete set") {
            report.orphans.push(line.to_string());
            continue;
        }

        // Everything else is retained for inspection.
        report.unparsed.push(line.to_string());
    }

    flush_chain(&mut report, &mut current);
    Ok(report)
}

fn flush_chain(report: &mut CollectionReport, current: &mut Option<Chain>) {
    if let Some(chain) = current.take() {
        report.chains.push(chain);
    }
}

fn set_time(
    chain: Option<&mut Chain>,
    which: &str,
    value: &str,
    setter: impl FnOnce(&mut Chain, DateTime<Local>),
) -> Result<()> {
    let chain = chain.context(format!(
        "'Chain {which} time:' line appeared before any chain header"
    ))?;
    let time = parse_date(value)
        .with_context(|| format!("parsing chain {which} time {value:?}"))?;
    setter(chain, time);
    Ok(())
}

/// Parse a duplicity 3.x table row: kind, then date, then volume count.
/// `line` is already trimmed and starts with "Full " or "Incremental ".
fn push_table_row(chain: Option<&mut Chain>, kind: SetKind, line: &str) -> Result<()> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 7 {
        bail!("table row too short to contain date + volumes: {line:?}");
    }
    // Last token is the volume count; the rest form the date.
    let volumes: u64 = tokens
        .last()
        .context("missing volume count in table row")?
        .parse()
        .with_context(|| format!("parsing volume count in table row {line:?}"))?;
    let date = tokens[1..tokens.len() - 1].join(" ");
    let time = parse_date(&date).with_context(|| format!("parsing table row date {line:?}"))?;
    let chain = chain
        .context("backup-set table row appeared before any chain header")?;
    chain.sets.push(BackupSet { kind, time, volumes: Some(volumes) });
    Ok(())
}

fn push_set(chain: Option<&mut Chain>, kind: SetKind, rest: &str) -> Result<()> {
    let chain = chain
        .context("'Full:' or 'Inc:' line appeared before any 'Chain:' line")?;
    let time = parse_date(rest)
        .with_context(|| format!("parsing set date from {rest:?}"))?;
    // In legacy listings the "Full:" line restates the chain's full set;
    // avoid duplicating the one the Chain: header already gave us.
    if kind == SetKind::Full && chain.sets.iter().any(|s| s.kind == SetKind::Full && s.time == time)
    {
        return Ok(());
    }
    chain.sets.push(BackupSet { kind, time, volumes: None });
    Ok(())
}

/// From a legacy "Chain:" description like
/// `Backup chain (full: Sat Nov  1 04:00:00 2025)` extract the date.
fn extract_chain_full_date(desc: &str) -> Result<DateTime<Local>> {
    let open = desc.find('(').with_context(|| format!("no '(' in chain line {desc:?}"))?;
    let close = desc.rfind(')').with_context(|| format!("no ')' in chain line {desc:?}"))?;
    let inner = &desc[open + 1..close];
    let date = inner
        .strip_prefix("full:")
        .with_context(|| format!("chain line lacks 'full:' prefix: {desc:?}"))?;
    parse_date(date.trim())
}

/// Parse a duplicity timestamp like `Sat Nov  1 04:00:00 2025`.
/// Whitespace is normalised first because duplicity space-pads the day.
fn parse_date(s: &str) -> Result<DateTime<Local>> {
    let normalised = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let naive = NaiveDateTime::parse_from_str(&normalised, "%a %b %e %H:%M:%S %Y")
        .with_context(|| format!("parsing duplicity date {s:?}"))?;
    let parsed = Local
        .from_local_datetime(&naive)
        .single()
        .with_context(|| format!("ambiguous or invalid local time: {s:?}"))?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> String {
        let path = format!("{}/../../fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("fixture {path} must exist: {e}"))
    }

    /// The synthetic legacy-format fixture.
    #[test]
    fn parses_legacy_fixture() {
        let report = parse(&fixture("collection-status-sample.txt")).unwrap();
        assert_eq!(report.chains.len(), 2);

        let first = &report.chains[0];
        assert_eq!(first.kind, ChainKind::Primary);
        assert_eq!(first.full().map(|s| s.time), first.start_time);
        assert_eq!(first.sets.len(), 4); // full + 3 incrementals
        // Sets are in ascending order.
        assert!(first.sets.windows(2).all(|w| w[0].time < w[1].time));
        assert!(first.end_time().is_some());

        let second = &report.chains[1];
        assert_eq!(second.sets.len(), 1, "second chain is full-only");

        assert_eq!(report.last_full_backup, Some(first.start_time.unwrap()));
        assert!(report.orphans.is_empty());
    }

    /// Real-world fixture from the user's FTP NAS target, duplicity 3.0.x.
    #[test]
    fn parses_real_duplicity3_fixture() {
        let report = parse(&fixture("collection-status-real-2.txt")).unwrap();

        // Three chains: two secondary, one primary.
        assert_eq!(report.chains.len(), 3);
        assert!(report.unparsed.is_empty(), "unparsed: {:?}", report.unparsed);
        assert!(report.orphans.is_empty());

        let secondary1 = &report.chains[0];
        assert_eq!(secondary1.kind, ChainKind::Secondary);
        assert_eq!(secondary1.num_backup_sets, Some(78));
        assert_eq!(secondary1.num_volumes, Some(1270));
        assert_eq!(secondary1.sets.len(), 78);
        assert_eq!(secondary1.full().map(|s| s.volumes), Some(Some(1149)));
        // Every set parsed with a volume count.
        assert!(secondary1.sets.iter().all(|s| s.volumes.is_some()));

        let secondary2 = &report.chains[1];
        assert_eq!(secondary2.kind, ChainKind::Secondary);
        assert_eq!(secondary2.num_backup_sets, Some(79));
        assert_eq!(secondary2.sets.len(), 79);

        let primary = &report.chains[2];
        assert_eq!(primary.kind, ChainKind::Primary);
        assert_eq!(primary.num_backup_sets, Some(18));
        assert_eq!(primary.num_volumes, Some(1199));
        assert_eq!(primary.sets.len(), 18);

        // Dates parsed correctly, ascending within each chain.
        for chain in &report.chains {
            assert!(chain.sets.windows(2).all(|w| w[0].time < w[1].time));
            assert_eq!(chain.end_time(), Some(chain.sets.last().unwrap().time));
        }
        assert_eq!(
            report.last_full_backup,
            primary.full().map(|s| s.time)
        );
    }

    #[test]
    fn tolerant_of_garbage() {
        let report = parse("hello\nworld\n").unwrap();
        assert!(report.chains.is_empty());
        assert_eq!(report.unparsed.len(), 2);
    }

    #[test]
    fn rejects_malformed_dates() {
        assert!(parse("Chain: Backup chain (full: not-a-date)").is_err());
    }
}
