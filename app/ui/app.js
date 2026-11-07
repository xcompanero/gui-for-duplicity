// GUI For Duplicity — frontend logic (vanilla JS, no build step).
// All heavy lifting happens in the Rust engine; this file only orchestrates
// views and calls the Tauri commands.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// Visible build marker: if this text appears in the sidebar, the CURRENT
// app.js is provably running (guards against stale embedded/cached assets).
const BUILD_TAG = "build 4 — 2026-11-07 (hidden-attribute fix)";
document.addEventListener("DOMContentLoaded", () => {
  const el = document.getElementById("build-tag");
  if (el) el.textContent = `UI ${BUILD_TAG} — JS running`;
});

const $ = (sel) => document.querySelector(sel);

// ---------- state ----------
let profiles = [];
let currentProfile = null; // the loaded Profile object
let activeJobId = null;

// ---------- view switching ----------
function showView(name) {
  for (const v of document.querySelectorAll(".view")) v.hidden = true;
  $(`#view-${name}`).hidden = false;
}

// ---------- profiles ----------
async function refreshProfiles() {
  profiles = await invoke("list_profiles");
  renderProfileList();
}

function renderProfileList() {
  const nav = $("#profile-list");
  nav.innerHTML = "";
  for (const p of profiles) {
    const btn = document.createElement("button");
    btn.textContent = p.name;
    btn.className = p.name === currentProfile?.name ? "active" : "";
    btn.onclick = () => openProfile(p.name);
    nav.appendChild(btn);
  }
}

async function openProfile(name) {
  try {
    currentProfile = await invoke("load_profile", { name });
  } catch (e) {
    flashError(String(e));
    return;
  }
  fillForm(currentProfile);
  showView("profile");
  renderProfileList();
}

function fillForm(p) {
  const f = $("#profile-form");
  f.name.value = p.name || "";
  f.target.value = p.target || "";
  f.sources.value = (p.sources || []).join("\n");
  f.excludes.value = (p.excludes || []).join("\n");
  f.retention.value = p.retention || "";
  f.extra_options.value = (p.extra_options || []).join(" ");
  f.encrypt.checked = p.encrypt !== false;
  f.read_only.checked = p.read_only === true;
  f.original_root.value = p.original_root || "";
  f.schedule.value = p.schedule || "";
  f.read_only.onchange = () => { $("#btn-backup").hidden = f.read_only.checked; };
  $("#profile-heading").textContent = `Profile: ${p.name || "(new)"}`;
  $("#save-msg").textContent = "";
  $("#btn-delete").disabled = !p.name;
  refreshScheduleStatus(p.name || "");
  // Restore-only profiles must never offer the backup button; verify needs
  // exactly one source directory to compare against.
  $("#btn-backup").hidden = f.read_only.checked;
  $("#btn-verify").hidden =
    f.read_only.checked ||
    !(f.sources.value.split("\n").map((s) => s.trim()).filter(Boolean).length === 1);
  $("#btn-show-status").hidden = false;
}

function blankProfile() {
  currentProfile = {
    name: "",
    target: "",
    sources: [],
    excludes: [],
    extra_options: [],
    gpg_options: [],
    retention: "",
    encrypt: true,
    read_only: false,
    original_root: null,
    schedule: "",
    last_backup_encrypt: null,
  };
  fillForm(currentProfile);
  showView("profile");
  renderProfileList();
  $("#profile-form").name.focus();
}

function readForm() {
  const f = $("#profile-form");
  return {
    name: f.name.value.trim(),
    target: f.target.value.trim(),
    sources: f.sources.value.split("\n").map((s) => s.trim()).filter(Boolean),
    excludes: f.excludes.value.split("\n").map((s) => s.trim()).filter(Boolean),
    extra_options: f.extra_options.value.trim() ? f.extra_options.value.trim().split(/\s+/) : [],
    gpg_options: [],
    retention: f.retention.value.trim(),
    encrypt: f.encrypt.checked,
    read_only: f.read_only.checked,
    original_root: f.original_root.value.trim() || null,
    schedule: f.schedule.value.trim(),
    // Internal state not editable in the form must round-trip, or saving
    // silently erases it (the schedule had the same bug once).
    last_backup_encrypt: currentProfile?.last_backup_encrypt ?? null,
  };
}

