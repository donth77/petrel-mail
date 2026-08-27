//! The synthetic mailbox: what the app shows when no account is configured.

use crate::state::AppState;
use petrel_engine::store::NewMessage;
use petrel_testkit::DemoMailbox;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const DEMO_MESSAGES: usize = 10_000;

/// The demo dataset's version. Bumping it makes the next launch throw the old
/// synthetic mail away and seed afresh; seeding stamps it, so a store that was
/// just seeded is already current.
const DEMO_SEED_VERSION: &str = "3";

pub(crate) fn spawn_demo_seeding(state: Arc<AppState>, account: i64) {
    std::thread::spawn(move || {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        // DemoMailbox, not MailboxGen: the latter generates word-salad on
        // purpose, because search-recall benchmarks need rare tokens and a flat
        // distribution. That is the wrong corpus for looking at the UI, where
        // noise hides exactly the problems you are trying to see.
        let mut generator = DemoMailbox::new(7, DEMO_MESSAGES, now);
        let mut seq = 0usize;
        loop {
            let generated: Vec<_> = generator.by_ref().take(500).collect();
            if generated.is_empty() {
                break;
            }
            // The bytes each row will be read from. Rendered before the lock is
            // taken, because this is string work and the store is shared with
            // the window.
            let raws: Vec<Vec<u8>> = generated
                .iter()
                .enumerate()
                .map(|(i, g)| g.to_rfc822(seq + i))
                .collect();
            seq += generated.len();
            let batch: Vec<NewMessage> = generated
                .into_iter()
                .map(|g| NewMessage {
                    account_id: account,
                    date_ms: g.date_ms,
                    from_addr: g.from_addr,
                    from_display: g.from_display,
                    to_addr: g.to_addr,
                    subject: g.subject,
                    body_text: g.body,
                })
                .collect();
            let n = batch.len();
            match state.store.lock() {
                Ok(mut store) => {
                    // Rows first, then the bytes behind them: inserting returns
                    // the ids the blobs have to be filed against.
                    let Ok(ids) = store.insert_messages(&batch) else {
                        break;
                    };
                    let bodies: Vec<(i64, Vec<u8>)> = ids.into_iter().zip(raws).collect();
                    if let Err(e) = store.attach_raw_bodies(&state.blobs, &bodies) {
                        eprintln!("[demo] could not store message bodies: {e}");
                        break;
                    }
                }
                Err(_) => break,
            }
            state.seeded.fetch_add(n, Ordering::Relaxed);
        }
        state.seeding.store(false, Ordering::Relaxed);
        // Stamp and decorate here rather than leaving both to the next launch.
        // Seeded-but-undecorated is not a state worth showing anyone: the mail
        // exists but belongs to no folder, and the Inbox reads placement, so
        // the window sits empty over ten thousand messages. It also took a
        // third launch to come right, because the unstamped seed looked stale
        // and was thrown away on the second.
        if let Ok(store) = state.store.lock() {
            let _ = store.set_meta("demo_seed_version", DEMO_SEED_VERSION);
        }
        decorate_demo_store(&state, account);
    });
}

/// Demo decoration for a store that holds synthetic mail: tags, read state, a
/// few stars and attachments, so the list shows what the design describes
/// instead of 10,000 identically-unread rows.
///
/// Runs once, guarded by a meta key, and **only when no real account is
/// configured** — this writes flags, and flags on real mail belong to the
/// server, not to a demo routine.
pub(crate) fn reseed_demo_if_stale(state: &Arc<AppState>, account: i64) -> bool {
    let synthetic = {
        let Ok(store) = state.store.lock() else {
            return false;
        };
        if store.meta("demo_seed_version").ok().flatten().as_deref() == Some(DEMO_SEED_VERSION) {
            return false;
        }
        // Only ever touches a store that is *entirely* synthetic. One real
        // message and this does nothing — deleting somebody's mail to improve a
        // demo would be an unforgivable trade.
        store.all_messages_synthetic().unwrap_or(false)
    };
    if !synthetic {
        return false;
    }
    match state.store.lock() {
        Ok(store) => {
            let removed = store.delete_all_messages().unwrap_or(0);
            let _ = store.set_meta("demo_seed_version", DEMO_SEED_VERSION);
            let _ = store.set_meta("demo_decorated", "");
            eprintln!("[demo] cleared {removed} synthetic messages for a fresh seed");
        }
        Err(_) => return false,
    }
    state.seeded.store(0, Ordering::Relaxed);
    state.seeding.store(true, Ordering::Relaxed);
    spawn_demo_seeding(state.clone(), account);
    true
}

pub(crate) fn decorate_demo_store(state: &Arc<AppState>, account: i64) {
    let Ok(store) = state.store.lock() else {
        return;
    };
    if store
        .meta("demo_decorated")
        .ok()
        .flatten()
        .is_some_and(|v| !v.is_empty())
    {
        return;
    }
    // A demo mailbox belongs to somebody. The bootstrap row is named
    // test@example.com, which reads as a fixture rather than a mailbox —
    // in the switcher, in the status bar, and in any screenshot. Only that
    // exact name is replaced, so an account someone made themselves and
    // then removed the servers from keeps the name they gave it.
    if let Ok(accounts) = store.accounts()
        && accounts
            .iter()
            .any(|a| a.id == account && a.email == "test@example.com")
    {
        let _ = store.set_account_email(account, "you@example.com");
    }

    let tags: Vec<(i64, u32)> = [
        ("urgent", "#B0524A", 7u32),
        ("receipts", "#5E7C4A", 11),
        ("read later", "#9A6B1F", 17),
    ]
    .iter()
    .filter_map(|(name, colour, every)| {
        store
            .ensure_tag(account, name, Some(colour))
            .ok()
            .map(|id| (id, *every))
    })
    .collect();

    let ids: Vec<i64> = match store.recent_ids(4000) {
        Ok(v) => v,
        Err(_) => return,
    };
    for (i, id) in ids.iter().enumerate() {
        // Most mail has been read; a scattering has not.
        if i % 6 != 0 {
            let _ = store.set_flags(*id, petrel_engine::store::flags::SEEN, 0);
        }
        if i % 23 == 0 {
            let _ = store.set_flags(*id, petrel_engine::store::flags::FLAGGED, 0);
        }
        if i % 9 == 0 {
            let _ = store.set_has_attachments(*id, true);
        }
        for (tag_id, every) in &tags {
            if (i as u32).is_multiple_of(*every) {
                let _ = store.tag_message(*id, *tag_id);
            }
        }
    }
    // A mailbox without folders is not a mailbox: triage has nowhere to move
    // mail to, and the folder mapping pane has nothing to report.
    for (role, path) in [
        ("inbox", "INBOX"),
        ("archive", "Archive"),
        ("sent", "Sent"),
        ("drafts", "Drafts"),
        ("spam", "Junk"),
        ("trash", "Trash"),
    ] {
        let _ = store.ensure_folder(account, role, path);
    }
    if let Ok(Some(inbox)) = store.folder_for_role(account, "inbox") {
        for id in &ids {
            let _ = store.place_message(*id, inbox);
        }
    }

    let _ = store.set_meta("demo_decorated", "1");
    eprintln!(
        "[demo] decorated {} messages with tags and flags",
        ids.len()
    );
}
