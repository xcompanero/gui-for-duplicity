fn main() {
    // tauri-build only tracks capabilities/tauri.conf.json — without this,
    // UI-only changes (app/ui/*) do NOT retrigger asset re-embedding and the
    // binary keeps serving stale frontend assets. (Discovered 2026-11-07.)
    println!("cargo:rerun-if-changed=../ui");
    tauri_build::build()
}