function flash(msg, isError = false) {
  const el = $("#save-msg");
  el.textContent = msg;
  el.className = "msg" + (isError ? " error" : "");
}

function flashError(msg) { flash(msg, true); }

$("#profile-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const p = readForm();
  if (!p.name) return flashError("Name is required");
  try {
    const previousSchedule = currentProfile?.schedule || "";
    const oldName = currentProfile?.name;
    await invoke("save_profile", { profile: p });
    if (oldName && oldName !== p.name) await invoke("delete_profile", { name: oldName });
    flash(`Saved to ${p.name}.toml`);
    currentProfile = p;
    // Keep the systemd timer in lockstep with the schedule field: a save
    // that changed the schedule applies it (this is what previously let a
    // save silently wipe a configured schedule while its timer kept firing).
    if (p.schedule !== previousSchedule) {
      try {
        const s = await invoke("set_schedule", {
          name: p.name,
          calendar: p.schedule || null,
        });
        if (s.enabled && s.next_run?.startsWith("WARNING")) {
          $("#schedule-status").textContent = s.next_run;
          $("#schedule-status").className = "msg error";
        }
      } catch (e) {
        $("#schedule-status").textContent = String(e);
        $("#schedule-status").className = "msg error";
      }
    }
    await refreshProfiles();
    renderProfileList();
  } catch (e) {
    flashError(String(e));
  }
});

$("#btn-delete").onclick = async () => {
  if (!currentProfile?.name) return;
  if (!confirm(`Delete profile "${currentProfile.name}"? (Only the profile definition is removed — your backups are untouched.)`)) return;
  await invoke("delete_profile", { name: currentProfile.name });
  currentProfile = null;
  blankProfile();
  await refreshProfiles();
};

$("#btn-new-profile").onclick = blankProfile;

// ---------- passphrase (KWallet) ----------
$("#btn-store-pass").onclick = async () => {
  const target = $("#profile-form").target.value.trim();
  const pass = $("#profile-form").passphrase.value;
  if (!target) return flashError("A target URL is required before storing a passphrase");
  if (!pass) return flashError("Type the passphrase first, then save it");
  try {
    await invoke("store_passphrase", { target, passphrase: pass });
    $("#profile-form").passphrase.value = "";
    flash("Passphrase saved to KWallet for this target.");
  } catch (e) {
    flashError(String(e));
  }
};

$("#btn-forget-pass").onclick = async () => {
  const target = $("#profile-form").target.value.trim();
  if (!target) return flashError("A target URL is required");
  try {
    await invoke("forget_passphrase", { target });
    flash("Stored passphrase removed from KWallet (if any).");
  } catch (e) {
    flashError(String(e));
  }
};

// ---------- status view ----------
$("#btn-show-status").onclick = async () => {
  const p = readForm();
  if (!p.target) return flashError("A target URL is required to query status");
  $("#status-target").textContent = p.target;
  $("#status-summary").textContent = "Querying collection-status…";
  $("#chains").innerHTML = "";
  showView("status");
  try {
    const report = await invoke("collection_status", { target: p.target, passphrase: null });
    renderStatus(report);
  } catch (e) {
    $("#status-summary").textContent = String(e);
  }
};

function renderStatus(report) {
  const last = report.last_full_backup
    ? new Date(report.last_full_backup).toLocaleString()
    : "none";
  $("#status-summary").textContent =
    `Last full backup: ${last} — ${report.chains.length} chain(s) found`;
  const box = $("#chains");
  box.innerHTML = "";
  for (const [i, chain] of report.chains.entries()) {
    const div = document.createElement("div");
    div.className = "chain";
    const badge = chain.kind === "Primary"
      ? '<span class="primary-badge">primary</span>'
      : '<span class="secondary-badge">secondary</span>';
    const fmt = (t) => (t ? new Date(t).toLocaleString() : "?");
    const sets = chain.sets
      .map(
        (s) => `<div>${s.kind}: ${fmt(s.time)}${s.volumes != null ? ` (${s.volumes} volumes)` : ""}
          <button class="setbrowse" data-time="${s.time || ""}" title="Browse the files as of this moment">⌛ browse as of…</button></div>`,
      )
      .join("");
    div.innerHTML = `
      <h3>Chain ${i + 1}${badge}</h3>
      <div class="meta">
        ${chain.num_backup_sets ?? chain.sets.length} set(s),
        ${chain.num_volumes ?? "?"} volumes —
        ${fmt(chain.start_time)} → ${fmt(chain.end_time)}
      </div>
      <div class="setlist">${sets}</div>
      ${chain.kind === "Primary" ? '<button class="browse-btn">Browse current files</button>' : ""}`;
    box.appendChild(div);
  }
  for (const btn of box.querySelectorAll(".browse-btn")) {
    btn.onclick = () => openBrowserForChain($("#status-target").textContent, null);
  }
  for (const btn of box.querySelectorAll(".setbrowse")) {
    btn.onclick = () => {
      if (!btn.dataset.time) return;
      openBrowserForChain($("#status-target").textContent, btn.dataset.time);
    };
  }
}

