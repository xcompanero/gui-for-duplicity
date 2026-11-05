//! Parsing of `duplicity list-current-files` output.
//!
//! Output format (verified against duplicity 3.0.x, see
//! `fixtures/list-current-files-real.txt`):
//!
//! ```text
//! Local and Remote metadata are synchronized, no sync needed.
//! Last full backup date: Thu Nov  5 12:44:24 2026
//! Thu Nov  5 12:44:18 2026 .
//! Thu Nov  5 12:44:24 2026 a.txt
//! Thu Nov  5 12:44:24 2026 sub
//! Thu Nov  5 12:44:18 2026 sub/b.txt
//! ```
//!
//! One line per entry: a timestamp, then the path (relative to the backup
//! root, `.` for the root itself). Notably, duplicity does *not* mark which
//! entries are directories — that must be inferred from which paths are
//! prefixes of other paths (see [`FileListing::directories`]).

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use std::collections::BTreeSet;
use std::path::PathBuf;

const KNOWN_INFO_PREFIXES: &[&str] = &[
    "Local and Remote metadata are synchronized",
    "Last full backup date:",
    "No encryption keys configured.",
    "No environment variables are set, asking user.",
    "Skipping passphrase request",
    "Collection Status",
];

/// One entry in a snapshot's file listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Path relative to the backup root.
    pub path: PathBuf,
    pub mtime: DateTime<Local>,
}

/// The parsed result of a `list-current-files` run.
#[derive(Debug, Clone, Default)]
pub struct FileListing {
    pub entries: Vec<FileEntry>,
    /// Lines the parser did not recognise, kept for debugging.
    pub unparsed: Vec<String>,
}

impl FileListing {
    /// Paths that have at least one child entry — i.e. the directories
    /// (duplicity's listing does not distinguish them itself). The root
    /// entry `.` counts as a directory.
    pub fn directories(&self) -> BTreeSet<PathBuf> {
        let mut dirs = BTreeSet::new();
        if self.entries.iter().any(|e| e.path == std::path::Path::new(".")) {
            dirs.insert(PathBuf::from("."));
        }
        for entry in &self.entries {
            let mut parent = entry.path.parent();
            while let Some(p) = parent {
                if !dirs.insert(p.to_path_buf()) {
                    break; // already present, ancestors are too
                }
                parent = p.parent();
            }
        }
        dirs
    }

    pub fn is_directory(&self, path: &std::path::Path) -> bool {
        self.directories().contains(path)
    }

    /// Direct children (files and directories) of `dir`.
    /// The root may be addressed as either `.` or the empty path.
    pub fn children(&self, dir: &std::path::Path) -> Vec<&FileEntry> {
        let at_root = dir == std::path::Path::new(".") || dir.as_os_str().is_empty();
        self.entries
            .iter()
            .filter(|e| e.path != std::path::Path::new("."))
            .filter(|e| {
                let parent = e.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                if at_root {
                    parent.as_os_str().is_empty()
                } else {
                    parent == dir
                }
            })
            .collect()
    }

    /// True if `path` (or anything beneath it) is present in the listing.
    pub fn contains_prefix(&self, path: &std::path::Path) -> bool {
        self.entries.iter().any(|e| e.path.starts_with(path))
    }
}

/// Parse the complete stdout of `duplicity list-current-files`.
pub fn parse(text: &str) -> Result<FileListing> {
    let mut listing = FileListing::default();

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if KNOWN_INFO_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            continue;
        }

        // Entry line: "Thu Nov  5 12:44:18 2026 <path>"
        // Date is exactly 5 whitespace-separated tokens; the path is the rest
        // (paths may themselves contain spaces).
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() >= 6 {
            if let Ok(mtime) = parse_date(&tokens[..5].join(" ")) {
                let path = tokens[5..].join(" ");
                listing.entries.push(FileEntry {
                    path: PathBuf::from(path),
                    mtime,
                });
                continue;
            }
        }
        listing.unparsed.push(trimmed.to_string());
    }
    Ok(listing)
}

/// Parse a duplicity timestamp like `Thu Nov  5 12:44:18 2026`.
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

    #[test]
    fn parses_real_fixture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/list-current-files-real.txt"
        ))
        .expect("fixture must exist");
        let listing = parse(&text).unwrap();
        assert!(listing.unparsed.is_empty(), "unparsed: {:?}", listing.unparsed);
        assert_eq!(listing.entries.len(), 5); // ".", a.txt, sub, sub/b.txt, sub/c.txt

        let dirs = listing.directories();
        assert!(dirs.contains(std::path::Path::new("sub")));
        assert!(dirs.contains(std::path::Path::new(".")));
        assert!(!listing.is_directory(std::path::Path::new("a.txt")));

        // Children of the root: everything at depth 1.
        let root = listing.children(std::path::Path::new("."));
        let names: Vec<String> = root
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        assert_eq!(names, vec!["a.txt", "sub"]);

        // Children of sub/
        let sub = listing.children(std::path::Path::new("sub"));
        let names: Vec<String> = sub
            .iter()
            .map(|e| e.path.display().to_string())
            .collect();
        assert_eq!(names, vec!["sub/b.txt", "sub/c.txt"]);

        assert!(listing.contains_prefix(std::path::Path::new("sub/b.txt")));
    }

    #[test]
    fn tolerant_of_garbage() {
        let listing = parse("nonsense line\n").unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(listing.unparsed.len(), 1);
    }
}
