//! A cache of *parsed* `list-current-files` results.
//!
//! Discovered the hard way: duplicity caches the raw signature archives
//! (~1.7 GB for a mid-sized home backup) but re-parses them into a file
//! index on **every** `list-current-files` invocation — minutes of Python
//! CPU each time. This module caches the parsed [`FileListing`] locally,
//! keyed by the target's current chain state so it invalidates itself
//! whenever a new backup lands.
//!
//! Cache location: `~/.cache/gui-for-duplicity/listings/<hash>.json`
//! (XDG_CACHE_HOME aware), where `<hash>` covers the target URL and the
//! requested time.
//!
//! Only default-time ("now") listings are cached; historical `--time`
//! listings bypass the cache.

use crate::files::FileListing;
use crate::runner::{self, Credentials};
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedListing {
    /// Chain end time at the moment the listing was captured.
    chain_end_time: Option<DateTime<Local>>,
    cached_at: DateTime<Utc>,
    listing: FileListing,
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            home.join(".cache")
        });
    base.join("gui-for-duplicity").join("listings")
}

fn cache_key(target: &str, time: Option<&str>) -> String {
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    time.unwrap_or("now").hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn cache_path(target: &str, time: Option<&str>) -> PathBuf {
    cache_dir().join(format!("{}.json", cache_key(target, time)))
}

/// The latest chain end time at the target — the invalidation key.
/// A cheap `collection-status` (manifests only) provides it.
fn current_chain_end(target: &str, creds: &Credentials<'_>) -> Result<Option<DateTime<Local>>> {
    let report = runner::collection_status(target, creds, None)?;
    Ok(report
        .chains
        .iter()
        .filter_map(|c| c.end_time())
        .max())
}

/// Get the file listing for `target`, from cache when the chain hasn't
/// changed, otherwise from duplicity (then cached). Returns the listing and
/// whether it came from cache.
pub fn get_or_fetch(
    target: &str,
    time: Option<&str>,
    creds: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<(FileListing, bool)> {
    if time.is_some() {
        // Historical listings are not cached (each --time is a different view).
        let listing = runner::list_current_files(target, time, creds, on_stderr_line)?;
        return Ok((listing, false));
    }

    let path = cache_path(target, time);

    // Fast path: cheap status check, then compare with the cached state.
    let now_end = current_chain_end(target, creds)?;
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(cached) = serde_json::from_str::<CachedListing>(&text) {
            if cached.chain_end_time == now_end {
                return Ok((cached.listing, true));
            }
        }
    }

    // Slow path: real duplicity run, then cache the parse.
    let listing = runner::list_current_files(target, time, creds, on_stderr_line)?;
    let entry = CachedListing {
        chain_end_time: now_end,
        cached_at: Utc::now(),
        listing: listing.clone(),
    };
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&entry)?)
        .with_context(|| format!("writing listing cache {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok((listing, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_distinguishes_inputs() {
        let a1 = cache_key("sftp://x/y", None);
        let a2 = cache_key("sftp://x/y", None);
        assert_eq!(a1, a2, "same input must give the same key across runs");
        assert_ne!(cache_key("sftp://x/y", None), cache_key("sftp://x/z", None));
        assert_ne!(cache_key("sftp://x/y", None), cache_key("sftp://x/y", Some("2026-01-01")));
    }

    #[test]
    fn cache_roundtrip() {
        // Use a temp override via env is not possible without process-global
        // state; instead exercise serialisation directly.
        let listing = FileListing {
            entries: vec![crate::files::FileEntry {
                path: "a.txt".into(),
                mtime: Local::now(),
            }],
            unparsed: vec![],
        };
        let entry = CachedListing {
            chain_end_time: Some(Local::now()),
            cached_at: Utc::now(),
            listing: listing.clone(),
        };
        let text = serde_json::to_string(&entry).unwrap();
        let back: CachedListing = serde_json::from_str(&text).unwrap();
        assert_eq!(back.listing.entries.len(), 1);
        assert_eq!(back.listing.entries[0].path, listing.entries[0].path);
    }
}