// ---------- file browser ----------
let browseTarget = null;
let browseTime = null; // null = current state; ISO string = historical snapshot
let browseListing = null;
let browseDirPrefix = null; // Path prefix currently "entered", or null = root

$("#btn-files-back").onclick = () => showView("status");

// Paths that are prefixes of other paths are directories (duplicity's
// listing does not mark them).
function computeDirs(entries) {
  const dirs = new Set();
  for (const e of entries) {
    const parts = String(e.path).split("/");
    parts.pop();
    let acc = "";
    for (const p of parts) {
      acc = acc ? acc + "/" + p : p;
      dirs.add(acc);
    }
  }
  return dirs;
}

function parentOf(path) {
  const parts = String(path).split("/");
  parts.pop();
  return parts.join("/");
}

function renderFileBrowser() {
  if (!browseListing) return;
  const all = browseListing.entries.filter((e) => e.path !== ".");
  const dirs = computeDirs(all);
  const filterText = $("#file-filter").value.toLowerCase().trim();
  const prefix = browseDirPrefix;

  // Inside a directory with no text filter: direct children only.
  // Otherwise: everything matching the filter (across the whole tree).
  let shown;
  if (prefix && !filterText) {
    shown = all.filter((e) => parentOf(e.path) === prefix);
  } else {
    shown = all.filter((e) => !filterText || e.path.toLowerCase().includes(filterText));
    if (prefix) shown = shown.filter((e) => e.path === prefix || e.path.startsWith(prefix + "/"));
  }

  $("#file-count").textContent = `${shown.length} of ${all.length} entries`;
  $("#files-meta").textContent = `${browseTarget} — at: ${prefix || "/"}`;

  const box = $("#file-list");
  box.innerHTML = "";

  if (shown.length > MAX_ROWS) {
    const note = document.createElement("div");
    note.className = "frow";
    note.innerHTML = `<span class="ficon">…</span><span>Showing the first ${MAX_ROWS} of ${shown.length} matches — refine the filter to narrow further.</span>`;
    box.appendChild(note);
    shown = shown.slice(0, MAX_ROWS);
  }

  if (prefix) {
    const up = document.createElement("div");
    up.className = "frow dir";
    up.innerHTML = `<span class="ficon">📁</span><span>..</span>`;
    up.onclick = () => {
      browseDirPrefix = parentOf(prefix) || null;
      renderFileBrowser();
    };
    box.appendChild(up);
  }

  for (const e of shown) {
    const isDir = dirs.has(e.path);
    const row = document.createElement("div");
    row.className = "frow" + (isDir ? " dir" : "");
    const label = prefix ? String(e.path).slice(prefix.length + 1) : String(e.path);
    row.innerHTML = `<span class="ficon">${isDir ? "📁" : "📄"}</span>
      <span class="fdate">${new Date(e.mtime).toLocaleString()}</span>
      <span>${label}</span>
      <button class="frestore" title="Restore this ${isDir ? "folder" : "file"}">⤓</button>`;
    if (isDir) {
      const labelSpan = row.querySelectorAll("span")[2];
      labelSpan.onclick = () => {
        browseDirPrefix = String(e.path);
        $("#file-filter").value = "";
        renderFileBrowser();
      };
    }
    box.appendChild(row);
  }
}

