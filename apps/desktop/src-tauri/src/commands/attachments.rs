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
    let name = safe_filename(meta.filename.as_deref());
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
    }
    // One opener for all three platforms, and pointedly not a shell.
    //
    // The Windows branch used to be `cmd /C start "" <path>`, which hands an
    // attacker-named file to a command interpreter: cmd re-parses its command
    // line after Rust's quoting, and expands %VARIABLES% even inside quotes,
    // so an attachment called `report %USERNAME%.pdf` opened something else or
    // nothing at all. The plugin calls ShellExecuteW directly — the same
    // reasoning as `open_external`, which says in as many words that the
    // opener is handed one argument and never a shell.
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(|e| e.to_string())?;
    Ok(())
}

/// An attachment's name, made safe to write to disk on any of the three
/// platforms.
///
/// A MIME filename is whatever the sender wrote. Taking the basename keeps it
/// from escaping the directory; that much was already here. What was missing
/// is that Windows refuses a further nine characters outright — `\\ / : * ? "
/// < > |` — and reserves a list of device names, so an attachment called
/// `Re: quarterly.pdf`, which is an ordinary thing for a person to send,
/// could not be written at all. The failure was a raw `fs::write` error at
/// the moment of opening, on Windows only.
///
/// Sanitised on every platform rather than behind a `cfg`, so the same name
/// produces the same file everywhere and the rule is testable on the machine
/// the developer happens to have.
pub(crate) fn safe_filename(raw: Option<&str>) -> String {
    const FALLBACK: &str = "attachment";
    // Reserved by DOS, still reserved by Windows, with or without extension.
    const DEVICES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];

    let base = raw
        .and_then(|f| {
            // Both separators, because the name came off the wire and a
            // Windows-shaped path means nothing to `file_name` on Unix.
            f.rsplit(['/', '\\']).next()
        })
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .unwrap_or(FALLBACK);

    let mut out: String = base
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            // Control characters are illegal in a Windows filename and a
            // nuisance in a Unix one.
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();

    // Windows silently strips trailing dots and spaces, which turns "a. " into
    // "a" and two attachments into one. Do it here, visibly, instead.
    let trimmed = out.trim_end_matches([' ', '.']);
    if trimmed.len() != out.len() {
        out = trimmed.to_string();
    }
    if out.is_empty() {
        return FALLBACK.to_string();
    }

    // `nul.txt` is as reserved as `nul`.
    let stem = out.split('.').next().unwrap_or("").to_ascii_lowercase();
    if DEVICES.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
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

#[cfg(test)]
mod tests {
    use super::safe_filename;

    /// The filename on an attachment is whatever the sender typed, and it has
    /// to survive being written to disk on all three platforms.
    #[test]
    fn a_senders_filename_is_made_safe_to_write() {
        // Unremarkable names are left exactly alone.
        assert_eq!(safe_filename(Some("Q3 Invoice.pdf")), "Q3 Invoice.pdf");
        assert_eq!(safe_filename(Some("réunion.ics")), "réunion.ics");

        // The bug: legal in a mail header, refused by Windows. A colon in a
        // subject-shaped filename is an ordinary thing for a person to send,
        // and `fs::write` failed outright on it.
        assert_eq!(
            safe_filename(Some("Re: quarterly.pdf")),
            "Re_ quarterly.pdf"
        );
        assert_eq!(
            safe_filename(Some(r#"a*b?c<d>e|f"g.txt"#)),
            "a_b_c_d_e_f_g.txt"
        );

        // Directory traversal, both separators — the name came off the wire,
        // so a Windows-shaped path has to be understood on Unix too.
        assert_eq!(safe_filename(Some("../../etc/passwd")), "passwd");
        assert_eq!(
            safe_filename(Some(r"..\..\Windows\System32\evil.dll")),
            "evil.dll"
        );

        // Windows strips trailing dots and spaces silently, which quietly
        // turns two attachments into one file.
        assert_eq!(safe_filename(Some("report. . ")), "report");

        // Reserved device names, with or without an extension.
        assert_eq!(safe_filename(Some("NUL")), "_NUL");
        assert_eq!(safe_filename(Some("con.txt")), "_con.txt");
        assert_eq!(safe_filename(Some("console.txt")), "console.txt");

        // Nothing usable left, or nothing to begin with.
        assert_eq!(safe_filename(None), "attachment");
        assert_eq!(safe_filename(Some("   ")), "attachment");
        assert_eq!(safe_filename(Some("..")), "attachment");
        assert_eq!(safe_filename(Some("/")), "attachment");
    }
}
