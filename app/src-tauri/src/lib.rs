//! GUI For Duplicity — Tauri backend.
//!
//! A thin layer: the engine crate does all real work; these commands mostly
//! translate between the UI and the engine, and stream duplicity's output
//! to the frontend as events while jobs run.

use engine::profile::Profile;
use engine::runner::{self, Credentials};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, State};

/// Shared application state: monotonically increasing job ids.
#[derive(Default)]
struct AppState {
    next_job_id: AtomicU64,
}

/// A job's progress event sent to the frontend.
#[derive(Serialize, Clone)]
struct JobLogLine {
    job_id: u64,
    line: String,
}

#[derive(Serialize, Clone)]
struct JobDone {
    job_id: u64,
    ok: bool,
    exit_code: Option<i32>,
    message: Option<String>,
}

#[derive(Serialize)]
struct ProfileSummary {
    name: String,
    target: String,
    sources: Vec<String>,
    retention: String,
}

fn err_msg(e: anyhow::Error) -> String {
    let text = format!("{e:#}");
    if text.contains("Bad session key") || text.contains("decryption failed") {
        return format!(
            "{text}\n\n\
             This looks like a passphrase problem: the target contains data encrypted \
             with a different (or missing) passphrase. Check the passphrase saved in \
             KWallet for this target, and make sure the whole chain was created with \
             the same encryption setting."
        );
    }
    text
}

/// Build credentials for a target: an explicit passphrase wins; otherwise
/// the KWallet-stored passphrase for that target is used, if one exists.
fn resolve_credentials(
    target: &str,
    passphrase: Option<&str>,
) -> Result<Credentials<'static>, anyhow::Error> {
    let owned = match passphrase {
        Some(p) => Some(p.to_string()),
        None => engine::secrets::get_passphrase(target).unwrap_or(None),
    };
    Ok(Credentials {
        // Leak is deliberate and bounded: one String per distinct target URL
        // for the process lifetime. Credentials borrows &'static str, which
        // lets commands return/hold them without lifetime gymnastics.
        passphrase: owned.map(|p| &*Box::leak(p.into_boxed_str())),
        ..Default::default()
    })
}

#[tauri::command]
fn duplicity_version() -> Result<String, String> {
    let run = runner::run_duplicity(&["--version".to_string()], &Credentials::new(), None)
        .map_err(err_msg)?;
    Ok(run.stdout.trim().to_string())
}

#[tauri::command]
fn list_profiles() -> Vec<ProfileSummary> {
    Profile::list_default()
        .into_iter()
        .filter_map(|name| {
            Profile::load_default(&name).ok().map(|p| ProfileSummary {
                name: p.name,
                target: p.target.clone(),
                sources: p.sources.iter().map(|s| s.display().to_string()).collect(),
                retention: p.retention.clone(),
            })
        })
        .collect()
}

#[tauri::command]
fn load_profile(name: String) -> Result<Profile, String> {
    Profile::load_default(&name).map_err(err_msg)
}

