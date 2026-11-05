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
    format!("{e:#}")
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
fn collection_status(target: String, passphrase: Option<String>) -> Result<engine::collection::CollectionReport, String> {
    let creds = Credentials {
        passphrase: passphrase.as_deref(),
        ..Default::default()
    };
    runner::collection_status(&target, &creds, None).map_err(err_msg)
}

/// Start a backup job for a profile. Returns a job id immediately; progress
/// arrives as `job-log` events and completion as `job-done`.
#[tauri::command]
fn start_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    passphrase: Option<String>,
) -> Result<u64, String> {
    let profile = Profile::load_default(&name).map_err(err_msg)?;
    let job_id = state.next_job_id.fetch_add(1, Ordering::SeqCst);
    let args = profile.backup_args();

    std::thread::spawn(move || {
        // Credentials must be constructed inside the thread: they borrow the
        // owned passphrase string, which we move in here.
        let creds = Credentials {
            passphrase: passphrase.as_deref(),
            ..Default::default()
        };
        let app_for_log = app.clone();
        let listener = move |line: &str| {
            let _ = app_for_log.emit(
                "job-log",
                JobLogLine { job_id, line: line.to_string() },
            );
        };
        let result = runner::run_duplicity(&args, &creds, Some(&listener));
        let _ = match result {
            Ok(run) => app.emit(
                "job-done",
                JobDone {
                    job_id,
                    ok: run.success(),
                    exit_code: run.exit_status.code(),
                },
            ),
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
            start_backup,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
