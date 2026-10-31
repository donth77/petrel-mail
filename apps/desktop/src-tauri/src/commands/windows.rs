//! Other windows: popped-out messages and composers, and links handed to the browser.

use tauri::Manager;

/// Opens a saved draft in a window of its own.
///
/// The window loads the app with `?compose=<id>`, which renders the composer
/// alone rather than a second copy of the whole client. A pop-out exists so a
/// long message can have the screen; giving it another rail and message list
/// would defeat the point and cost a second sync loop.
///
/// The draft must already be saved — the id is the only thing the new window
/// gets, and it is also what stops the two windows from being separate
/// unsaved copies of the same message.
#[tauri::command]
pub fn popout_compose(draft_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let label = format!("compose-{draft_id}");
    // Already open: focus it rather than making a second window onto the same
    // draft, which would leave two editors racing to save over each other.
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    crate::without_link_preview(WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?compose={draft_id}").into()),
    ))
    .title("Petrel")
    // A file dropped on this composer is the page's to read, as in the main
    // window. Tauri's own handler takes every file drag first and tells the
    // webview nothing, so the composer's drop never fired here, on any
    // platform; nothing listens for the event Tauri sends instead.
    .disable_drag_drop_handler()
    .inner_size(720.0, 620.0)
    .min_inner_size(420.0, 360.0)
    // The same allow-list as the main window: this is the same bundle, and
    // the popped-out reader shows the same links.
    .on_navigation(crate::main_navigation_allowed)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens one conversation in a window of its own.
///
/// The same bundle with a query parameter, as the popped-out composer is: a
/// second rail, list and sync loop would cost real memory and a second poll
/// against the mail server to show one thread.
#[tauri::command]
pub fn popout_message(thread_id: i64, app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let label = format!("message-{thread_id}");
    // Already open: focus it. A second window onto the same conversation is
    // never what was meant, and both would drift as it is triaged.
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }

    crate::without_link_preview(WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?message={thread_id}").into()),
    ))
    .title("Petrel")
    .inner_size(780.0, 700.0)
    .min_inner_size(420.0, 360.0)
    // The same allow-list as the main window: this is the same bundle, and
    // the popped-out reader shows the same links.
    .on_navigation(crate::main_navigation_allowed)
    .build()
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens a link from a message in the user's browser.
///
/// Mail is the most-phished medium there is, so the scheme is checked here
/// rather than trusted from the frame. Only the two web schemes are handed to
/// the system: `file:` would open local content, `javascript:` is an execution
/// vector, and the custom schemes registered by other applications on the
/// machine are a large and unaudited surface reachable from any sender.
///
/// `mailto:` is deliberately absent — the app answers that itself by opening a
/// composer, rather than handing a mail link to some other mail program.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https links can be opened".into());
    }
    // Passed as a single argument to the platform's opener, never through a
    // shell, so nothing in the URL can be read as a command.
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32.exe");
        c.arg("url.dll,FileProtocolHandler").arg(&url);
        c
    };
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// The number on the Dock icon.
///
/// Unread in the inbox, summed across accounts, which is the number every
/// other surface already shows: `unread_count` deliberately excludes spam,
/// trash and anything filed elsewhere, because counting unread *anywhere*
/// once had the account switcher announce seven for an account whose inbox
/// said zero.
///
/// `None` clears it. That is what "Mailbox counts: None" means here, and it
/// is also what zero means — macOS draws a badge for 0 rather than removing
/// it, and a red dot insisting you have nothing to read is worse than no
/// badge at all.
///
/// The badge lives on the main window rather than the app: a popped-out
/// composer setting a count would be a second, competing badge.
#[tauri::command]
pub fn set_dock_badge(count: Option<i64>, app: tauri::AppHandle) -> Result<(), String> {
    let Some(main) = app.get_webview_window("main") else {
        // A pop-out asking before the main window exists is not an error.
        return Ok(());
    };
    let shown = match count {
        Some(n) if n > 0 => Some(n),
        _ => None,
    };
    // Logged because the badge cannot be read back. macOS exposes it only to
    // assistive APIs, which this machine refuses, so the log line is the only
    // way anyone can check what was actually asked for against what the Dock
    // shows.
    crate::diag::log_sync(&format!("dock badge: {shown:?}"));
    main.set_badge_count(shown).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every window turns WebKit's link previews off. One built without
    /// `without_link_preview` would bring back the force click that opened a
    /// message's link in the system's preview, past Petrel's link handling.
    ///
    /// Every way Tauri has of starting a window or a webview is looked for,
    /// not only the one the app uses today: a window begun another way would
    /// pass a check that knew only one spelling.
    #[test]
    fn every_window_is_built_without_link_previews() {
        // Split so this file does not count as a builder of its own.
        let builders = [
            ["WebviewWindowBuilder", "::new("].concat(),
            ["WebviewWindowBuilder", "::from_config("].concat(),
            ["WebviewWindow", "::builder("].concat(),
            ["WebviewBuilder", "::new("].concat(),
            ["WebviewBuilder", "::from_config("].concat(),
        ];
        let mut files = Vec::new();
        sources(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut found = 0;
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for builder in &builders {
                for (at, _) in text.match_indices(builder.as_str()) {
                    found += 1;
                    let before = text[..at].trim_end().trim_end_matches("tauri::");
                    assert!(
                        before.ends_with("without_link_preview("),
                        "{} starts a webview without without_link_preview",
                        file.display()
                    );
                }
            }
        }
        // The main window, the minimal one, print, source and both pop-outs.
        assert!(found >= 6, "found only {found} window builders");

        // A window named in the config is built by Tauri, never by this code,
        // so it would keep link previews on whatever the code does.
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        let windows = &config["app"]["windows"];
        assert!(
            windows.is_null() || windows.as_array().is_some_and(|w| w.is_empty()),
            "tauri.conf.json names windows, which are built without without_link_preview"
        );
    }
}
