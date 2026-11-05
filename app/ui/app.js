// GUI For Duplicity — frontend logic (vanilla JS, no build step).
// All heavy lifting happens in the Rust engine; this file only orchestrates
// views and calls the Tauri commands.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

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
  $("#profile-heading").textContent = `Profile: ${p.name || "(new)"}`;
  $("#save-msg").textContent = "";
  $("#btn-delete").disabled = !p.name;
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
    const oldName = currentProfile?.name;
    await invoke("save_profile", { profile: p });
    if (oldName && oldName !== p.name) await invoke("delete_profile", { name: oldName });
    flash(`Saved to ${p.name}.toml`);
    currentProfile = p;
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
      .map((s) => `<div>${s.kind}: ${fmt(s.time)}${s.volumes != null ? ` (${s.volumes} volumes)` : ""}</div>`)
      .join("");
    div.innerHTML = `
      <h3>Chain ${i + 1}${badge}</h3>
      <div class="meta">
        ${chain.num_backup_sets ?? chain.sets.length} set(s),
        ${chain.num_volumes ?? "?"} volumes —
        ${fmt(chain.start_time)} → ${fmt(chain.end_time)}
      </div>
      <div class="setlist">${sets}</div>`;
    box.appendChild(div);
  }
}

// ---------- backup ----------
$("#btn-backup").onclick = async () => {
  const p = readForm();
  if (!p.name) return flashError("Save the profile first (it needs a name)");
  try {
    activeJobId = await invoke("start_backup", { name: p.name, passphrase: null });
    $("#log").textContent = "";
    $("#log-job").textContent = `job #${activeJobId} — profile "${p.name}"`;
    $("#log-status").textContent = "Running…";
    showView("log");
  } catch (e) {
    flashError(String(e));
  }
};

// ---------- boot ----------
(async () => {
  await listen("job-log", (ev) => {
    const { job_id: id, line } = ev.payload;
    if (id !== activeJobId) return;
    const log = $("#log");
    log.textContent += line + "\n";
    log.scrollTop = log.scrollHeight;
  });

  await listen("job-done", (ev) => {
    const { job_id: id, ok } = ev.payload;
    if (id !== activeJobId) return;
    const el = $("#log-status");
    el.textContent = ok ? "Backup finished successfully." : "Backup FAILED — see log above.";
    el.className = "msg" + (ok ? "" : " error");
  });

  showView("profile");
  blankProfile();
  await refreshProfiles();
  if (profiles.length > 0) await openProfile(profiles[0]);
})();