#[tauri::command]
fn save_profile(profile: Profile) -> Result<String, String> {
    let path = profile.save_default().map_err(err_msg)?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn delete_profile(name: String) -> Result<(), String> {
    let path = Profile::default_profiles_dir().join(format!("{name}.toml"));
    std::fs::remove_file(&path).map_err(|e| format!("deleting {}: {e}", path.display()))
}

#[tauri::command]
fn collection_status(
    target: String,
    passphrase: Option<String>,
) -> Result<engine::collection::CollectionReport, String> {
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;
    runner::collection_status(&target, &creds, None).map_err(err_msg)
}

// ---------- passphrase (KWallet / Secret Service) ----------

#[tauri::command]
fn store_passphrase(target: String, passphrase: String) -> Result<(), String> {
    engine::secrets::store_passphrase(&target, &passphrase).map_err(err_msg)
}

#[tauri::command]
fn get_passphrase(target: String) -> Result<Option<String>, String> {
    engine::secrets::get_passphrase(&target).map_err(err_msg)
}

#[tauri::command]
fn forget_passphrase(target: String) -> Result<(), String> {
    engine::secrets::forget_passphrase(&target).map_err(err_msg)
}

#[derive(Serialize, Clone)]
struct ListingStale {
    target: String,
    stale: bool,
}

#[derive(Serialize)]
struct BrowseResult {
    listing: engine::files::FileListing,
    from_cache: bool,
    /// True when a background freshness check is running.
    freshness_check_running: bool,
}

#[tauri::command]
fn browse_files(
    app: AppHandle,
    target: String,
    time: Option<String>,
    passphrase: Option<String>,
    force_refresh: Option<bool>,
) -> Result<BrowseResult, String> {
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;

    // Instant path: serve the cached index without any network activity.
    // Historical views are immutable; only "now" needs a freshness check.
    if !force_refresh.unwrap_or(false) {
        if let Some((listing, cached_end)) =
            engine::listing_cache::get_cached(&target, time.as_deref())
        {
            if time.is_some() {
                return Ok(BrowseResult {
                    listing,
                    from_cache: true,
                    freshness_check_running: false,
                });
            }
            let creds_bg = creds.clone();
            let app_bg = app.clone();
            let target_bg = target.clone();
            std::thread::spawn(move || {
                let fresh = engine::listing_cache::current_chain_end(&target_bg, &creds_bg)
                    .ok()
                    .flatten();
                let stale = match (fresh, cached_end) {
                    (Some(f), Some(c)) => f != c,
                    (Some(_), None) => true, // cached listing predates chain tracking
                    (None, _) => false,      // status failed; say nothing rather than cry wolf
                };
                let _ = app_bg.emit(
                    "listing-stale",
                    ListingStale { target: target_bg, stale },
                );
            });
            return Ok(BrowseResult {
                listing,
                from_cache: true,
                freshness_check_running: true,
            });
        }
    }

    // Slow path: real duplicity run (can take minutes to hours on big
    // targets), then cache the parse.
    engine::listing_cache::get_or_fetch(&target, time.as_deref(), &creds, None)
        .map(|(listing, from_cache)| BrowseResult {
            listing,
            from_cache,
            freshness_check_running: false,
        })
        .map_err(err_msg)
}

/// Start a backup job for a profile. Returns a job id immediately; progress
/// arrives as `job-log` events and completion as `job-done`.
#[tauri::command]
fn start_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    force_new_chain: Option<bool>,
) -> Result<u64, String> {
    let job_id = state.next_job_id.fetch_add(1, Ordering::SeqCst);

    // Cheap validation up-front so refusals appear instantly in the UI.
    let prepared = engine::jobs::prepare_backup(&name, force_new_chain.unwrap_or(false))
        .map_err(err_msg)?;

    std::thread::spawn(move || {
        let app_for_log = app.clone();
        let listener = move |line: &str| {
            let _ = app_for_log.emit(
                "job-log",
                JobLogLine { job_id, line: line.to_string() },
            );
        };
        let result = engine::jobs::run_backup(&prepared, Some(&listener));
        let _ = match result {
            Ok(outcome) => app.emit(
                "job-done",
                JobDone {
                    job_id,
                    ok: outcome.success,
                    exit_code: None,
                    message: Some(outcome.message),
                },
            ),
            Err(e) => {
                let message = err_msg(e);
                app.emit(
                    "job-done",
                    JobDone { job_id, ok: false, exit_code: None, message: Some(message.clone()) },
                )
                .map(|_| {
                    let _ = app.emit(
                        "job-log",
                        JobLogLine { job_id, line: format!("ERROR: {message}") },
                    );
                })
            }
        };
    });

    Ok(job_id)
}

fn format_elapsed(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Start a restore job: `path` (or the whole snapshot when None) from
/// `target` at optional `time`, restored *into* `dest_dir` (the item keeps
/// its own name). Progress streams as job-log events.
#[tauri::command]
fn start_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    path: Option<String>,
    time: Option<String>,
    dest_dir: String,
    passphrase: Option<String>,
) -> Result<u64, String> {
    let job_id = state.next_job_id.fetch_add(1, Ordering::SeqCst);
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;

    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let app_log = app.clone();
        let listener = move |line: &str| {
            let _ = app_log.emit(
                "job-log",
                JobLogLine { job_id, line: line.to_string() },
            );
        };
        match runner::restore_into_dir(
            &target,
            path.as_deref(),
            time.as_deref(),
            &dest_dir,
            &creds,
            Some(&listener),
        ) {
            Ok(outcome) => {
                let message = if outcome.success {
                    Some(format!(
                        "Restored to {} (took {})",
                        outcome.final_dest,
                        format_elapsed(started.elapsed())
                    ))
                } else if outcome.refused_existing {
                    Some(format!(
                        "Restored nothing: {}. Duplicity never overwrites existing files.",
                        outcome.stderr
                    ))
                } else {
                    Some(format!(
                        "Restore failed — see log above.{}",
                        if outcome.stderr.trim().is_empty() {
                            String::new()
                        } else {
                            format!("\n{}", outcome.stderr.trim())
                        }
                    ))
                };
                let _ = app.emit(
                    "job-done",
                    JobDone { job_id, ok: outcome.success, exit_code: None, message },
                );
            }
            Err(e) => {
                let _ = app.emit(
                    "job-done",
                    JobDone { job_id, ok: false, exit_code: None, message: Some(err_msg(e)) },
                );
            }
        }
    });

    Ok(job_id)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("launching file manager for {path}: {e}"))?;
    Ok(())
}