async function openBrowserForChain(target, time) {
  browseTarget = target;
  browseTime = time;
  browseDirPrefix = null;
  $("#file-filter").value = "";
  $("#stale-banner").hidden = true;
  const asOf = time ? ` as of ${time.replace("T", " ")}` : " (current)";
  $("#files-meta").textContent = `${target}${asOf} — at: /`;
  $("#file-count").textContent = "";
  $("#file-list").innerHTML = time
    ? '<div class="frow"><span>Building the historical index — this can take many minutes the first time for each date. The good news: a past state never changes, so this view is cached permanently afterwards…</span></div>'
    : '<div class="frow"><span>Loading cached file index…</span></div>';
  showView("files");
  try {
    const res = await invoke("browse_files", {
      target,
      time,
      passphrase: null,
      forceRefresh: false,
    });
    browseListing = res.listing;
    $("#files-meta").textContent =
      `${target}${asOf} — at: / ${res.from_cache ? "(from cache)" : "(freshly parsed)"}`;
    if (res.freshness_check_running) {
      $("#files-meta").textContent += " — checking for newer backups in background…";
    }
    renderFileBrowser();
  } catch (e) {
    $("#file-list").innerHTML = "";
    $("#file-count").textContent = String(e);
  }
}

// Background freshness check result: offer a refresh, never block.
// (Listener registered in the boot block below.)
let onListingStale = (ev) => {
  const { target, stale } = ev.payload;
  if (target !== browseTarget) return;
  // Historical views are immutable — a late freshness event from an
  // earlier "current" browse must not claim otherwise.
  if (browseTime !== null) return;
  $("#stale-banner").hidden = !stale;
};

$("#btn-refresh-index").onclick = async () => {
  const target = browseTarget;
  if (!target) return;
  const btn = $("#btn-refresh-index");

  // Historical indexes are immutable — nothing to refresh; just dismiss.
  if (browseTime !== null) {
    $("#stale-banner").hidden = true;
    return;
  }

  // Always acknowledge the click immediately, even though the re-parse
  // can take minutes (or ~an hour on very large targets).
  btn.disabled = true;
  $("#file-list").innerHTML =
    '<div class="frow"><span>Refreshing the index — duplicity is re-parsing its signatures. Minutes on small targets, up to about an hour on very large ones. The previous index stays until this finishes.</span></div>';
  try {
    const res = await invoke("browse_files", {
      target,
      time: null,
      passphrase: null,
      forceRefresh: true,
    });
    browseListing = res.listing;
    $("#stale-banner").hidden = true;
    renderFileBrowser();
  } catch (e) {
    $("#file-list").innerHTML =
      `<div class="frow"><span>Refresh failed: ${String(e)}</span></div>`;
  } finally {
    btn.disabled = false;
  }
};

$("#btn-dismiss-stale").onclick = () => {
  $("#stale-banner").hidden = true;
};

// Debounced filter: re-render at most every 150 ms while typing, and render
// at most MAX_ROWS rows — filtering 60k entries on every keystroke would
// otherwise make the whole page feel frozen.
let filterTimer = null;
const MAX_ROWS = 500;
$("#file-filter").addEventListener("input", () => {
  clearTimeout(filterTimer);
  filterTimer = setTimeout(renderFileBrowser, 150);
});

// ---------- restore from the browser ----------
let restoreJobKind = null; // "backup" | "restore"
let lastRestoreDest = null;

// Per-row ⤓ buttons (event delegation: rows come and go with filtering).
$("#file-list").addEventListener("click", async (ev) => {
  const btn = ev.target.closest(".frestore");
  if (!btn) return;
  const row = btn.closest(".frow");
  const labelSpan = row?.querySelectorAll("span")[2];
  if (!labelSpan) return;
  const rel = labelSpan.textContent;
  const path = browseDirPrefix ? `${browseDirPrefix}/${rel}` : rel;
  openRestoreBar(path);
});

