//! Drafts on the server: pushed after a pause, dropped when sent or discarded.

use crate::config::imap_config_for;
use crate::diag::log_sync;
use crate::signin::SignIn;
use crate::state::AppState;
use crate::sync::loggable;
use petrel_engine::store::Store;
use std::sync::Arc;

/// Pushes one draft to the server's Drafts folder, replacing its previous
/// copy there.
///
/// The draft travels under a Message-ID minted on its first push and kept for
/// life: every later push carries the same one, so the server copy is an edit
/// rather than a sibling — and when ordinary folder sync fetches it back, the
/// dedupe key lands it on the local draft row instead of beside it. The copy
/// this store recorded last time is removed even when its words have changed,
/// and so is any other copy of these same words. A copy that says something
/// else may be another client's revision and is left standing, every one of
/// them, so a conflict is never silently discarded.
pub(crate) async fn push_draft_to_server(
    state: &Arc<AppState>,
    draft_id: i64,
) -> Result<(), String> {
    push_draft_using(state, draft_id, imap_config_for).await
}

async fn push_draft_using<F>(
    state: &Arc<AppState>,
    draft_id: i64,
    config_for: F,
) -> Result<(), String>
where
    F: FnOnce(&Store, i64) -> Option<petrel_providers::imap::ImapConfig>,
{
    let (account, record, msgid, old_uid, cfg, drafts_folder, identity, from_addr) = {
        let mut store = state.store()?;
        // The draft's own account, never the active one: a send or a save can
        // finish after the rail has been switched, and the copy must land in
        // the Drafts of the account that wrote it.
        let Some(account) = store
            .account_of_message(draft_id)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        // A draft that has left Drafts — trashed, spammed — must not grow a
        // new server copy there. The debounce from the last save can still
        // fire after the row has moved, and an APPEND would let ordinary
        // folder sync put it back in the local Drafts list.
        if !store
            .message_in_role(draft_id, "drafts")
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        // Nor one with a send time: that is post in the outbox, not a draft.
        // The save inside Send starts this debounce, and firing during the
        // undo window put a fresh draft on the server for a message already
        // on its way — one another client could send a second time.
        if store.has_send_time(draft_id).map_err(|e| e.to_string())? {
            return Ok(());
        }
        // Signed out: the push waits for the account to sign in, rather than
        // asking a server that refuses the password. The autosave's every
        // thirty seconds was one refused sign-in each.
        if state.signin(account).is_some() {
            state.hold_draft(account, draft_id);
            return Ok(());
        }
        let record = store.load_draft(draft_id).map_err(|e| e.to_string())?;
        let (msgid, old_uid) = store
            .draft_sync_state(draft_id)
            .map_err(|e| e.to_string())?;
        let Some(cfg) = config_for(&store, account) else {
            // No server to push to is not a failure of the draft. Servers and
            // no password Petrel can read is not "no server": the account
            // needs its password, and the push waits for it.
            if has_servers(&store, account) {
                state.set_signin(account, SignIn::Missing);
                state.hold_draft(account, draft_id);
            }
            return Ok(());
        };
        // With the UIDVALIDITY the recorded copy's number belongs to, read
        // under the same guard as the number: the old copy is expunged by it.
        let drafts_folder = store
            .folder_for_role(account, "drafts")
            .ok()
            .flatten()
            .and_then(|fid| {
                let path = store.folder_path(fid).ok().flatten()?;
                Some((fid, path, store.folder_validity(fid).ok().flatten()))
            });
        let identity = store.identity(account).ok();
        // The draft's Message-ID is minted under the account's own address,
        // the same one the message will go out as — never the login name,
        // which is not always an address at all.
        let from_addr = crate::send::sender_address(identity.as_ref(), &cfg.user);
        let domain = crate::send::address_domain(&from_addr);
        let msgid = match msgid {
            Some(m) => m,
            None => {
                let minted = format!(
                    "draft-{:x}.{}@{domain}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0),
                    std::process::id(),
                );
                store
                    .set_draft_msgid(draft_id, &minted)
                    .map_err(|e| e.to_string())?;
                minted
            }
        };
        (
            account,
            record,
            msgid,
            old_uid,
            cfg,
            drafts_folder,
            identity,
            from_addr,
        )
    };
    let Some((drafts_id, drafts_path, drafts_validity)) = drafts_folder else {
        return Ok(());
    };

    let raw = draft_copy(
        &record,
        from_addr,
        identity.map(|i| i.display_name).unwrap_or_default(),
        &msgid,
    );

    let replaced = petrel_providers::imap::replace_draft_copy(
        &cfg,
        &drafts_path,
        drafts_validity,
        &raw,
        &msgid,
        old_uid,
        state.caps(account).has_uidplus,
    )
    .await
    .map_err(|e| format!("append: {e}"))?;
    for uid in &replaced.unremoved {
        log_sync(&format!("old draft copy (uid {uid}) not removed"));
    }
    // A push that could not name its copy leaves the recorded UID as it was.
    // If removing that copy failed, the next push tries again; if it worked,
    // removing it again does nothing, since a UID is not given out twice.
    if let Some(uid) = replaced.new_uid
        && let Ok(mut store) = state.store.lock()
    {
        let _ = store.set_draft_server_uid(draft_id, Some(uid));
    }
    log_sync(&format!("draft {draft_id} pushed to folder {drafts_id}"));
    Ok(())
}

