//! A folder's children are named with its server's delimiter, and only that.
//!
//! IMAP names a child as parent, delimiter, leaf, and each server picks one
//! delimiter: `/` on Gmail, iCloud and Outlook, `.` on many Courier and
//! Dovecot hosts. Everything used to read both, so on a `/` server a folder
//! honestly called `Trash.2019` was part of the bin — Empty Trash expunged it
//! — and `Receipts.old` went wherever `Receipts` went. Once a survey has
//! recorded the delimiter, only it nests; before that, both still do.

use petrel_engine::blob::BlobStore;
use petrel_engine::search_query::Renamed;
use petrel_engine::store::{ListView, Sort, Store};

fn fixture(mid: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\nTo: me@example.com\r\n\
         Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
         Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nbody {subject}\r\n"
    )
    .into_bytes()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    (dir, store, blobs, account)
}

/// Every folder on the account given one delimiter, as a survey records it.
fn surveyed_with(store: &mut Store, account: i64, delim: &str) {
    let rows: Vec<(String, Option<String>)> = store
        .folders(account)
        .unwrap()
        .into_iter()
        .map(|f| (f.path, Some(delim.to_string())))
        .collect();
    store.set_folder_delimiters(account, &rows).unwrap();
}

fn path_of(store: &Store, account: i64, id: i64) -> String {
    store
        .folders(account)
        .unwrap()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
        .path
}

fn subjects(store: &Store, view: &ListView) -> Vec<String> {
    let mut s: Vec<String> = store
        .list_threads(view, 0, 50, Sort::default())
        .unwrap()
        .into_iter()
        .map(|t| t.subject)
        .collect();
    s.sort();
    s
}

/// Renaming `Taxes` to `Taxes.old` used to update the folder, then find it
/// again under `Taxes.` and append the tail twice: `Taxes.old.old` here,
/// `Taxes.old` on the server, and the next survey pruning one as a stranger.
#[test]
fn a_rename_that_extends_the_old_name_is_applied_once() {
    let (_dir, mut store, _blobs, account) = setup();
    let taxes = store.ensure_named_folder(account, "Taxes").unwrap();
    let year = store.ensure_named_folder(account, "Taxes/2024").unwrap();

    store.rename_folder(taxes, "Taxes.old").unwrap();

    assert_eq!(path_of(&store, account, taxes), "Taxes.old");
    assert_eq!(path_of(&store, account, year), "Taxes.old/2024");
}

/// The same, once the server has said it nests with `/`.
#[test]
fn a_rename_that_extends_the_old_name_is_applied_once_on_a_slash_server() {
    let (_dir, mut store, _blobs, account) = setup();
    let clients = store.ensure_named_folder(account, "Clients/Acme").unwrap();
    store.ensure_named_folder(account, "Clients").unwrap();
    surveyed_with(&mut store, account, "/");

    store.rename_folder(clients, "Clients/Acme.com").unwrap();

    assert_eq!(path_of(&store, account, clients), "Clients/Acme.com");
}

/// LIKE ignores ASCII case, so renaming `Work` also rewrote `work/Private` —
/// another folder on every server that tells the two apart.
#[test]
fn a_rename_leaves_a_folder_that_differs_only_in_case() {
    let (_dir, mut store, _blobs, account) = setup();
    let work = store.ensure_named_folder(account, "Work").unwrap();
    let mine = store.ensure_named_folder(account, "Work/Clients").unwrap();
    let other = store.ensure_named_folder(account, "work/Private").unwrap();

    store.rename_folder(work, "Jobs").unwrap();

    assert_eq!(path_of(&store, account, work), "Jobs");
    assert_eq!(path_of(&store, account, mine), "Jobs/Clients");
    assert_eq!(path_of(&store, account, other), "work/Private");
}

/// On a `/` server a dot is part of a name: `Receipts.old` is a sibling of
/// `Receipts`, not something filed inside it.
#[test]
fn on_a_slash_server_a_dotted_sibling_is_not_a_child() {
    let (_dir, mut store, blobs, account) = setup();
    let receipts = store.ensure_named_folder(account, "Receipts").unwrap();
    let year = store.ensure_named_folder(account, "Receipts/2026").unwrap();
    let sibling = store.ensure_named_folder(account, "Receipts.old").unwrap();
    surveyed_with(&mut store, account, "/");
    assert!(
        store
            .folders(account)
            .unwrap()
            .iter()
            .all(|f| f.delim.as_deref() == Some("/")),
        "the delimiter reaches the rail with every folder"
    );
    for (n, folder) in [receipts, year, sibling].into_iter().enumerate() {
        let n = n as u32 + 1;
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(n),
                &fixture(&format!("r{n}@x"), "receipt"),
            )
            .unwrap();
    }

    let subtree: Vec<i64> = store
        .folder_subtree(receipts)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(subtree, vec![receipts, year]);
    assert_eq!(store.folder_message_count(receipts).unwrap(), 2);
    assert_eq!(store.mark_folder_seen(receipts, true).unwrap(), 2);

    store.rename_folder(receipts, "Bills").unwrap();
    assert_eq!(path_of(&store, account, year), "Bills/2026");
    assert_eq!(
        path_of(&store, account, sibling),
        "Receipts.old",
        "the server renamed one folder and its children, and so does this"
    );
}