async function openRestoreBar(path) {
  $("#restore-path").textContent = path;
  $("#restore-note").textContent = "";
  const bar = $("#restore-bar");
  bar.hidden = false;
  // "To original location" is offered when the profile declares the root of
  // the filesystem the archive was taken from.
  const root = currentProfile?.original_root;
  const origBtn = $("#btn-orig-dest");
  if (root) {
    const parent = parentOf(path);
    const origDir = root === "/" ? `/${parent}` : `${root.replace(/\/$/, "")}/${parent}`;
    origBtn.hidden = false;
    origBtn.onclick = () => {
      $("#restore-dest").value = origDir;
      $("#restore-note").textContent = `Original location: ${origDir}/${path.split("/").pop()}`;
    };
  } else {
    origBtn.hidden = true;
  }
  const destInput = $("#restore-dest");
  if (!destInput.value) {
    try {
      const home = await invoke("home_dir");
      destInput.value = `${home}/RestoredFromBackup`;
    } catch {
      destInput.value = "/tmp/restored-from-backup";
    }
  }
  destInput.focus();
}

$("#btn-cancel-restore").onclick = () => {
  $("#restore-bar").hidden = true;
};

// ---------- schedule ----------
async function refreshScheduleStatus(name) {
  const el = $("#schedule-status");
  if (!name) { el.textContent = ""; return; }
  try {
    const s = await invoke("get_schedule", { name });
    if (!s.calendar) {
      el.textContent = "Not scheduled — backups run manually only.";
    } else if (s.enabled) {
      el.textContent = `Scheduled (${s.calendar}) — next run: ${s.next_run ?? "pending"}`;
    } else {
      el.textContent = `Schedule set (${s.calendar}) but the timer is NOT enabled.`;
    }
  } catch (e) {
    el.textContent = String(e);
  }
}

$("#btn-set-schedule").onclick = async () => {
  const p = readForm();
  if (!p.name) return flashError("Save the profile first");
  const cal = $("#profile-form").schedule.value.trim();
  if (!cal) return flashError("Enter an OnCalendar expression first (e.g. daily)");
  try {
    await invoke("save_profile", { profile: { ...p, schedule: cal } });
    const s = await invoke("set_schedule", { name: p.name, calendar: cal });
    if (s.next_run?.startsWith("WARNING")) {
      $("#schedule-status").textContent = s.next_run;
      $("#schedule-status").className = "msg error";
      return;
    }
    $("#schedule-status").textContent = s.enabled
      ? `Scheduled (${s.calendar}) — next run: ${s.next_run ?? "pending"}`
      : "Timer could not be enabled — see the application log.";
    await refreshProfiles();
    renderProfileList();
  } catch (e) {
    $("#schedule-status").textContent = String(e);
  }
};

$("#btn-remove-schedule").onclick = async () => {
  const p = readForm();
  if (!p.name) return;
  try {
    await invoke("save_profile", { profile: { ...p, schedule: "" } });
    await invoke("set_schedule", { name: p.name, calendar: null });
    $("#schedule-status").textContent = "Schedule removed — backups run manually only.";
  } catch (e) {
    $("#schedule-status").textContent = String(e);
  }
};

// ---------- retention ----------
$("#btn-retention-preview").onclick = async () => {
  const p = readForm();
  const el = $("#schedule-status");
  if (!p.target) return flashError("A target URL is required");
  if (!p.retention) return flashError("Set a retention period first (e.g. 6M)");
  el.textContent = "Running retention dry-run…";
  try {
    const lines = await invoke("retention_preview", { target: p.target, retention: p.retention });
    el.textContent = lines.length === 0
      ? "Dry run: nothing to remove — everything is within retention."
      : `Dry run: ${lines.length} line(s) reported. Nothing was deleted:\n` +
        (lines.length > 20
          ? lines.slice(0, 20).join("\n") + `\n… and ${lines.length - 20} more`
          : lines.join("\n"));
    el.className = "msg";
  } catch (e) {
    el.textContent = String(e);
    el.className = "msg error";
  }
};

// ---------- verify ----------
$("#btn-verify").onclick = async () => {
  const p = readForm();
  if (!p.target || p.sources.length !== 1) {
    return flashError("Verify needs a target URL and exactly one source directory");
  }
  const proceed = confirm(
    `Full verification of:\n  ${p.target}\nagainst source:\n  ${p.sources[0]}\n\n` +
      "Every file's data in the backup will be read from the target and compared — " +
      "for large or remote archives this can take hours.\n\nProceed?"
  );
  if (!proceed) return;

  restoreJobKind = "verify";
  try {
    activeJobId = await invoke("start_verify", {
      target: p.target,
      localPath: p.sources[0],
      time: null,
      passphrase: null,
    });
    $("#log").textContent = "";
    $("#log-job").textContent = `verify job #${activeJobId} — ${p.target}`;
    $("#log-status").textContent = "Verifying…";
    showView("log");
  } catch (e) {
    flashError(String(e));
  }
};

