//! Attachments: decoded on demand, saved or opened where the user asks.

use crate::state::AppState;
use std::sync::Arc;
use tauri::State;

/// The bytes of one attachment, re-read from the message's raw blob.
///
/// Nothing is stored twice: the raw message holds every attachment, and the
/// part is decoded when asked for — on save, on open, on preview.
fn attachment_bytes(
    state: &AppState,
    message_id: i64,
    part: usize,
) -> Result<(petrel_mime::Attachment, Vec<u8>), String> {
    let hash = {
        let store = state.store()?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message body not stored")?
    };
    let raw = state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable (failed verification)")?;
    petrel_mime::attachment_bytes(&raw, part)
        .ok_or_else(|| "that attachment is not in the message".into())
}

/// File types that run when opened. Opening one is a real decision — the
/// spec asks for a warning, and the UI asks before calling `open_attachment`
/// on any of these — so the list lives here, next to the thing it guards.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "pif", "ps1", "vbs", "vbe", "js", "jse", "wsf",
    "wsh", "hta", "jar", "app", "dmg", "pkg", "command", "sh", "zsh", "bash", "csh", "py", "rb",
    "pl", "php", "apk", "deb", "rpm", "appimage", "lnk", "url", "reg", "scpt", "action",
    "workflow", "terminal",
];

/// Whether a file name ends in something the OS would execute.
#[tauri::command]
pub fn attachment_is_executable(filename: String) -> bool {
    let ext = std::path::Path::new(&filename)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext {
        Some(e) => EXECUTABLE_EXTENSIONS.contains(&e.as_str()),
        None => false,
    }
}

/// Writes an attachment to a path the user chose. The dialog is the UI's;
/// this only gets the path it produced.
#[tauri::command]
pub fn save_attachment(
    message_id: i64,
    part: usize,
    path: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let (_, bytes) = attachment_bytes(&state, message_id, part)?;
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {path}: {e}"))
}

/// Opens an attachment in whatever the OS uses for its type.
///
/// Written to a per-launch temporary directory first, under its own name
/// so the application that opens it sees the right extension. The file is
/// quarantined the way a download is — macOS then shows its own "downloaded
/// from the internet" prompt for anything it considers risky, on top of the
/// warning the UI has already shown for executables.
#[tauri::command]
pub fn open_attachment(
    message_id: i64,
    part: usize,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let (meta, bytes) = attachment_bytes(&state, message_id, part)?;
    let name = meta
        .filename
        .as_deref()
        .and_then(|f| std::path::Path::new(f).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "attachment".to_string());
    let dir = std::env::temp_dir().join(format!("petrel-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A subdirectory per message and part, so two attachments that share a
    // name do not overwrite each other while both are open.
    let dir = dir.join(format!("{message_id}-{part}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        // The quarantine attribute is what makes Gatekeeper treat this as a
        // download. Best effort: a file the OS cannot mark is still opened,
        // since the UI's own warning has already been shown.
        let _ = std::process::Command::new("xattr")
            .arg("-w")
            .arg("com.apple.quarantine")
            .arg("0083;00000000;Petrel;")
            .arg(&path)
            .status();
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A one-use URL for previewing an attachment in the reading pane, over the
/// same sandboxed protocol that serves message bodies.
#[tauri::command]
pub fn attachment_url(message_id: i64, part: usize, state: State<Arc<AppState>>) -> String {
    format!(
        "petrel-msg://localhost/attachment/{}/{part}",
        state.tokens.issue(message_id)
    )
}