/// The case the audit found: Empty Trash and the bin's expiry expunged a
/// top-level `Trash.2019` on a `/` server, and its mail was missing from All
/// Mail and search besides.
#[test]
fn on_a_slash_server_a_folder_named_after_the_bin_is_not_in_it() {
    let (_dir, mut store, blobs, account) = setup();
    store
        .sync_folders(
            account,
            &[
                ("INBOX".into(), Some("inbox".into())),
                ("Trash".into(), Some("trash".into())),
                ("Trash/Old job".into(), None),
                ("Trash.2019".into(), None),
            ],
        )
        .unwrap();
    surveyed_with(&mut store, account, "/");
    let trash = store.ensure_named_folder(account, "Trash").unwrap();
    let inside = store.ensure_named_folder(account, "Trash/Old job").unwrap();
    let beside = store.ensure_named_folder(account, "Trash.2019").unwrap();
    let mut ingest = |folder: i64, uid: u32, mid: &str, subject: &str| {
        store
            .ingest_raw(
                &blobs,
                account,
                Some(folder),
                Some(uid),
                &fixture(mid, subject),
            )
            .unwrap()
            .message_id
    };
    let binned = ingest(trash, 1, "binned@x", "binned quarterly");
    let filed = ingest(inside, 2, "filed@x", "filed quarterly");
    let kept = ingest(beside, 3, "kept@x", "kept quarterly");

    let mut emptied: Vec<i64> = store
        .trash_contents(account)
        .unwrap()
        .into_iter()
        .map(|(_, _, id)| id)
        .collect();
    emptied.sort();
    assert_eq!(
        emptied,
        [binned, filed],
        "Empty Trash leaves Trash.2019 alone"
    );

    let now = 1_800_000_000_000i64;
    store.refresh_trash_clock(account, now).unwrap();
    let mut due: Vec<i64> = store
        .trash_expired(account, 0, now)
        .unwrap()
        .into_iter()
        .map(|(_, _, id)| id)
        .collect();
    due.sort();
    assert_eq!(due, [binned, filed], "and so does the bin's expiry");

    assert_eq!(subjects(&store, &ListView::AllMail), ["kept quarterly"]);
    let found: Vec<i64> = store
        .search_threads("quarterly", 10)
        .unwrap()
        .into_iter()
        .map(|t| t.thread_id)
        .collect();
    assert_eq!(found.len(), 1, "search finds it like any other folder");
    assert_eq!(Some(found[0]), store.thread_of(kept).unwrap());
}

/// On a `.` server the dot nests and the slash is part of a name.
#[test]
fn on_a_dot_server_the_dot_nests_and_the_slash_does_not() {
    let (_dir, mut store, blobs, account) = setup();
    store
        .sync_folders(
            account,
            &[
                ("INBOX".into(), Some("inbox".into())),
                ("Trash".into(), Some("trash".into())),
                ("Trash.Old job".into(), None),
                ("Trash/2019".into(), None),
            ],
        )
        .unwrap();
    surveyed_with(&mut store, account, ".");
    let inside = store.ensure_named_folder(account, "Trash.Old job").unwrap();
    let beside = store.ensure_named_folder(account, "Trash/2019").unwrap();
    let filed = store
        .ingest_raw(&blobs, account, Some(inside), Some(1), &fixture("f@x", "f"))
        .unwrap()
        .message_id;
    store
        .ingest_raw(&blobs, account, Some(beside), Some(2), &fixture("k@x", "k"))
        .unwrap();

    let emptied: Vec<i64> = store
        .trash_contents(account)
        .unwrap()
        .into_iter()
        .map(|(_, _, id)| id)
        .collect();
    assert_eq!(emptied, [filed]);
}