/// Marks the draft dirty and, if it was clean, starts the 30-second clock.
///
/// Saves inside the window coalesce: the sleeping task pushes whatever the
/// draft says when the clock runs out, which is the newest save. Closing the
/// composer pushes immediately through the `push_draft` command instead.
pub(crate) fn schedule_draft_push(state: Arc<AppState>, draft_id: i64) {
    {
        let Ok(mut dirty) = state.draft_dirty.lock() else {
            return;
        };
        if !dirty.insert(draft_id) {
            return; // a task is already sleeping on it
        }
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let still_dirty = state
            .draft_dirty
            .lock()
            .map(|mut d| d.remove(&draft_id))
            .unwrap_or(false);
        if still_dirty && let Err(e) = push_draft_to_server(&state, draft_id).await {
            log_sync(&format!("draft {draft_id} push failed: {}", loggable(&e)));
        }
    });
}

/// A server copy of a draft to remove: its folder's path, its UID, and the
/// UIDVALIDITY that UID was learned under. A removal held while the account
/// was signed out can wait hours, and a folder renumbered meanwhile has given
/// the number to another message; the server's SELECT refuses it then.
pub(crate) type ServerCopy = (String, u32, Option<u32>);

/// Deletes the draft's server copies — for a draft being discarded, or one
/// that just became a sent message. Reads through the caller's guard,
/// because two of the three callers already hold the lock.
pub(crate) fn drop_server_draft_using(state: &AppState, store: &Store, draft_id: i64) {
    // The draft's account, not the active one. This runs from the send
    // worker after the undo window, by which time the rail may show another
    // account — and expunging this UID in *that* account's Drafts destroys
    // whatever draft it holds there.
    let Some(account) = store.account_of_message(draft_id).ok().flatten() else {
        return;
    };
    let copies = server_copies(store, account, draft_id);
    if copies.is_empty() {
        return;
    }
    // Signed out, the copies wait to be removed until the account signs in.
    // Left alone they would sync back as drafts of mail already sent; asked
    // for now, each would be a refused sign-in.
    if state.signin(account).is_some() {
        state.hold_drops(account, copies);
        return;
    }
    let Some(cfg) = imap_config_for(store, account) else {
        if has_servers(store, account) {
            state.set_signin(account, SignIn::Missing);
            state.hold_drops(account, copies);
        }
        return;
    };
    let uidplus = state.caps(account).has_uidplus;
    tauri::async_runtime::spawn(async move {
        expunge_copies(&cfg, copies, uidplus).await;
    });
}

/// Removes server copies of a draft, one by one.
pub(crate) async fn expunge_copies(
    cfg: &petrel_providers::imap::ImapConfig,
    copies: Vec<ServerCopy>,
    uidplus: bool,
) {
    // UIDPLUS makes the expunge surgical. Without it a copy is only
    // flagged \Deleted, because a bare EXPUNGE would commit other
    // clients' deletions too, and it stays until the server expunges it.
    for (path, uid, validity) in copies {
        if let Err(e) =
            petrel_providers::imap::expunge_uid(cfg, &path, uid, validity, uidplus).await
        {
            log_sync(&format!(
                "server draft copy (uid {uid}) not removed: {}",
                loggable(&e)
            ));
        }
    }
}

/// Whether the account has servers, which makes a missing password a
/// missing password rather than an account with no server to reach.
fn has_servers(store: &Store, account: i64) -> bool {
    store
        .account_servers(account)
        .ok()
        .flatten()
        .is_some_and(|s| !s.imap_host.is_empty())
}

