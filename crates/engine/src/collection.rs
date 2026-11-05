//! Parsing of `duplicity collection-status` output.
//!
//! `collection-status` asks a backup destination "what backups do you have?"
//! and prints a human-readable report: the chains of full + incremental sets
//! with their timestamps. This module turns that text into structured data.
//!
//! The parser is deliberately tolerant: lines it doesn't recognise are kept
//! in [`CollectionReport::unparsed`] rather than rejected, because duplicity's
//! output varies slightly across versions and backend types. Real-world
//! fixtures (see `fixtures/`) drive the unit tests.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// One volume-set in a backup chain: a full backup or an incremental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct BackupSet {
    pub kind: SetKind,
    pub time: DateTime<Local>,
}

impl std::fmt::Display for BackupSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.time)
    }
}

/// A chain: one full backup plus zero or more incrementals on top of it.
#[derive(Debug, Clone)]
pub struct Chain {
    /// The full backup the chain rests on.
    pub full: BackupSet,
    /// Incrementals, oldest first.
    pub incrementals: Vec<BackupSet>,
}

impl Chain {
    /// Time of the most recent state this chain can restore to
    /// (last incremental, or the full backup if there are none).
    pub fn end_time(&self) -> DateTime<Local> {
        self.incrementals
            .last()
            .map(|s| s.time)
            .unwrap_or(self.full.time)
    }

    /// Number of individual backup sets in this chain.
    pub fn len(&self) -> usize {
        1 + self.incrementals.len()
    }
}

/// The parsed result of a `collection-status` run.
#[derive(Debug, Clone, Default)]
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

/// Parse the complete stdout of `duplicity collection-status`.
pub fn parse(text: &str) -> Result<CollectionReport> {
    let mut report = CollectionReport::default();
    let mut current: Option<Chain> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("---") {
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

        // "    Chain: Backup chain (full: Sat Nov  1 04:00:00 2025)"
        if let Some(rest) = line.strip_prefix("Chain:") {
            // Flush the chain we were collecting.
            if let Some(chain) = current.take() {
                report.chains.push(chain);
            }
            let rest = rest.trim();
            let full_time = extract_chain_full_date(rest)?;
            current = Some(Chain {
                full: BackupSet { kind: SetKind::Full, time: full_time },
                incrementals: Vec::new(),
            });
            continue;
        }

        // "         Full: Sat Nov  1 04:00:00 2025"
        // "         Inc:  Sun Nov  2 04:00:00 2025"
        if let Some(rest) = line.strip_prefix("Full:") {
            push_set(current.as_mut(), SetKind::Full, rest)?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("Inc:") {
            push_set(current.as_mut(), SetKind::Inc, rest)?;
            continue;
        }

        if line.contains("Orphaned set") || line.contains("Incomplete set") {
            report.orphans.push(line.to_string());
            continue;
        }

        // Notes like "No orphaned or incomplete backup sets found." or
        // "No backup chains with a signature set found" are informational;
        // everything else is retained for inspection.
        report.unparsed.push(line.to_string());
    }

    if let Some(chain) = current.take() {
        report.chains.push(chain);
    }
    Ok(report)
}

fn push_set(chain: Option<&mut Chain>, kind: SetKind, rest: &str) -> Result<()> {
    let chain = chain
        .context("'Full:' or 'Inc:' line appeared before any 'Chain:' line")?;
    let time = parse_date(rest.trim())
        .with_context(|| format!("parsing set date from {rest:?}"))?;
    if kind == SetKind::Full {
        // Within a chain listing, the "Full:" line restates the chain's full
        // set; keep it authoritative (it should match the Chain: header).
        chain.full = BackupSet { kind, time };
    } else {
        chain.incrementals.push(BackupSet { kind, time });
    }
    Ok(())
}

/// From a "Chain:" description like
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

    /// The synthetic sample fixture; real captured output from the user's
    /// targets is added alongside it as more fixtures land in `fixtures/`.
    fn sample() -> String {
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/collection-status-sample.txt"
        ))
        .expect("fixture file must exist")
    }

    #[test]
    fn parses_sample_fixture() {
        let report = parse(&sample()).unwrap();
        assert_eq!(report.chains.len(), 2);

        let first = &report.chains[0];
        assert_eq!(first.full.kind, SetKind::Full);
        assert_eq!(first.incrementals.len(), 3);
        // Increments are strictly after the full and in ascending order.
        assert!(first.incrementals.windows(2).all(|w| w[0].time < w[1].time));
        assert!(first.incrementals[0].time > first.full.time);
        assert_eq!(first.end_time(), first.incrementals[2].time);

        let second = &report.chains[1];
        assert_eq!(second.incrementals.len(), 0, "second chain is full-only");
        assert_eq!(second.end_time(), second.full.time);

        assert!(report.last_full_backup.is_some());
        assert_eq!(report.last_full_backup, Some(report.chains[0].full.time));
        assert!(report.orphans.is_empty());
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
