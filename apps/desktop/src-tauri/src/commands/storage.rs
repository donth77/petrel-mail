//! What is on this Mac, and taking it elsewhere: the storage report, import, and export.

use crate::diag::log_sync;
use crate::state::{AppState, Timed, active_account};
use petrel_engine::store::{ListView, StorageReport};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::State;

/// What the Storage pane shows.
///
/// Async so the work runs off the main thread: a plain command executes on
/// the thread the window paints from, and this one reads the search index's
/// page count out of `dbstat`, which walks every FTS page. On a large mailbox
/// that held the whole window still for the better part of a second — the
/// pane did not so much load slowly as refuse to appear until the numbers
/// were ready.
#[tauri::command]
pub async fn storage_report(state: State<'_, Arc<AppState>>) -> Result<StorageReport, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let _t = Timed::new("storage_report");
        let store = state.store()?;
        store
            .storage_report(&std::path::Path::new(&state.data_dir).join("petrel.db"))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What removing everything would cost, in the numbers that decide it.
#[derive(serde::Serialize)]
pub(crate) struct RemovalReport {
    /// Messages held here.
    messages: i64,
    /// Of those, the ones that exist nowhere else — imported mail and
    /// anything in a local folder. No resync brings these back.
    local_only: i64,
    accounts: usize,
    bytes: u64,
    /// Shown so the sentence "and this folder goes with it" names a place.
    path: String,
}

/// What "Remove all local data" would take, before anybody agrees to it.
#[tauri::command]
pub async fn removal_report(state: State<'_, Arc<AppState>>) -> Result<RemovalReport, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let store = state.store()?;
        let report = store
            .storage_report(&std::path::Path::new(&state.data_dir).join("petrel.db"))
            .map_err(|e| e.to_string())?;
        Ok(RemovalReport {
            messages: report.messages,
            local_only: store.local_only_messages().map_err(|e| e.to_string())?,
            accounts: store.accounts().map_err(|e| e.to_string())?.len(),
            bytes: report.database_bytes + report.blob_bytes,
            path: state.data_dir.clone(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Deletes the mail, the settings and the stored passwords, then quits.
///
/// The uninstall story, and the reason it has to live in the app: no
/// uninstaller removes this. Dragging Petrel to the Trash leaves the mailbox
/// behind, and the Windows uninstaller's "delete application data" tick box
/// clears `dev.petrel.desktop` while the mail sits in `Petrel` — so somebody
/// who wanted a clean machine keeps gigabytes of mail and a keychain full of
/// passwords without being told.
///
/// Passwords first, because they are the part that cannot be deleted later by
/// hand: once the store is gone there is nothing left that knows which
/// keychain entries were Petrel's, and they would sit there for good.
///
/// Then the whole directory, then quit. Quitting is not tidiness — the store
/// is open, SQLite holds the file, and a window left running on top of a
/// deleted database is a window that will write one back.
#[tauri::command]
pub async fn remove_all_local_data(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let state = Arc::clone(&state);
    let dir = std::path::PathBuf::from(state.data_dir.clone());

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // Every account's password, while the store can still name them.
        if let Ok(store) = state.store()
            && let Ok(accounts) = store.accounts()
        {
            for a in accounts {
                if let Ok(entry) = crate::config::keychain_entry(a.id) {
                    // A missing entry is fine; the point is that none remains.
                    let _ = entry.delete_credential();
                }
            }
        }
        log_sync("removing all local data at the user's request");
        std::fs::remove_dir_all(&dir).map_err(|e| format!("could not remove {dir:?}: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;

    app.exit(0);
    Ok(())
}

/// What an import did, honestly itemised.
#[derive(serde::Serialize)]
pub(crate) struct ImportReport {
    imported: usize,
    /// Already here — same Message-ID. Importing twice is a no-op, not a copy.
    duplicates: usize,
    failed: usize,
}

/// Imports mbox files and .eml messages into a local "Imported" folder.
///
/// Local, marked so: the server has never heard of this folder, so the sync
/// survey must not prune it and the sync loop must not ask about it. The
/// messages carry no UID for the same reason — NULL is already how "not
/// addressable on a server" is spelled here. Dedupe is the ordinary one, by
/// Message-ID, which is what makes a re-import of the same archive report
/// duplicates instead of doubling the mailbox.
/// Async for the same reason as `storage_report`: an archive is read and
/// parsed message by message, minutes of work for a large one, and a plain
/// command would spend those minutes holding the window still.
#[tauri::command]
pub async fn import_mail(
    paths: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<ImportReport, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let _t = Timed::new("import_mail");
        let mut report = ImportReport {
            imported: 0,
            duplicates: 0,
            failed: 0,
        };
        let mut store = state.store()?;
        let account = active_account(&store)?;
        let folder = store
            .ensure_named_folder(account, "Imported")
            .map_err(|e| e.to_string())?;
        store.mark_folder_local(folder).map_err(|e| e.to_string())?;

        for path in &paths {
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    log_sync(&format!("import: could not read {path}: {e}"));
                    report.failed += 1;
                    continue;
                }
            };
            let messages: Vec<Vec<u8>> = if path.to_ascii_lowercase().ends_with(".eml") {
                vec![bytes]
            } else {
                petrel_engine::mbox::split(&bytes)
            };
            for raw in &messages {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    store.ingest_raw(&state.blobs, account, Some(folder), None, raw)
                }));
                match outcome {
                    Ok(Ok(ingested)) if ingested.was_new => {
                        report.imported += 1;
                        state.seeded.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Ok(_)) => report.duplicates += 1,
                    Ok(Err(e)) => {
                        log_sync(&format!("import: one message failed: {e}"));
                        report.failed += 1;
                    }
                    Err(_) => {
                        log_sync("import: one message PANICKED the parser — skipped");
                        report.failed += 1;
                    }
                }
            }
        }
        log_sync(&format!(
            "import: {} new, {} duplicate(s), {} failed",
            report.imported, report.duplicates, report.failed
        ));
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes a view's mail to an mbox file the user chose.
///
/// The path comes from the OS save panel rather than a location Petrel picks:
/// an export is something you take somewhere, and guessing where would make the
/// durability promise depend on knowing where Petrel hides things.
///
/// Async for the same reason as `storage_report`: a whole mailbox is read blob
/// by blob and written out, which is seconds of work on a large account, and
/// a plain command would spend those seconds holding the window still.
#[tauri::command]
pub async fn export_mbox(
    account_id: i64,
    view: Option<String>,
    path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let view = ListView::parse(view.as_deref().unwrap_or("inbox"));
        let store = state.store()?;
        let (written, skipped) = store
            .export_mbox(&state.blobs, account_id, &view, std::path::Path::new(&path))
            .map_err(|e| e.to_string())?;
        log_sync(&format!(
            "exported {written} message(s) from account {account_id} to mbox, {skipped} skipped"
        ));
        Ok(format!("{written}/{skipped}"))
    })
    .await
    .map_err(|e| e.to_string())?
}
