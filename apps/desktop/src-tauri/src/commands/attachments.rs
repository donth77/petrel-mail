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

/// File types that run when opened, or that talk something else into
/// running: executables and scripts, the shortcut formats that point at one,
/// and the disk images that mount a folder full of them.
///
/// Opening one is a real decision — the spec asks for a warning — so the
/// list lives here, next to the thing it guards, and the decision is made
/// here rather than taken from the window.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "msi", "bat", "cmd", "com", "scr", "pif", "ps1", "psm1", "vbs", "vbe", "js", "jse",
    "wsf", "wsh", "hta", "jar", "app", "dmg", "iso", "img", "pkg", "command", "sh", "zsh", "bash",
    "csh", "py", "rb", "pl", "php", "apk", "deb", "rpm", "appimage", "lnk", "url", "webloc",
    "inetloc", "desktop", "reg", "cpl", "inf", "msc", "scpt", "action", "workflow", "terminal",
];

/// Whether the file that would actually be written runs when it is opened.
///
/// Decided on the *sanitised* name, which is the whole point of it being a
/// function. The check used to read the raw MIME filename while the file was
/// written under the trimmed one: `invoice.pdf.exe ` has no extension the
/// raw check recognises — the trailing space is part of it — so nothing
/// warned, and what landed on disk was `invoice.pdf.exe`. Windows strips
/// those characters itself, which is why they are worth anything to a
/// sender in the first place.
pub(crate) fn is_executable_attachment(filename: &str) -> bool {
    let safe = safe_filename(Some(filename));
    let ext = std::path::Path::new(&safe)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext {
        Some(e) => EXECUTABLE_EXTENSIONS.contains(&e.as_str()),
        None => false,
    }
}

/// Whether a file name ends in something the OS would execute. The window
/// asks so it can warn; the answer is the same one `open_attachment` makes
/// for itself, so the two can never disagree about a given file.
#[tauri::command(async)]
pub fn attachment_is_executable(filename: String) -> bool {
    is_executable_attachment(&filename)
}

/// Marks a file as downloaded, so Windows treats it as one.
///
/// The Mark of the Web is an alternate data stream beside the file. With it,
/// Office opens the document in Protected View and SmartScreen has something
/// to check; without it a document that arrived by mail is indistinguishable
/// from one the person wrote themselves. macOS has the same idea as the
/// quarantine attribute, which `open_attachment` sets below.
///
/// Best effort on both: a file the OS will not let us mark is still written,
/// because failing the save would be the worse outcome.
fn mark_of_the_web(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::io::Write;
        // Zone 3 is "Internet". The stream is written by opening the file
        // under its own name plus the stream's; it fails on a filesystem
        // with no streams (a FAT-formatted USB stick), which is fine.
        let mut name = path.as_os_str().to_os_string();
        name.push(":Zone.Identifier");
        if let Ok(mut f) = std::fs::File::create(std::path::PathBuf::from(name)) {
            let _ = f.write_all(b"[ZoneTransfer]\r\nZoneId=3\r\n");
        }
    }
    #[cfg(not(windows))]
    let _ = path;
}

/// Writes an attachment to a path the person chose in the save panel.
///
/// The panel is opened by `pick_save_path`, in Rust, and this accepts only a
/// path that came back from it — see `AppState::vetted_path`. Before that
/// the window named the file, so one call could have overwritten anything
/// this user can write to.
#[tauri::command(async)]
pub fn save_attachment(
    message_id: i64,
    part: usize,
    path: String,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let target = state.vetted_path(&path, &[])?;
    let (_, bytes) = attachment_bytes(&state, message_id, part)?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "attachment".into());
    // The name, not the path: this string reaches the window and the log.
    std::fs::write(&target, bytes).map_err(|e| format!("could not write {name}: {e}"))?;
    mark_of_the_web(&target);
    Ok(())
}

/// Opens an attachment in whatever the OS uses for its type.
///
/// Written to a per-launch temporary directory first, under its own name
/// so the application that opens it sees the right extension. The file is
/// quarantined the way a download is — macOS then shows its own "downloaded
/// from the internet" prompt for anything it considers risky, on top of the
/// warning the UI has already shown for executables.
#[tauri::command(async)]
pub fn open_attachment(
    message_id: i64,
    part: usize,
    // Whether the person was warned and said yes. The warning is the
    // window's to show, but the decision about what needs one is made here,
    // on the name the file will actually have — the window asked about the
    // sender's raw name, and a trailing space was enough to slip an
    // executable past the question entirely.
    confirmed: Option<bool>,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let (meta, bytes) = attachment_bytes(&state, message_id, part)?;
    let name = safe_filename(meta.filename.as_deref());
    if is_executable_attachment(&name) && confirmed != Some(true) {
        return Err("this attachment can run when it is opened".into());
    }
    let dir = std::env::temp_dir().join(format!("petrel-open-{}", std::process::id()));
    // 0700 on Unix: /tmp is world-readable, and these are copies of
    // somebody's mail.
    crate::diag::create_private_dir(&dir).map_err(|e| e.to_string())?;
    // A subdirectory per message and part, so two attachments that share a
    // name do not overwrite each other while both are open.
    let dir = dir.join(format!("{message_id}-{part}"));
    crate::diag::create_private_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    mark_of_the_web(&path);
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
#[tauri::command(async)]
pub fn attachment_url(message_id: i64, part: usize, state: State<Arc<AppState>>) -> String {
    format!(
        "{}/attachment/{}/{part}",
        crate::message_view::message_origin(),
        state.tokens.issue(message_id)
    )
}

#[cfg(test)]
mod tests {
    use super::{is_executable_attachment, safe_filename};

    /// The bypass this closes: the warning read the
    /// extension of the raw MIME name, and the file opened was the trimmed
    /// one. A trailing space or dot was the whole trick.
    #[test]
    fn a_trailing_space_or_dot_no_longer_defeats_the_warning() {
        for name in [
            "invoice.pdf.exe ",
            "invoice.pdf.exe.",
            "setup.command ",
            "run.sh.",
            "installer.msi   ",
            "shortcut.lnk.",
        ] {
            assert!(
                is_executable_attachment(name),
                "{name:?} was not flagged, and it is what gets written"
            );
        }
    }

    /// The list covers what the platforms actually run, including the
    /// shortcut formats that point at something else and the disk images
    /// that mount a folder of them.
    #[test]
    fn the_list_covers_the_formats_that_run_or_lead_somewhere_that_does() {
        for name in [
            "a.desktop",
            "a.webloc",
            "a.inetloc",
            "a.iso",
            "a.img",
            "a.cpl",
            "a.inf",
            "a.msc",
            "a.lnk",
            "a.url",
            "a.hta",
            "a.scr",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.jse",
            "a.wsf",
            "a.msi",
            "a.reg",
            "a.bat",
            "a.cmd",
            "a.com",
            "a.pif",
        ] {
            assert!(is_executable_attachment(name), "{name}");
        }
        // Case is the sender's choice and means nothing.
        assert!(is_executable_attachment("Invoice.EXE"));
        assert!(is_executable_attachment("x.Cmd"));
    }

    /// And ordinary mail is not warned about, or the warning stops meaning
    /// anything.
    #[test]
    fn the_files_people_actually_send_raise_nothing() {
        for name in [
            "Q3 Invoice.pdf",
            "photo.jpeg",
            "notes.txt",
            "réunion.ics",
            "archive.zip",
            "deck.pptx",
            "no-extension",
        ] {
            assert!(!is_executable_attachment(name), "{name}");
        }
    }

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
