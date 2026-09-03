//! The file dialogs, opened from Rust so the app knows which paths it handed out.
//!
//! Every command that reads or writes a path used to take one straight from
//! the window: the picker ran in the renderer and the path travelled back
//! over IPC as a string. That is fine while the window is Petrel's own page
//! and nothing else — but the whole model of this app is that message
//! content must not reach it, and after a webview compromise one call could
//! mail any file on the disk, or overwrite one. Opening the dialog here
//! turns the path into something the app produced rather than something it
//! was told, and `AppState::vetted_path` refuses everything else.

use crate::state::AppState;
use std::path::PathBuf;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

/// What a pick is for, which decides the filters and whether more than one
/// file may be chosen.
fn open_filters(purpose: &str) -> (&'static str, Vec<(&'static str, &'static [&'static str])>) {
    match purpose {
        "mail" => (
            "Choose mail to import",
            vec![("Mail", &["mbox", "eml"][..])],
        ),
        "settings" => (
            "Choose a settings file",
            vec![("Petrel settings", &["json"][..])],
        ),
        // Attachments are whatever the person wants to send.
        _ => ("Choose files to attach", Vec::new()),
    }
}

fn save_filters(purpose: &str) -> (&'static str, Vec<(&'static str, &'static [&'static str])>) {
    match purpose {
        "mbox" => ("Export mail", vec![("Mailbox", &["mbox"][..])]),
        "settings" => ("Save settings", vec![("Petrel settings", &["json"][..])]),
        _ => ("Save attachment", Vec::new()),
    }
}

/// Opens the system's file picker and reports what was chosen.
///
/// Empty when it was cancelled — an answer, not a failure, and the caller
/// must not report it as one. `attach` and `mail` take several files;
/// `settings` takes one.
///
/// Runs on the thread pool rather than the main thread (`command(async)` on
/// a synchronous function), which is what makes the blocking form of the
/// dialog safe: on the main thread it would wait for an event loop that is
/// waiting for it.
#[tauri::command(async)]
pub fn pick_files(purpose: String, app: tauri::AppHandle) -> Vec<String> {
    let (title, filters) = open_filters(&purpose);
    let mut dialog = app.dialog().file().set_title(title);
    for (name, extensions) in filters {
        dialog = dialog.add_filter(name, extensions);
    }
    if let Some(main) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&main);
    }
    let picked: Vec<PathBuf> = if purpose == "settings" {
        dialog
            .blocking_pick_file()
            .and_then(|f| f.into_path().ok())
            .into_iter()
            .collect()
    } else {
        dialog
            .blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .collect()
    };
    let state = app.state::<std::sync::Arc<AppState>>();
    state.remember_paths(&picked);
    picked
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

/// Opens the system's save panel and reports where to write.
///
/// `None` when it was cancelled. `suggested` is only a starting name; the
/// person may put the file anywhere, and wherever they put it is a path this
/// app then accepts.
#[tauri::command(async)]
pub fn pick_save_path(suggested: String, purpose: String, app: tauri::AppHandle) -> Option<String> {
    let (title, filters) = save_filters(&purpose);
    // The name only: a suggestion arriving with a path in it would be the
    // window choosing a directory, which is the thing this exists to stop.
    let name = std::path::Path::new(&suggested)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "petrel".to_string());
    let mut dialog = app.dialog().file().set_title(title).set_file_name(name);
    for (filter, extensions) in filters {
        dialog = dialog.add_filter(filter, extensions);
    }
    if let Some(main) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&main);
    }
    let chosen = dialog
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())?;
    let state = app.state::<std::sync::Arc<AppState>>();
    state.remember_paths(std::slice::from_ref(&chosen));
    Some(chosen.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{open_filters, save_filters};

    /// Each purpose offers what it is for. An import that shows only `.json`
    /// cannot find an mbox, and the picker is the only way in now.
    #[test]
    fn each_purpose_filters_for_what_it_takes() {
        let (_, mail) = open_filters("mail");
        let extensions: Vec<&str> = mail.iter().flat_map(|(_, e)| e.iter().copied()).collect();
        assert!(extensions.contains(&"mbox") && extensions.contains(&"eml"));

        let (_, settings) = open_filters("settings");
        assert_eq!(
            settings
                .iter()
                .flat_map(|(_, e)| e.iter().copied())
                .collect::<Vec<_>>(),
            vec!["json"]
        );

        // Attachments are anything, and so is an attachment being saved.
        assert!(open_filters("attach").1.is_empty());
        assert!(save_filters("attachment").1.is_empty());
        assert!(
            save_filters("mbox")
                .1
                .iter()
                .any(|(_, e)| e.contains(&"mbox"))
        );
        // An unknown purpose falls to the permissive case rather than
        // refusing: a new caller gets a working picker, not a dead button.
        assert!(open_filters("something new").1.is_empty());
    }
}
