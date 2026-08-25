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
