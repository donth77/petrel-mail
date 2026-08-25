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

    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?compose={draft_id}").into()),
    )
    .title("Petrel")
    .inner_size(720.0, 620.0)
    .min_inner_size(420.0, 360.0)
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

    WebviewWindowBuilder::new(
        &app,
        &label,
        WebviewUrl::App(format!("index.html?message={thread_id}").into()),
    )
    .title("Petrel")
    .inner_size(780.0, 700.0)
    .min_inner_size(420.0, 360.0)
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