/// Does what waited for the account to sign in: the drafts whose push was
/// held, and the server copies whose removal was. Called whenever signing in
/// has just worked — a first pass, the hourly retry, a cycle that reached the
/// server — however the folders fared. A failure is logged, as any push's
/// is. What meets the account signed out again is held again, and what a
/// worker stopped part-way had not reached is handed back for the account's
/// next run. Held work lives in memory, so a quit still loses it (docs/25
/// #98).
pub(crate) async fn push_held(
    state: &Arc<AppState>,
    account: i64,
    stop: &tokio::sync::watch::Receiver<bool>,
) {
    let held = state.take_held(account);
    if held == crate::signin::Held::default() {
        return;
    }
    log_sync(&format!(
        "account {account}: {} held draft push(es) and {} held removal(s) go now",
        held.drafts.len(),
        held.drops.len()
    ));
    // Whose mailbox the removals were meant for, as the store has it now.
    let mailbox = mailbox_of(state, account);
    let mut drafts = held.drafts.into_iter();
    while let Some(draft) = drafts.next() {
        if *stop.borrow() {
            let rest = std::iter::once(draft).chain(drafts).collect();
            hand_back(state, account, &mailbox, rest, held.drops);
            return;
        }
        // Signed out meanwhile, the push holds the draft again itself.
        if let Err(e) = push_draft_to_server(state, draft).await {
            log_sync(&format!("draft {draft} push failed: {}", loggable(&e)));
        }
    }
    if held.drops.is_empty() {
        return;
    }
    let cfg = if state.signin(account).is_some() {
        None
    } else {
        let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
        imap_config_for(&store, account)
    };
    let Some(cfg) = cfg else {
        hand_back(state, account, &mailbox, Vec::new(), held.drops);
        return;
    };
    let uidplus = state.caps(account).has_uidplus;
    let mut drops = held.drops.into_iter();
    while let Some(copy) = drops.next() {
        // One sign-in each: a stopped worker's password may be the one Sign
        // in again just replaced, and a refusal met meanwhile stands.
        if *stop.borrow() || state.signin(account).is_some() {
            let rest = std::iter::once(copy).chain(drops).collect();
            hand_back(state, account, &mailbox, Vec::new(), rest);
            return;
        }
        expunge_copies(&cfg, vec![copy], uidplus).await;
    }
}

/// The server and login an account's mail lives under, as the store has it.
fn mailbox_of(state: &AppState, account: i64) -> Option<(String, String)> {
    let store = state.store.lock().unwrap_or_else(|p| p.into_inner());
    let servers = store.account_servers(account).ok().flatten()?;
    Some((servers.imap_host, servers.username))
}

/// Holds again what `push_held` did not reach, for the account's next run.
///
/// Only while the account runs: a removed account's work dies with it. And
/// removals only for the mailbox they were meant for, because ids are
/// reused — an account set up under this one's id, on another server,
/// would have had UIDs in its own folders expunged.
fn hand_back(
    state: &AppState,
    account: i64,
    mailbox: &Option<(String, String)>,
    drafts: Vec<i64>,
    drops: Vec<ServerCopy>,
) {
    if *state.stop_signal(account).borrow() {
        return;
    }
    for draft in drafts {
        state.hold_draft(account, draft);
    }
    if !drops.is_empty() && mailbox.is_some() && mailbox_of(state, account) == *mailbox {
        state.hold_drops(account, drops);
    }
}

/// The server copies of a draft to drop, by folder path and UID: every one
/// the store has numbered in Drafts, or in the Trash or Spam a conversation
/// took it to (`Store::draft_copies`), and the copy the last push recorded,
/// which Drafts may not have numbered yet. Each once, and each with its
/// folder's UIDVALIDITY, read through the same guard as the number.
pub(crate) fn server_copies(store: &Store, account: i64, draft_id: i64) -> Vec<ServerCopy> {
    let mut copies = store.draft_copies(draft_id).unwrap_or_default();
    let pushed = store
        .draft_sync_state(draft_id)
        .ok()
        .and_then(|(_, uid)| uid);
    let drafts = store
        .folder_for_role(account, "drafts")
        .ok()
        .flatten()
        .and_then(|fid| store.folder_path(fid).ok().flatten());
    if let (Some(uid), Some(path)) = (pushed, drafts)
        && !copies.contains(&(path.clone(), uid))
    {
        copies.push((path, uid));
    }
    copies
        .into_iter()
        .map(|(path, uid)| {
            let validity = store
                .folder_at(account, &path)
                .ok()
                .flatten()
                .and_then(|(_, validity)| validity);
            (path, uid, validity)
        })
        .collect()
}

