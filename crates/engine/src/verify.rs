//! Parsing of `duplicity verify` output.
//!
//! Output format (verified against duplicity 3.0.x):
//!
//! ```text
//! Local and Remote metadata are synchronized, no sync needed.
//! Last full backup date: Thu Nov  5 12:44:24 2026
//! Verify complete: 5 file(s) compared, 0 difference(s) found.
//! ```
//!
//! With `--compare-data` and differences present, each difference is listed:
//!
//! ```text
//! Difference found: File sub/b.txt has mtime Thu Nov  5 12:44:48 2026, expected Thu Nov  5 12:44:18 2026
//! Difference found: New file sub/c.txt
//! Verify complete: 5 file(s) compared, 4 difference(s) found.
//! ```
//!
//! Important: a non-zero exit status means *differences were found* — it is
//! an expected, non-crash outcome and the GUI must present it as a
//! verification report, not an error.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

const KNOWN_INFO_PREFIXES: &[&str] = &[
    "Local and Remote metadata are synchronized",
    "Last full backup date:",
    "No encryption keys configured.",
    "No environment variables are set, asking user.",
    "Skipping passphrase request",
];

/// The parsed result of a `duplicity verify` run.
#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// Files compared, from the "Verify complete:" summary line.
    pub files_compared: Option<u64>,
    /// Differences found, from the summary line.
    pub differences: Option<u64>,
    /// Each "Difference found: ..." line, verbatim.
    pub difference_lines: Vec<String>,
    /// Lines the parser did not recognise, kept for debugging.
    pub unparsed: Vec<String>,
}

impl VerifyReport {
    /// True when the summary says zero differences. `None` if no summary
    /// line was found (the run probably failed before completing).
    pub fn is_clean(&self) -> Option<bool> {
        self.differences.map(|d| d == 0)
    }
}

/// Parse the complete stdout of `duplicity verify`.
pub fn parse(text: &str) -> Result<VerifyReport> {
    let mut report = VerifyReport::default();

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if KNOWN_INFO_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("Difference found:") {
            report.difference_lines.push(rest.trim().to_string());
            continue;
        }

        // "Verify complete: 5 file(s) compared, 0 difference(s) found."
        if let Some(rest) = trimmed.strip_prefix("Verify complete:") {
            let (files, diffs) = parse_summary(rest)
                .with_context(|| format!("parsing verify summary line {trimmed:?}"))?;
            report.files_compared = Some(files);
            report.differences = Some(diffs);
            continue;
        }

        report.unparsed.push(trimmed.to_string());
    }
    Ok(report)
}

fn parse_summary(rest: &str) -> Result<(u64, u64)> {
    // Tokens: "5 file(s) compared, 0 difference(s) found."
    let mut numbers = Vec::new();
    for token in rest.split_whitespace() {
        let digits: String = token.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            numbers.push(
                digits
                    .parse::<u64>()
                    .with_context(|| format!("parsing number in {rest:?}"))?,
            );
        }
    }
    if numbers.len() != 2 {
        anyhow::bail!("summary line did not contain exactly two numbers: {rest:?}");
    }
    Ok((numbers[0], numbers[1]))
}

/// Parse a duplicity timestamp like `Thu Nov  5 12:44:18 2026`.
#[allow(dead_code)]
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

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../../fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap_or_else(|e| panic!("fixture {name} must exist: {e}"))
    }

    #[test]
    fn parses_clean_verify() {
        let report = parse(&fixture("verify-real.txt")).unwrap();
        assert!(report.unparsed.is_empty(), "unparsed: {:?}", report.unparsed);
        assert_eq!(report.files_compared, Some(5));
        assert_eq!(report.differences, Some(0));
        assert_eq!(report.is_clean(), Some(true));
        assert!(report.difference_lines.is_empty());
    }

    #[test]
    fn parses_verify_with_differences() {
        let report = parse(&fixture("verify-real-differences.txt")).unwrap();
        assert!(report.unparsed.is_empty(), "unparsed: {:?}", report.unparsed);
        assert_eq!(report.files_compared, Some(6));
        assert_eq!(report.differences, Some(3));
        assert_eq!(report.is_clean(), Some(false));
        assert_eq!(report.difference_lines.len(), 3);
        assert!(report.difference_lines.iter().any(|l| l.contains("b.txt")));
        assert!(report.difference_lines.iter().any(|l| l.contains("New file d.txt")));
    }

    #[test]
    fn tolerant_of_garbage() {
        let report = parse("nonsense\n").unwrap();
        assert_eq!(report.differences, None);
        assert_eq!(report.is_clean(), None);
    }
}