/// Before any survey has said, both separators nest, exactly as before the
/// delimiter was recorded; and a delimiter that is not one character (a
/// flat server's NIL) is forgotten rather than guessed at.
#[test]
fn until_the_server_has_said_both_separators_nest() {
    let (_dir, mut store, _blobs, account) = setup();
    let receipts = store.ensure_named_folder(account, "Receipts").unwrap();
    let slash = store.ensure_named_folder(account, "Receipts/2026").unwrap();
    let dot = store.ensure_named_folder(account, "Receipts.old").unwrap();
    let all = |store: &Store| -> Vec<i64> {
        let mut ids: Vec<i64> = store
            .folder_subtree(receipts)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(all(&store), vec![receipts, slash, dot]);

    surveyed_with(&mut store, account, "/");
    assert_eq!(all(&store), vec![receipts, slash]);

    let rows: Vec<(String, Option<String>)> = store
        .folders(account)
        .unwrap()
        .into_iter()
        .map(|f| (f.path, None))
        .collect();
    store.set_folder_delimiters(account, &rows).unwrap();
    assert!(
        store
            .folders(account)
            .unwrap()
            .iter()
            .all(|f| f.delim.is_none())
    );
    assert_eq!(all(&store), vec![receipts, slash, dot]);
}

/// The queue and the saved searches follow the rename by the same rule as
/// the folder rows, so none of the three disagrees about what moved.
#[test]
fn the_queue_and_saved_searches_follow_the_servers_delimiter() {
    let (_dir, mut store, blobs, account) = setup();
    let receipts = store.ensure_named_folder(account, "Receipts").unwrap();
    let sibling = store.ensure_named_folder(account, "Receipts.old").unwrap();
    let elsewhere = store.ensure_named_folder(account, "Elsewhere").unwrap();
    surveyed_with(&mut store, account, "/");
    let ingested = store
        .ingest_raw(
            &blobs,
            account,
            Some(sibling),
            Some(9),
            &fixture("q@x", "queued"),
        )
        .unwrap();
    let thread = store
        .thread_of(ingested.message_id)
        .unwrap()
        .unwrap_or(-ingested.message_id);
    store
        .apply_thread_action(
            account,
            thread,
            petrel_engine::actions::ActionKind::Move,
            Some(elsewhere),
            petrel_engine::actions::PlacementPolicy::Exclusive,
        )
        .unwrap();
    let kept = store
        .create_saved_search(account, "Old", "in:\"receipts.old\"")
        .unwrap();
    let child = store
        .create_saved_search(account, "Child", "in:\"receipts/2026\"")
        .unwrap();

    store.rename_folder(receipts, "Bills").unwrap();

    assert_eq!(
        store.pending_actions(account).unwrap()[0].folder_path,
        "Receipts.old",
        "a sibling's queued change still names the sibling"
    );
    let q = |id| store.saved_search(id).unwrap().unwrap().query;
    assert_eq!(q(kept), "in:\"receipts.old\"", "untouched, as written");
    assert_eq!(q(child), "in:bills/2026");
}

/// A leaf is split where the folder nests, so renaming `Clients/Acme` to
/// `Clients/acme.com` on a `/` server makes `in:acme` say `in:acme.com`, not
/// `in:com`.
#[test]
fn a_saved_search_takes_the_whole_leaf_on_a_slash_server() {
    let (_dir, mut store, _blobs, account) = setup();
    let leaf = store
        .create_saved_search(account, "Acme", "in:acme")
        .unwrap();
    store
        .rename_in_saved_searches(
            account,
            &Renamed::Folder {
                from: "Clients/Acme",
                to: "Clients/acme.com",
                separators: &['/'],
            },
        )
        .unwrap();
    assert_eq!(
        store.saved_search(leaf).unwrap().unwrap().query,
        "in:acme.com"
    );
}

/// Search reads a folder's leaf where the folder nests, as the rail and a
/// rename do: on a `/` server `in:old` is not `Receipts.old`.
#[test]
fn search_reads_a_leaf_by_the_servers_delimiter() {
    let (_dir, mut store, blobs, account) = setup();
    let sibling = store.ensure_named_folder(account, "Receipts.old").unwrap();
    let child = store.ensure_named_folder(account, "Receipts/old").unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(sibling),
            Some(1),
            &fixture("s1@x", "sibling note"),
        )
        .unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(child),
            Some(2),
            &fixture("c1@x", "child note"),
        )
        .unwrap();
    let found = |store: &Store, q: &str| -> Vec<String> {
        let mut s: Vec<String> = store
            .search_threads(q, 10)
            .unwrap()
            .into_iter()
            .map(|t| t.subject)
            .collect();
        s.sort();
        s
    };
    assert_eq!(
        found(&store, "in:old note"),
        ["child note", "sibling note"],
        "both, until the server has said"
    );
    surveyed_with(&mut store, account, "/");
    assert_eq!(found(&store, "in:old note"), ["child note"]);
    assert_eq!(found(&store, "in:\"receipts.old\" note"), ["sibling note"]);
}

/// A folder made here takes the account's delimiter, and a survey gives it
/// to any folder it did not list that has none, so no folder of the account
/// nests by both separators while its neighbours nest by one.
#[test]
fn folders_made_here_or_never_listed_nest_like_the_rest() {
    let (_dir, mut store, _blobs, account) = setup();
    let early = store.ensure_named_folder(account, "Imported").unwrap();
    store
        .sync_folders(account, &[("INBOX".into(), Some("inbox".into()))])
        .unwrap();
    store
        .set_folder_delimiters(account, &[("INBOX".into(), Some("/".into()))])
        .unwrap();
    let late = store.ensure_named_folder(account, "Made here").unwrap();
    let delim = |store: &Store, id: i64| {
        store
            .folders(account)
            .unwrap()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap()
            .delim
    };
    assert_eq!(
        delim(&store, early).as_deref(),
        Some("/"),
        "given by the survey"
    );
    assert_eq!(delim(&store, late).as_deref(), Some("/"), "born with it");
    assert!(
        store.folder_awaits_server(late).unwrap(),
        "and still waiting for the server, as every folder made here is"
    );
}