/// The copy of a draft that goes to the server's Drafts folder.
///
/// The sender's copy, Bcc header and all, as Thunderbird keeps it: a draft
/// picked up on the phone or in webmail still has its blind copies, and a
/// draft only ever goes to its own account's Drafts.
pub(crate) fn draft_copy(
    record: &petrel_engine::store::DraftRecord,
    from_addr: String,
    from_name: String,
    msgid: &str,
) -> Vec<u8> {
    let msg = petrel_providers::smtp::Outgoing {
        from_addr,
        from_name,
        to: addresses_of(&record.to),
        cc: addresses_of(&record.cc),
        bcc: addresses_of(&record.envelope.bcc),
        subject: record.subject.clone(),
        body_text: record.body.clone(),
        body_html: Some(record.html.clone()).filter(|h| !h.trim().is_empty()),
        in_reply_to: record.envelope.in_reply_to.clone(),
        references: record.envelope.references.clone(),
        // Attachment files stay local until send: a draft's paths may not
        // even exist by the time it is reopened, and pushing megabytes on
        // every autosave is the wrong trade. The text notes nothing; other
        // clients see the words, which is what a draft is.
        attachments: Vec::new(),
    };
    msg.sender_copy(&msg.render_with_id(msgid))
}

/// Splits a recipient field as the send does (`split_recipients`), for
/// rendering a draft whose addresses are still one string. A draft may
/// legitimately have none at all.
pub(crate) fn addresses_of(field: &str) -> Vec<String> {
    crate::send::split_recipients(field)
}

#[cfg(test)]
mod draft_copy_tests {
    use super::draft_copy;
    use petrel_engine::store::{DraftEnvelope, DraftRecord};

    /// The copy in the server's Drafts carries the blind copies as a Bcc
    /// header, so the draft opened on the phone or in webmail still has them.
    #[test]
    fn the_server_copy_of_a_draft_keeps_its_blind_copies() {
        let record = DraftRecord {
            id: 1,
            to: "dana@example.com".into(),
            cc: String::new(),
            subject: "Numbers".into(),
            body: "In.".into(),
            html: String::new(),
            envelope: DraftEnvelope {
                bcc: "priya@example.net, board@example.org".into(),
                ..Default::default()
            },
        };
        let raw = draft_copy(
            &record,
            "sam@example.com".into(),
            "Sam".into(),
            "d1@example.com",
        );
        let parsed = petrel_mime::parse_message(&raw).unwrap();
        let bcc: Vec<&str> = parsed.bcc.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(bcc, vec!["priya@example.net", "board@example.org"]);
        assert_eq!(parsed.message_id.as_deref(), Some("d1@example.com"));
        assert_eq!(parsed.to, vec![(None, "dana@example.com".to_string())]);
    }

    /// A quoted name with a comma is one recipient here too (fifth audit
    /// #174). Split at every comma, `"Wu, Dana" <dana@example.com>` became
    /// `"Wu`, dropped as no address, and `Dana" <…>`: the copy named her
    /// "Dana", and a blind copy lost half its name the same way.
    #[test]
    fn the_server_copy_keeps_a_quoted_name_with_a_comma_whole() {
        let record = DraftRecord {
            id: 1,
            to: "\"Wu, Dana\" <dana@example.com>, alex@example.com".into(),
            cc: String::new(),
            subject: "Numbers".into(),
            body: "In.".into(),
            html: String::new(),
            envelope: DraftEnvelope {
                bcc: "\"Nair, Priya\" <priya@example.net>".into(),
                ..Default::default()
            },
        };
        let raw = draft_copy(
            &record,
            "sam@example.com".into(),
            "Sam".into(),
            "d2@example.com",
        );
        let parsed = petrel_mime::parse_message(&raw).unwrap();
        assert_eq!(
            parsed.to,
            vec![
                (Some("Wu, Dana".to_string()), "dana@example.com".to_string()),
                (None, "alex@example.com".to_string()),
            ]
        );
        assert_eq!(
            parsed.bcc,
            vec![(
                Some("Nair, Priya".to_string()),
                "priya@example.net".to_string()
            )]
        );
    }
}

#[cfg(test)]
mod server_copies_tests {
    use super::server_copies;
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_engine::store::Store;

    /// A draft pushed once: the copy's UID recorded, and numbered in Drafts,
    /// whose numbers belong to UIDVALIDITY 3.
    fn pushed() -> (Store, i64, i64) {
        let mut store = Store::open_in_memory().unwrap();
        let account = store.ensure_test_account().unwrap();
        let drafts = store.ensure_folder(account, "drafts", "Drafts").unwrap();
        store.set_folder_validity(drafts, Some(3)).unwrap();
        store.ensure_folder(account, "trash", "Trash").unwrap();
        let id = store
            .save_draft(account, None, "sam@example.com", "Plans", "yes", "")
            .unwrap();
        store.set_draft_server_uid(id, Some(7)).unwrap();
        store
            .heal_placement_uid(id, account, "Drafts", 7, Some(3))
            .unwrap();
        (store, account, id)
    }

    #[test]
    fn a_draft_in_drafts_drops_its_one_copy() {
        let (store, account, id) = pushed();
        assert_eq!(
            server_copies(&store, account, id),
            vec![("Drafts".to_string(), 7, Some(3))]
        );
    }