/// Start a verify job: full data comparison of `target` (at optional `time`)
/// against `local_path`. Streams duplicity's messages, then reports the
/// parsed verdict. This reads the entire backup from the target.
#[tauri::command]
fn start_verify(
    app: AppHandle,
    state: State<'_, AppState>,
    target: String,
    local_path: String,
    time: Option<String>,
    passphrase: Option<String>,
) -> Result<u64, String> {
    let job_id = state.next_job_id.fetch_add(1, Ordering::SeqCst);
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;

    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let app_log = app.clone();
        let listener = move |line: &str| {
            let _ = app_log.emit(
                "job-log",
                JobLogLine { job_id, line: line.to_string() },
            );
        };
        match runner::verify(
            &target,
            &local_path,
            true,
            time.as_deref(),
            &creds,
            Some(&listener),
        ) {
            Ok(outcome) => {
                for line in &outcome.report.difference_lines {
                    let _ = app.emit(
                        "job-log",
                        JobLogLine { job_id, line: format!("  DIFFERENCE: {line}") },
                    );
                }
                let verdict = match outcome.report.is_clean() {
                    Some(true) => format!(
                        "VERIFIED — {} file(s) compared, 0 differences (took {})",
                        outcome.report.files_compared.unwrap_or(0),
                        format_elapsed(started.elapsed())
                    ),
                    Some(false) => format!(
                        "DIFFERENCES FOUND — {} file(s) compared, {} difference(s); see the list above",
                        outcome.report.files_compared.unwrap_or(0),
                        outcome.report.differences.unwrap_or(0)
                    ),
                    None => "Verify did not complete — see log above".to_string(),
                };
                let ok = outcome.success && outcome.report.is_clean() == Some(true);
                let _ = app.emit(
                    "job-done",
                    JobDone { job_id, ok, exit_code: None, message: Some(verdict) },
                );
            }
            Err(e) => {
                let _ = app.emit(
                    "job-done",
                    JobDone { job_id, ok: false, exit_code: None, message: Some(err_msg(e)) },
                );
            }
        }
    });

    Ok(job_id)
}

#[tauri::command]
fn get_schedule(name: String) -> Result<engine::schedule::ScheduleStatus, String> {
    let profile = Profile::load_default(&name).map_err(err_msg)?;
    engine::schedule::status(&profile).map_err(err_msg)
}

/// Set (calendar = Some) or remove (calendar = None) a profile's schedule.
#[tauri::command]
fn set_schedule(name: String, calendar: Option<String>) -> Result<engine::schedule::ScheduleStatus, String> {
    let mut profile = Profile::load_default(&name).map_err(err_msg)?;
    if profile.read_only && calendar.is_some() {
        return Err(format!("'{}' is restore-only; scheduling is disabled", name));
    }
    profile.schedule = calendar.unwrap_or_default().trim().to_string();
    profile.save_default().map_err(err_msg)?;

    if profile.schedule.is_empty() {
        // Best-effort disable: a never-enabled timer may legitimately fail.
        let _ = engine::schedule::disable(&name);
    } else {
        engine::schedule::write_units(&profile).map_err(err_msg)?;
        engine::schedule::enable(&name).map_err(err_msg)?;
    }
    engine::schedule::status(&profile).map_err(err_msg)
}

/// Dry-run retention: list what `remove-older-than` would delete, without
/// deleting anything.
#[tauri::command]
fn retention_preview(
    target: String,
    retention: String,
    passphrase: Option<String>,
) -> Result<Vec<String>, String> {
    if retention.trim().is_empty() {
        return Err("No retention period is set on this profile".to_string());
    }
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;
    let run = engine::runner::remove_older_than(&target, retention.trim(), false, &creds, None)
        .map_err(err_msg)?;
    if !run.success() {
        return Err(format!(
            "retention dry-run failed: {}",
            run.stderr.trim()
        ));
    }
    Ok(run
        .stdout
        .lines()
        .chain(run.stderr.lines())
        .map(|s| s.to_string())
        .filter(|l| !l.trim().is_empty())
        .collect())
}

#[tauri::command]
fn home_dir() -> String {    std::env::var("HOME").unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            duplicity_version,
            list_profiles,
            load_profile,
            save_profile,
            delete_profile,
            collection_status,
            browse_files,
            store_passphrase,
            get_passphrase,
            forget_passphrase,
            start_backup,
            start_restore,
            start_verify,
            get_schedule,
            set_schedule,
            retention_preview,
            home_dir,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