$("#btn-choose-dest").onclick = async () => {
  try {
    const picked = await window.__TAURI__.dialog.open({
      directory: true,
      multiple: false,
      title: "Choose restore destination",
    });
    if (typeof picked === "string" && picked) {
      $("#restore-dest").value = picked;
    }
  } catch (e) {
    $("#restore-note").textContent = String(e);
  }
};

$("#btn-do-restore").onclick = async () => {
  const path = $("#restore-path").textContent;
  const dest = $("#restore-dest").value.trim();
  if (!path || !dest) return;
  const home = await invoke("home_dir");
  const intoHome = dest === home || dest.startsWith(`${home}/`);
  const originalSpot = (`${home}/${path}` === `${dest}/${path.split("/").pop()}`);
  const asOf = browseTime ? `\nAs of:    ${browseTime.replace("T", " ")}` : "";
  const lines = [
    `Restore:  ${path}`,
    `From:     ${browseTarget}${asOf}`,
    `Into:     ${dest}/${path.split("/").pop()}`,
    "",
    "Existing files are never overwritten — duplicity refuses.",
    intoHome
      ? "⚠ You have chosen a destination inside your home directory: missing files will be recreated among your live data."
      : "",
    originalSpot
      ? "⚠ This is the file's original location in your home directory."
      : "",
  ].filter(Boolean);
  if (!confirm(lines.join("\n"))) return;

  restoreJobKind = "restore";
  lastRestoreDest = dest;
  try {
    activeJobId = await invoke("start_restore", {
      target: browseTarget,
      path,
      time: browseTime,
      destDir: dest,
      passphrase: null,
    });
    $("#log").textContent = "";
    $("#log-job").textContent = `restore job #${activeJobId} — ${path}`;
    $("#log-status").textContent = "Restoring…";
    showView("log");
  } catch (e) {
    $("#restore-note").textContent = String(e);
  }
};

// ---------- backup ----------
$("#btn-backup").onclick = async () => {
  const p = readForm();
  if (!p.name) return flashError("Save the profile first (it needs a name)");
  await runBackup(p.name, false);
};

async function runBackup(name, forceNewChain) {
  restoreJobKind = "backup";
  try {
    activeJobId = await invoke("start_backup", {
      name,
      forceNewChain,
    });
    $("#log").textContent = "";
    $("#log-job").textContent = `job #${activeJobId} — profile "${name}"${forceNewChain ? " (new chain)" : ""}`;
    $("#log-status").textContent = "Running…";
    showView("log");
  } catch (e) {
    const msg = String(e);
    if (msg.includes("start a new chain")) {
      if (confirm(msg + "\n\nStart a new chain now? (A full backup will run; the old chain stays until you prune it.)")) {
        await runBackup(name, true);
      }
    } else {
      flashError(msg);
    }
  }
}

// ---------- boot ----------
(async () => {
  await listen("listing-stale", onListingStale);
  await listen("job-log", (ev) => {
    const { job_id: id, line } = ev.payload;
    if (id !== activeJobId) return;
    const log = $("#log");
    log.textContent += line + "\n";
    log.scrollTop = log.scrollHeight;
  });

  await listen("job-done", (ev) => {
    const { job_id: id, ok, message } = ev.payload;
    if (id !== activeJobId) return;
    const el = $("#log-status");
    el.textContent = message || (ok ? "Job finished successfully." : "Job FAILED — see log above.");
    el.className = "msg" + (ok ? "" : " error");
    if (restoreJobKind === "restore" && ok && lastRestoreDest) {
      const openBtn = document.createElement("button");
      openBtn.textContent = "Open folder in file manager";
      openBtn.style.marginLeft = "10px";
      openBtn.onclick = async () => {
        try {
          await invoke("open_path", { path: lastRestoreDest });
        } catch (e) {
          console.error(e);
        }
      };
      el.appendChild(openBtn);
    }
    restoreJobKind = null;
  });

  showView("profile");
  blankProfile();
  await refreshProfiles();
  if (profiles.length > 0) await openProfile(profiles[0]);
})();