    /// A reply binned with its conversation has its copy in the Trash, and
    /// that goes too. Only Drafts was looked in, and the copy synced back as
    /// a draft inside the binned conversation.
    #[test]
    fn a_binned_draft_drops_its_copy_in_the_bin() {
        let (store, account, id) = pushed();
        store
            .apply_message_action(
                account,
                id,
                ActionKind::Trash,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        store
            .heal_placement_uid(id, account, "Trash", 12, None)
            .unwrap();
        assert_eq!(
            server_copies(&store, account, id),
            vec![
                ("Trash".to_string(), 12, None),
                ("Drafts".to_string(), 7, Some(3))
            ]
        );
    }

    /// Nothing is expunged by a number the store does not hold, or anywhere
    /// but Drafts and the bins.
    #[test]
    fn only_numbered_copies_in_drafts_and_the_bins_are_dropped() {
        let (store, account, id) = pushed();
        let archive = store.ensure_folder(account, "archive", "Archive").unwrap();
        store.place_message_at(id, archive, 30).unwrap();
        assert_eq!(
            server_copies(&store, account, id),
            vec![("Drafts".to_string(), 7, Some(3))]
        );
        // Moved to the Trash, with no UID there yet: only the push's copy.
        store
            .apply_message_action(
                account,
                id,
                ActionKind::Trash,
                None,
                PlacementPolicy::Exclusive,
            )
            .unwrap();
        assert_eq!(
            server_copies(&store, account, id),
            vec![("Drafts".to_string(), 7, Some(3))]
        );
    }
}

#[cfg(test)]
mod signed_out_tests {
    //! The review's probe D. A draft's push asked the server whether or not
    //! the account could sign in: writing a reply while the password was
    //! refused was a refused sign-in every thirty seconds.
    use super::push_draft_to_server;
    use crate::scripted_imap::{Srv, serve};
    use crate::signin::SignIn;
    use crate::signin::test_support::{servers_at, unkeyed_account};
    use crate::state::{AppState, test_state};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    fn a_draft(state: &AppState, account: i64) -> i64 {
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Drafts".into(), Some("drafts".into())),
                ],
            )
            .unwrap();
        store
            .save_draft_full(
                account,
                None,
                "dana@example.com",
                "",
                "Numbers",
                "words",
                "",
                &petrel_engine::store::DraftEnvelope::default(),
            )
            .unwrap()
    }

    #[test]
    fn a_draft_waits_for_the_account_to_sign_in() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            let draft = a_draft(&state, account);
            crate::config::remember_password(account, "revoked");
            state.set_signin(account, SignIn::Refused);
            for _ in 0..3 {
                push_draft_to_server(&state, draft).await.unwrap();
            }
            let conns = srv.conns.load(Ordering::SeqCst);
            let held = state.take_held(account);
            crate::config::forget_password(account);
            assert_eq!(conns, 0, "sign-in attempts from three pushes");
            assert!(held.drafts.contains(&draft), "the draft waits, to go later");
        });
    }

    /// Servers and no password Petrel can read: not "no server to push to",
    /// which used to drop the push for good. The account needs its password.
    #[test]
    fn a_draft_of_an_account_without_its_password_waits_too() {
        let _turn = crate::config::cache_turn();
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = unkeyed_account(&state);
        let srv = Srv::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")], &[]);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            servers_at(&state, account, port);
            crate::config::forget_password(account);
            let draft = a_draft(&state, account);
            push_draft_to_server(&state, draft).await.unwrap();
            assert_eq!(srv.conns.load(Ordering::SeqCst), 0);
            assert_eq!(state.signin(account), Some(SignIn::Missing));
            assert!(state.take_held(account).drafts.contains(&draft));
        });
    }
}

