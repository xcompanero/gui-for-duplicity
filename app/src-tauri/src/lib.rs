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

#[tauri::command]
fn browse_files(
    target: String,
    passphrase: Option<String>,
) -> Result<BrowseResult, String> {
    let creds = resolve_credentials(&target, passphrase.as_deref()).map_err(err_msg)?;
    engine::listing_cache::get_or_fetch(&target, None, &creds, None)
        .map(|(listing, from_cache)| BrowseResult { listing, from_cache })
        .map_err(err_msg)
}

#[derive(Serialize)]
struct BrowseResult {
    listing: engine::files::FileListing,
    from_cache: bool,
}

/// Start a backup job for a profile. Returns a job id immediately; progress
/// arrives as `job-log` events and completion as `job-done`.
#[tauri::command]
fn start_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    passphrase: Option<String>,
    force_new_chain: Option<bool>,
) -> Result<u64, String> {
    let profile = Profile::load_default(&name).map_err(err_msg)?;
    let job_id = state.next_job_id.fetch_add(1, Ordering::SeqCst);

    // Encryption changes are only safe at the start of a new chain.
    if let Some(last) = profile.last_backup_encrypt {
        if last != profile.encrypt && !force_new_chain.unwrap_or(false) {
            return Err(format!(
                "The encryption setting for '{}' changed since the last backup \
                 (target chain was created with encryption {}). \
                 An incremental cannot cross that boundary safely — start a new chain instead.",
                profile.name,
                if last { "ON" } else { "OFF" }
            ));
        }
    }

    let force_full = force_new_chain.unwrap_or(false);
    let args = profile.backup_args(force_full);
    let profile_name = profile.name.clone();
    let now_encrypted = profile.encrypt;
    // 'static credentials (KWallet lookup / leaked owned string) move freely
    // into the job thread.
    let creds = resolve_credentials(&profile.target, passphrase.as_deref()).map_err(err_msg)?;

    // Fail fast and clearly when encryption is intended but no passphrase
    // can be found. Silently proceeding with a placeholder is what caused
    // the "Bad session key" mixed-chain incident.
    if profile.encrypt && creds.passphrase.is_none() {
        return Err(format!(
            "Profile '{}' has encryption enabled, but no passphrase is available.\n\
             Save the GPG passphrase for this target in KWallet (profile editor), \
             or untick 'Encrypt backups' if this target should be unencrypted.",
            profile.name
        ));
    }

    std::thread::spawn(move || {
        let app_for_log = app.clone();
        let listener = move |line: &str| {
            let _ = app_for_log.emit(
                "job-log",
                JobLogLine { job_id, line: line.to_string() },
            );
        };
        let result = runner::run_duplicity(&args, &creds, Some(&listener));
        let _ = match result {
            Ok(run) => {
                // Record the chain's encryption state so future setting
                // changes are caught before they can corrupt anything.
                if run.success() {
                    if let Ok(mut p) = Profile::load_default(&profile_name) {
                        p.last_backup_encrypt = Some(now_encrypted);
                        let _ = p.save_default();
                    }
                }
                app.emit(
                    "job-done",
                    JobDone {
                        job_id,
                        ok: run.success(),
                        exit_code: run.exit_status.code(),
                    },
                )
            }
            Err(e) => app.emit(
                "job-done",
                JobDone { job_id, ok: false, exit_code: None },
            )
            .map(|_| {
                let _ = app.emit(
                    "job-log",
                    JobLogLine { job_id, line: format!("ERROR: {e:#}") },
                );
            }),
        };
    });

    Ok(job_id)
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
