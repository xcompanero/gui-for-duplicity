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
/// A cheap `collection-status` (manifests only) provides it. NOTE: on slow
/// remote backends this can still take a while — it must never be allowed
/// to block serving a cached listing (see `get_cached`).
pub fn current_chain_end(target: &str, creds: &Credentials<'_>) -> Result<Option<DateTime<Local>>> {
    let report = runner::collection_status(target, creds, None)?;
    Ok(report
        .chains
        .iter()
        .filter_map(|c| c.end_time())
        .max())
}

/// Read a cached listing (default or historical) without any network
/// activity. Returns the listing and the chain-end state it was cached for
/// (None for historical listings — they are immutable).
pub fn get_cached(target: &str, time: Option<&str>) -> Option<(FileListing, Option<DateTime<Local>>)> {
    let path = cache_path(target, time);
    let text = std::fs::read_to_string(&path).ok()?;
    let cached: CachedListing = serde_json::from_str(&text).ok()?;
    Some((cached.listing, cached.chain_end_time))
}

/// Store an externally-parsed listing for `target` (fetching the current
/// chain-end state for the invalidation key). Used by the CLI's `seed-cache`
/// subcommand to turn a captured listing into a warm cache.
pub fn store(target: &str, listing: FileListing) -> Result<()> {
    let now_end = current_chain_end(target, &Credentials::new())?;
    write_entry(target, None, now_end, listing)
}

/// Get the file listing for `target` — from cache when possible, otherwise
/// from duplicity (then cached).
///
/// Two regimes:
/// - `time: None` ("now"): the listing changes whenever a new backup lands,
///   so the cache is validated against the target's current chain-end time
///   (cheap status; run in the background by the GUI).
/// - `time: Some(t)`: the past is immutable — a listing at time *t* never
///   changes, so its cache entry is permanent and needs no freshness check.
pub fn get_or_fetch(
    target: &str,
    time: Option<&str>,
    creds: &Credentials<'_>,
    on_stderr_line: Option<&dyn Fn(&str)>,
) -> Result<(FileListing, bool)> {
    let path = cache_path(target, time);

    if let Some(t) = time {
        // Immutable historical view: cache hit is always valid.
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cached) = serde_json::from_str::<CachedListing>(&text) {
                return Ok((cached.listing, true));
            }
        }
        let listing = runner::list_current_files(target, Some(t), creds, on_stderr_line)?;
        write_entry(target, time, None, listing.clone())?;
        return Ok((listing, false));
    }

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
    let listing = runner::list_current_files(target, None, creds, on_stderr_line)?;
    write_entry(target, None, now_end, listing.clone())?;
    Ok((listing, false))
}

fn write_entry(
    target: &str,
    time: Option<&str>,
    chain_end_time: Option<DateTime<Local>>,
    listing: FileListing,
) -> Result<()> {
    let entry = CachedListing {
        chain_end_time,
        cached_at: Utc::now(),
        listing,
    };
    let path = cache_path(target, time);
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(&entry)?)
        .with_context(|| format!("writing listing cache {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    prune_cache(&dir)?;
    Ok(())
}

/// Eviction guard: keep the listing cache bounded on small-disk machines.
/// Oldest-modified files are evicted once either limit is exceeded. Limits
/// are generous: ~30 listings or ~100 MB total (a magrathea-scale listing
/// is ~7 MB).
const MAX_CACHE_FILES: usize = 30;
const MAX_CACHE_BYTES: u64 = 100 * 1024 * 1024;

fn prune_cache(dir: &std::path::Path) -> Result<()> {
    prune_cache_with_limits(dir, MAX_CACHE_FILES, MAX_CACHE_BYTES)
}

fn prune_cache_with_limits(
    dir: &std::path::Path,
    max_files: usize,
    max_bytes: u64,
) -> Result<()> {
    let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let mut total: u64 = 0;
    for e in std::fs::read_dir(dir)?.flatten() {
        let meta = e.metadata()?;
        if !meta.is_file() || !e.path().extension().is_some_and(|x| x == "json") {
            continue;
        }
        entries.push((e.path(), meta.modified()?, meta.len()));
        total += meta.len();
    }
    if entries.len() <= max_files && total <= max_bytes {
        return Ok(());
    }
    // Oldest first.
    entries.sort_by_key(|(_, mtime, _)| *mtime);
    let mut excess_files = entries.len().saturating_sub(max_files);
    for (path, _, size) in entries {
        if excess_files > 0 {
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(size);
            excess_files -= 1;
        } else if total > max_bytes {
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(size);
        } else {
            break;
        }
    }
    Ok(())
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

    #[test]
    fn prune_evicts_oldest_beyond_file_cap() {
        let dir = std::env::temp_dir().join(format!("gfd-prune-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Five files with ascending mtimes (0 = oldest).
        for i in 0..5 {
            let p = dir.join(format!("{i}.json"));
            let f = std::fs::File::create(&p).unwrap();
            let old = std::time::SystemTime::now()
                - std::time::Duration::from_secs(1000 - i as u64);
            f.set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
            drop(f);
            std::fs::write(&p, "[]").unwrap();
        }
        prune_cache_with_limits(&dir, 3, u64::MAX).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        // The two oldest (0, 1) must be evicted; 2, 3, 4 remain.
        assert_eq!(left, vec!["2.json", "3.json", "4.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