/// A push replaces the copy it wrote last time, including when a search of
/// the folder cannot see the append yet.
#[cfg(all(test, feature = "dev-plaintext-imap"))]
mod push_replaces_its_copy {
    use super::push_draft_using;
    use crate::state::test_state;
    use crate::sync::stateful_imap::{Imap, serve};
    use petrel_providers::imap::{Credential, ImapConfig, Security};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    fn plain(port: u16) -> ImapConfig {
        ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            credential: Credential::password("pw"),
            security: Security::InsecurePlaintext,
        }
    }

    fn letter(id: &str, to: &str, subject: &str, body: &str) -> Vec<u8> {
        format!(
            "From: Dana Wu <dana@example.com>\r\nTo: {to}\r\n\
             Subject: {subject}\r\nDate: Tue, 01 Oct 2026 09:00:00 +0000\r\n\
             Message-ID: <{id}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn open(state: &crate::state::AppState, subject: &str, body: &str) -> i64 {
        let account = state.account_id;
        let mut store = state.store.lock().unwrap();
        store
            .sync_folders(
                account,
                &[
                    ("INBOX".into(), Some("inbox".into())),
                    ("Drafts".into(), Some("drafts".into())),
                ],
            )
            .unwrap();
        store
            .save_draft(account, None, "dana@example.com", subject, body, "")
            .unwrap()
    }

    fn bodies(srv: &Imap) -> Vec<String> {
        srv.held("Drafts")
            .into_iter()
            .map(|m| {
                String::from_utf8_lossy(&m.raw)
                    .split("\r\n\r\n")
                    .nth(1)
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .collect()
    }

    /// The server UID this store holds for the draft's copy.
    fn recorded(state: &crate::state::AppState, draft: i64) -> Option<u32> {
        let store = state.store.lock().unwrap();
        store.draft_sync_state(draft).unwrap().1
    }

    /// The composer saving new words, to the same recipient as `open`.
    fn edit(state: &crate::state::AppState, draft: i64, body: &str) {
        let account = state.account_id;
        let store = state.store.lock().unwrap();
        store
            .save_draft(account, Some(draft), "dana@example.com", "Plans", body, "")
            .unwrap();
    }

    async fn push(state: &Arc<crate::state::AppState>, draft: i64, port: u16) {
        push_draft_using(state, draft, move |_, _| Some(plain(port)))
            .await
            .unwrap();
    }

    /// Two edits, and the search never lists the copy just appended. One
    /// message remains, and it is the later edit.
    #[test]
    fn two_pushes_leave_the_latest_copy_when_search_misses_the_append() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let draft = open(&state, "Plans", "one");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.hide_newest_message_id.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            {
                let store = state.store.lock().unwrap();
                store
                    .save_draft(account, Some(draft), "dana@example.com", "Plans", "two", "")
                    .unwrap();
            }
            push(&state, draft, port).await;
        });
        let held = srv.held("Drafts");
        assert_eq!(held.len(), 1, "both edits stayed in Drafts");
        let body = String::from_utf8_lossy(&held[0].raw);
        assert!(body.contains("two"), "the remaining copy is the later edit");
        assert!(
            !body.contains("\r\none\r\n"),
            "the first edit is still there"
        );
        let recorded = state
            .store
            .lock()
            .unwrap()
            .draft_sync_state(draft)
            .unwrap()
            .1;
        assert_eq!(recorded, Some(held[0].uid));
    }

    /// A copy of the same words is the draft and goes. A copy that says
    /// something else, under the same Message-ID, is the other client and stays.
    #[test]
    fn a_push_drops_its_own_duplicate_and_keeps_another_clients_revision() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "yes");
        {
            let mut store = state.store.lock().unwrap();
            store.set_draft_msgid(draft, "d-keep@example.com").unwrap();
        }
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.put(
            "Drafts",
            letter("d-keep@example.com", "dana@example.com", "Plans", "yes"),
            &["\\Draft"],
        );
        srv.put(
            "Drafts",
            letter(
                "d-keep@example.com",
                "dana@example.com",
                "Plans",
                "their edit",
            ),
            &["\\Draft"],
        );
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
        });
        let mut left = bodies(&srv);
        left.sort();
        assert_eq!(
            left,
            vec!["their edit".to_string(), "yes".to_string()],
            "the duplicate of this draft stayed, or the other revision went"
        );
    }

    /// The same words to other people are still the other client's
    /// revision: a copy that changed only who it goes to stays.
    #[test]
    fn a_copy_to_other_recipients_is_another_clients_revision() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "yes");
        {
            let mut store = state.store.lock().unwrap();
            store.set_draft_msgid(draft, "d-cc@example.com").unwrap();
        }
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.put(
            "Drafts",
            letter(
                "d-cc@example.com",
                "dana@example.com, sam@example.com",
                "Plans",
                "yes",
            ),
            &["\\Draft"],
        );
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
        });
        assert_eq!(
            srv.held("Drafts").len(),
            2,
            "a revision that changed only the recipients was taken for a duplicate"
        );
    }

    /// Without UIDPLUS the server names no UID for an append and cannot
    /// expunge one message. The push finds its copy by searching, marks the
    /// one before it, and a later push does not fetch a marked copy again.
    #[test]
    fn without_uidplus_a_push_finds_its_copy_and_passes_over_marked_ones() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "one");
        let srv = Imap::new("MOVE", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        let (first, last) = tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            let first = recorded(&state, draft);
            edit(&state, draft, "two");
            push(&state, draft, port).await;
            edit(&state, draft, "three");
            push(&state, draft, port).await;
            (first, recorded(&state, draft))
        });
        assert_eq!(first, Some(1), "the first push did not find its copy");
        assert_eq!(last, Some(3), "the last push did not find its copy");
        let held = srv.held("Drafts");
        let live: Vec<u32> = held
            .iter()
            .filter(|m| !m.flags.iter().any(|f| f == "\\Deleted"))
            .map(|m| m.uid)
            .collect();
        assert_eq!(live, vec![3], "an earlier copy was not marked deleted");
        assert_eq!(
            srv.said("UID FETCH 1 (BODY.PEEK[])").len(),
            1,
            "a copy already marked deleted was fetched again to compare"
        );
    }

    /// Another push of this draft can land between this one's append and
    /// its search. That copy says the same and is newer: removing it, while
    /// that push removed this one's, left the server with neither.
    #[test]
    fn a_newer_copy_of_the_same_draft_is_left_to_the_push_that_wrote_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "yes");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.twin_next_append.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
        });
        let uids: Vec<u32> = srv.held("Drafts").iter().map(|m| m.uid).collect();
        assert_eq!(
            uids,
            vec![1, 2],
            "the newer copy of the same draft was removed"
        );
        assert_eq!(recorded(&state, draft), Some(1));
    }

    /// Three edits, and a search never lists the copy just appended. One
    /// message remains, and it is the last edit. The number comes from
    /// APPENDUID, which still names a message the search has not listed.
    #[test]
    fn three_edits_leave_one_copy_when_search_misses_the_append() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "one");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.hide_newest_message_id.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            edit(&state, draft, "two");
            push(&state, draft, port).await;
            edit(&state, draft, "three");
            push(&state, draft, port).await;
        });
        let held = srv.held("Drafts");
        assert_eq!(held.len(), 1, "each edit left its own copy: {held:?}");
        let body = String::from_utf8_lossy(&held[0].raw);
        assert!(
            body.contains("three"),
            "the remaining copy is an earlier edit"
        );
        assert_eq!(recorded(&state, draft), Some(held[0].uid));
    }

    /// The UID named by APPENDUID is gone, and the bytes sit at the next
    /// number. Recording the vanished number leaves every rewritten copy in
    /// the folder: the next push deletes a message that is not there.
    #[test]
    fn a_rewritten_append_is_recorded_as_the_copy_that_remains() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "one");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.rewrite_appended.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            edit(&state, draft, "two");
            push(&state, draft, port).await;
            edit(&state, draft, "three");
            push(&state, draft, port).await;
        });
        let held = srv.held("Drafts");
        assert_eq!(held.len(), 1, "rewritten copies piled up: {held:?}");
        let body = String::from_utf8_lossy(&held[0].raw);
        assert!(
            body.contains("three"),
            "the remaining copy is an earlier edit"
        );
        assert_eq!(
            recorded(&state, draft),
            Some(held[0].uid),
            "the vanished APPENDUID was stored"
        );
    }

    /// The raw text of every copy in Drafts.
    fn raws(srv: &Imap) -> Vec<String> {
        srv.held("Drafts")
            .iter()
            .map(|m| String::from_utf8_lossy(&m.raw).into_owned())
            .collect()
    }

    /// Two other clients each saved a revision of this draft, and neither
    /// says what this push says. Nothing shows this client wrote either, so
    /// both stay: a push does not get to choose which of someone's edits to
    /// keep.
    #[test]
    fn every_revision_another_client_saved_stays() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "kiwi");
        {
            let mut store = state.store.lock().unwrap();
            store.set_draft_msgid(draft, "d-many@example.com").unwrap();
        }
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        for body in ["phone revision", "webmail revision"] {
            srv.put(
                "Drafts",
                letter("d-many@example.com", "dana@example.com", "Plans", body),
                &["\\Draft"],
            );
        }
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
        });
        let raws = raws(&srv);
        assert_eq!(raws.len(), 3, "a copy went: {raws:?}");
        for kept in ["phone revision", "webmail revision", "kiwi"] {
            assert!(
                raws.iter().any(|r| r.contains(kept)),
                "{kept} went: {raws:?}"
            );
        }
    }

    /// APPENDUID names a message that is gone, the search misses the copy
    /// this push left, and the one arrival it does list is another client's
    /// revision, landing while the push ran. That arrival says something
    /// else, so it is not recorded as this push's copy, and the next push,
    /// which removes the recorded copy whatever it says, leaves it alone.
    #[test]
    fn an_arrival_that_says_something_else_is_not_taken_for_this_push() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "kiwi");
        {
            let mut store = state.store.lock().unwrap();
            store
                .set_draft_msgid(draft, "d-theirs@example.com")
                .unwrap();
        }
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.put(
            "Drafts",
            letter(
                "d-theirs@example.com",
                "dana@example.com",
                "Plans",
                "their revision",
            ),
            &["\\Draft"],
        );
        srv.rewrite_appended.store(true, Ordering::Relaxed);
        srv.hide_newest_message_id.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            srv.hide_newest_message_id.store(false, Ordering::Relaxed);
            edit(&state, draft, "mango");
            push(&state, draft, port).await;
        });
        let raws = raws(&srv);
        assert!(
            raws.iter().any(|r| r.contains("their revision")),
            "another client's revision was removed: {raws:?}"
        );
        assert!(
            raws.iter().any(|r| r.contains("mango")),
            "the copy just written went: {raws:?}"
        );
    }

    /// Without UIDPLUS, and with a search that misses the copy just
    /// appended, a push cannot name its copy, and that copy stays
    /// unrecorded. The next push finds it beside another client's older
    /// revision. Neither is known to be this client's, so neither goes.
    #[test]
    fn a_copy_this_client_could_not_name_costs_no_other_clients_revision() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "kiwi");
        {
            let mut store = state.store.lock().unwrap();
            store
                .set_draft_msgid(draft, "d-unnamed@example.com")
                .unwrap();
        }
        let srv = Imap::new("MOVE", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.put(
            "Drafts",
            letter(
                "d-unnamed@example.com",
                "dana@example.com",
                "Plans",
                "their revision",
            ),
            &["\\Draft"],
        );
        srv.hide_newest_message_id.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            srv.hide_newest_message_id.store(false, Ordering::Relaxed);
            edit(&state, draft, "mango");
            push(&state, draft, port).await;
        });
        let theirs = srv
            .held("Drafts")
            .into_iter()
            .find(|m| String::from_utf8_lossy(&m.raw).contains("their revision"))
            .expect("another client's revision was expunged");
        assert!(
            !theirs.flags.iter().any(|f| f == "\\Deleted"),
            "another client's revision was marked deleted"
        );
    }

    /// The server files each append under a later number than APPENDUID
    /// names, and answers NO to a fetch of a UID it does not hold, so the
    /// check of APPENDUID fails. The push still finds the copy it left and
    /// removes the one before it, rather than failing after its APPEND had
    /// landed and leaving that copy behind.
    #[test]
    fn a_refused_check_of_appenduid_does_not_fail_the_push() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let draft = open(&state, "Plans", "kiwi");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.rewrite_appended.store(true, Ordering::Relaxed);
        srv.refuse_missing_fetch.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            edit(&state, draft, "mango");
            push(&state, draft, port).await;
        });
        let held = srv.held("Drafts");
        assert_eq!(held.len(), 1, "copies piled up: {:?}", raws(&srv));
        assert!(String::from_utf8_lossy(&held[0].raw).contains("mango"));
        assert_eq!(recorded(&state, draft), Some(held[0].uid));
    }

    /// Two pushes, then the sweep that files server UIDs the store has not
    /// placed. The earlier copy is already gone, so the sweep has nothing
    /// to file beside the draft.
    #[test]
    fn two_pushes_are_not_filed_as_extra_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let account = state.account_id;
        let draft = open(&state, "Plans", "one");
        let srv = Imap::new("UIDPLUS", &[("INBOX", ""), ("Drafts", "\\Drafts")]);
        srv.rewrite_appended.store(true, Ordering::Relaxed);
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            push(&state, draft, port).await;
            edit(&state, draft, "two");
            push(&state, draft, port).await;
        });
        let held = srv.held("Drafts");
        assert_eq!(held.len(), 1, "the first edit is still on the server");
        let uid = recorded(&state, draft).expect("the surviving copy");
        assert_eq!(held[0].uid, uid);
        let folder = {
            let mut store = state.store.lock().unwrap();
            let folder = store.folder_for_role(account, "drafts").unwrap().unwrap();
            store.set_folder_validity(folder, Some(1)).unwrap();
            assert!(
                store
                    .heal_placement_uid(draft, account, "Drafts", uid, Some(1))
                    .unwrap()
            );
            store.set_backfill_floor(folder, 1).unwrap();
            folder
        };
        tauri::async_runtime::block_on(async {
            let port = serve(Arc::clone(&srv)).await;
            super::super::reconcile_ghost_placements(&state, account, &plain(port)).await;
        });
        let mut uids = state.store.lock().unwrap().placement_uids(folder).unwrap();
        uids.sort_unstable();
        assert_eq!(
            uids,
            vec![uid],
            "an earlier copy was filed as its own draft"
        );
        assert!(
            state
                .store
                .lock()
                .unwrap()
                .draft_conflict(draft)
                .unwrap()
                .is_none(),
            "the draft's own earlier push was offered as another client's revision"
        );
    }
}
