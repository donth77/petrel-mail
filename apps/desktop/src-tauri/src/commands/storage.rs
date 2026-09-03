//! What is on this Mac, and taking it elsewhere: the storage report, import, and export.

use crate::diag::log_sync;
use crate::state::{AppState, Timed, active_account};
use petrel_engine::store::{ListView, StorageReport};
use std::path::PathBuf;
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

/// What a data directory on its way out is called: the same name with this
/// and a number after it, so the next launch can recognise one without
/// keeping a note anywhere.
pub(crate) const REMOVED_MARKER: &str = ".removed-";

/// Moves the data directory out of the way, and says where it went.
///
/// A rename rather than a delete, because on Windows a delete cannot work:
/// SQLite opens its file without FILE_SHARE_DELETE, so as long as this
/// process is alive the database cannot be removed — and the old code
/// deleted the keychain entries first and then failed at the directory,
/// leaving a mailbox that could no longer sign in to anything. A rename of
/// the containing directory succeeds while the file inside it is open.
pub(crate) fn rename_aside(dir: &std::path::Path, stamp: i64) -> std::io::Result<PathBuf> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Petrel".to_string());
    let parent = dir.parent().unwrap_or(std::path::Path::new("."));
    let mut target = parent.join(format!("{name}{REMOVED_MARKER}{stamp}"));
    // Twice in the same millisecond is not a thing a person does, but a
    // leftover from a previous attempt is.
    let mut n = 1;
    while target.exists() {
        target = parent.join(format!("{name}{REMOVED_MARKER}{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(dir, &target)?;
    Ok(target)
}

/// Deletes whatever an earlier removal renamed aside. Called at launch,
/// when nothing holds those files open any more.
pub(crate) fn purge_removed(dir: &std::path::Path) -> usize {
    let name = match dir.file_name() {
        Some(n) => n.to_string_lossy().into_owned(),
        None => return 0,
    };
    let Some(parent) = dir.parent() else {
        return 0;
    };
    let prefix = format!("{name}{REMOVED_MARKER}");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut gone = 0;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix)
            && std::fs::remove_dir_all(entry.path()).is_ok()
        {
            gone += 1;
        }
    }
    gone
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
/// The order is the whole of it, and it is not the obvious one:
///
///  1. the workers stop, so nothing writes to a store that is going away;
///  2. the account ids are read out while the store can still name them —
///     they are what the keychain entries are keyed by;
///  3. the store connection is replaced, which closes the database file;
///  4. the directory is renamed aside, which works on Windows where a
///     delete does not;
///  5. **only then** the keychain entries go. Passwords used to be deleted
///     first, and when the delete of the open database failed the person was
///     left with all their mail and no way to sign in to any of it;
///  6. the app quits, and the next launch deletes the renamed directory.
#[tauri::command]
pub async fn remove_all_local_data(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let state = Arc::clone(&state);
    let dir = std::path::PathBuf::from(state.data_dir.clone());

    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let accounts: Vec<i64> = state
            .store()
            .ok()
            .and_then(|s| s.account_ids().ok())
            .unwrap_or_default();
        for id in &accounts {
            state.stop_workers(*id);
        }
        log_sync("removing all local data at the user's request");
        {
            // Replaced rather than dropped: every other command reaches for
            // this lock, and an empty in-memory store answers them honestly
            // in the seconds between here and the app quitting.
            let mut store = state.store()?;
            *store = petrel_engine::store::Store::open_in_memory()
                .map_err(|e| format!("could not close the mailbox: {e}"))?;
        }
        let moved = rename_aside(&dir, crate::state::now_ms())
            .map_err(|e| format!("could not remove the mail: {e}"))?;
        log_sync("local data set aside; it goes on the next launch");
        // Now that the mail is certainly gone, and not before.
        for id in accounts {
            if let Ok(entry) = crate::config::keychain_entry(id) {
                // A missing entry is fine; the point is that none remains.
                let _ = entry.delete_credential();
            }
        }
        // And best effort at once, so a machine that is never launched again
        // is not left holding it.
        let _ = std::fs::remove_dir_all(&moved);
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
        let (account, folder) = {
            let mut store = state.store()?;
            let account = active_account(&store)?;
            let folder = store
                .ensure_named_folder(account, "Imported")
                .map_err(|e| e.to_string())?;
            store.mark_folder_local(folder).map_err(|e| e.to_string())?;
            (account, folder)
        };

        for path in &paths {
            // Only a file the person chose in Petrel's own picker.
            let file = match state.vetted_path(path, &[]) {
                Ok(f) => f,
                Err(e) => {
                    log_sync(&format!("import: {e}"));
                    report.failed += 1;
                    continue;
                }
            };
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let bytes = match std::fs::read(&file) {
                Ok(b) => b,
                Err(e) => {
                    // The name, not the path: a log is not the place for
                    // where somebody keeps their files.
                    log_sync(&format!("import: could not read {name}: {e}"));
                    report.failed += 1;
                    continue;
                }
            };
            let messages: Vec<Vec<u8>> = if name.to_ascii_lowercase().ends_with(".eml") {
                vec![bytes]
            } else {
                petrel_engine::mbox::split(&bytes)
            };
            for raw in &messages {
                // The lock per message, not per archive. Held for the whole
                // import, a large mbox stopped every other command in the
                // app for as long as it took — minutes, on a real archive —
                // and the window looked hung rather than busy.
                let mut store = state.store()?;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    store.ingest_raw(&state.blobs, account, Some(folder), None, raw)
                }));
                drop(store);
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
        // Where the save panel said, and nowhere else: this writes a file,
        // and the window used to choose which.
        let target = state.vetted_path(&path, &[])?;
        let store = state.store()?;
        let (written, skipped) = store
            .export_mbox(&state.blobs, account_id, &view, &target)
            .map_err(|e| e.to_string())?;
        log_sync(&format!(
            "exported {written} message(s) from account {account_id} to mbox, {skipped} skipped"
        ));
        Ok(format!("{written}/{skipped}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod removal_tests {
    use super::{purge_removed, rename_aside};

    /// The order that makes removal work on Windows: the directory is moved
    /// out of the way while the database inside it may still be open, and
    /// the next launch deletes what was moved. A delete in its place fails
    /// outright there — and used to fail *after* the passwords were gone.
    #[test]
    fn the_directory_is_renamed_aside_and_deleted_on_the_next_launch() {
        let base = tempfile::tempdir().expect("tempdir");
        let data = base.path().join("Petrel");
        std::fs::create_dir_all(data.join("blobs")).unwrap();
        // A file still open, as the store's would be.
        let db = data.join("petrel.db");
        std::fs::write(&db, b"pretend database").unwrap();
        let held = std::fs::File::open(&db).unwrap();

        let moved = rename_aside(&data, 1_771_803_000_000).expect("renamed aside");
        assert!(!data.exists(), "the data directory is out of the way");
        assert!(moved.exists());
        assert!(
            moved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Petrel.removed-"),
            "{moved:?}"
        );
        drop(held);

        // A second removal before the first was cleaned up gets its own name.
        std::fs::create_dir_all(&data).unwrap();
        let again = rename_aside(&data, 1_771_803_000_000).expect("renamed aside again");
        assert_ne!(again, moved);

        // The next launch takes both, and leaves the live directory alone.
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("petrel.db"), b"fresh").unwrap();
        assert_eq!(purge_removed(&data), 2);
        assert!(!moved.exists());
        assert!(!again.exists());
        assert!(
            data.join("petrel.db").exists(),
            "the new store is untouched"
        );
        // And running it again with nothing to do is not an error.
        assert_eq!(purge_removed(&data), 0);
    }
}
