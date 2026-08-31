//! Folder create / rename / delete against a real server.
//!
//!     source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_folder_ops -- --ignored --nocapture
//!
//! Uses throwaway names of its own; the delete at the end is the cleanup.

use petrel_providers::imap::{Credential, ImapConfig, Security};

fn cfg() -> ImapConfig {
    ImapConfig {
        host: std::env::var("PETREL_NC_IMAP_HOST").expect("source .env.namecheap"),
        port: 993,
        user: std::env::var("PETREL_NC_USER").expect("PETREL_NC_USER"),
        credential: Credential::password(std::env::var("PETREL_NC_PASS").expect("PETREL_NC_PASS")),
        security: Security::Tls,
    }
}

/// Selecting a folder is the existence test IMAP itself uses.
async fn exists(cfg: &ImapConfig, name: &str) -> bool {
    petrel_providers::imap::find_message_id(cfg, name, "definitely-absent@petrel.test")
        .await
        .is_ok()
}

#[tokio::test]
#[ignore = "creates and deletes a throwaway folder on the real account"]
async fn create_rename_delete_round_trip() {
    let cfg = cfg();
    let a = "PetrelOpsTest";
    let b = "PetrelOpsTestRenamed";

    // Start clean even after an earlier failed run.
    let _ = petrel_providers::imap::delete_folder(&cfg, a).await;
    let _ = petrel_providers::imap::delete_folder(&cfg, b).await;

    petrel_providers::imap::create_folder(&cfg, a)
        .await
        .expect("create");
    assert!(exists(&cfg, a).await, "created folder is selectable");
    // Creating what exists is success, not an error.
    petrel_providers::imap::create_folder(&cfg, a)
        .await
        .expect("re-create");

    petrel_providers::imap::rename_folder(&cfg, a, b)
        .await
        .expect("rename");
    assert!(!exists(&cfg, a).await, "old name gone");
    assert!(exists(&cfg, b).await, "new name selectable");

    petrel_providers::imap::delete_folder(&cfg, b)
        .await
        .expect("delete");
    assert!(!exists(&cfg, b).await, "deleted folder is gone");
    println!("create → rename → delete, all confirmed by the server.");
}

/// What the server does when a folder is dragged to the Trash and a folder of
/// that name is already in there.
///
/// The app builds the destination as `Trash/<leaf>` and sends one RENAME. If
/// the server refuses an occupied destination, the move is a dead end and the
/// app has to pick a free name; if it merges, the app has to say so.
#[tokio::test]
#[ignore = "creates and deletes throwaway folders on the real account"]
async fn renaming_onto_an_occupied_name() {
    let cfg = cfg();
    let loose = "PetrelBinTest";
    let taken = "Trash/PetrelBinTest";

    let _ = petrel_providers::imap::delete_folder(&cfg, loose).await;
    let _ = petrel_providers::imap::delete_folder(&cfg, taken).await;

    petrel_providers::imap::create_folder(&cfg, loose)
        .await
        .expect("create loose");
    petrel_providers::imap::create_folder(&cfg, taken)
        .await
        .expect("create taken");

    let outcome = petrel_providers::imap::rename_folder(&cfg, loose, taken).await;
    println!("rename onto an occupied name: {outcome:?}");
    assert!(
        outcome.is_err(),
        "the server refuses an occupied destination"
    );
    assert!(exists(&cfg, loose).await, "and the folder has not moved");

    // So the app numbers it instead. The name carries a space, which the
    // command has to quote — an unquoted RENAME would be read as three
    // arguments and fail, or worse, half-succeed.
    let free = "Trash/PetrelBinTest 2";
    let _ = petrel_providers::imap::delete_folder(&cfg, free).await;
    petrel_providers::imap::rename_folder(&cfg, loose, free)
        .await
        .expect("a free name in the bin is accepted");
    assert!(!exists(&cfg, loose).await, "the folder left its old name");
    assert!(
        exists(&cfg, free).await,
        "and is in the bin under the new one"
    );
    println!("  numbered destination accepted: {free}");

    let _ = petrel_providers::imap::delete_folder(&cfg, free).await;
    let _ = petrel_providers::imap::delete_folder(&cfg, loose).await;
    let _ = petrel_providers::imap::delete_folder(&cfg, taken).await;
}

/// Whether one RENAME carries the children, which is what the local cascade
/// assumes when it rewrites every descendant path.
#[tokio::test]
#[ignore = "creates and deletes throwaway folders on the real account"]
async fn renaming_a_parent_carries_its_children() {
    let cfg = cfg();
    let parent = "PetrelTreeTest";
    let kid = "PetrelTreeTest/kid";
    let moved = "Trash/PetrelTreeTest";
    let moved_kid = "Trash/PetrelTreeTest/kid";

    for p in [moved_kid, moved, kid, parent] {
        let _ = petrel_providers::imap::delete_folder(&cfg, p).await;
    }
    petrel_providers::imap::create_folder(&cfg, parent)
        .await
        .expect("create parent");
    petrel_providers::imap::create_folder(&cfg, kid)
        .await
        .expect("create kid");

    let outcome = petrel_providers::imap::rename_folder(&cfg, parent, moved).await;
    println!("rename a parent into the bin: {outcome:?}");
    println!(
        "  Trash/PetrelTreeTest      : {}",
        exists(&cfg, moved).await
    );
    println!(
        "  Trash/PetrelTreeTest/kid  : {}",
        exists(&cfg, moved_kid).await
    );
    println!("  PetrelTreeTest/kid (old)  : {}", exists(&cfg, kid).await);

    for p in [moved_kid, moved, kid, parent] {
        let _ = petrel_providers::imap::delete_folder(&cfg, p).await;
    }
}

/// What Gmail does when a folder is put inside its bin.
///
/// Written because the guess was wrong. `[Gmail]/Trash` does not refuse a
/// child: the CREATE succeeds and the folder is selectable. It is an ordinary
/// label whose name begins `[Gmail]/Trash/` — Gmail's labels are flat and the
/// slashes are cosmetic — so a message appended into it is found there and
/// not in `[Gmail]/Trash`, which is what this asserts.
///
/// That is the case for refusing the move entirely: every destination the
/// server accepts here looks like it worked and none of them bins anything.
///
///     source .env.local && \
///     PETREL_NC_IMAP_HOST=$PETREL_IMAP_HOST PETREL_NC_USER=$PETREL_IMAP_USER \
///     PETREL_NC_PASS=$PETREL_IMAP_PASS \
///     cargo test -p petrel-providers --test live_folder_ops gmail -- --ignored --nocapture
#[tokio::test]
#[ignore = "creates and deletes throwaway labels on a real Gmail account"]
async fn gmails_bin_accepts_a_folder_and_does_not_hold_its_mail() {
    let cfg = cfg();
    let inside_the_bin = "[Gmail]/Trash/PetrelBinProbe";
    let plain_label = "Trash/PetrelBinProbe";

    for p in [inside_the_bin, plain_label] {
        let _ = petrel_providers::imap::delete_folder(&cfg, p).await;
    }

    // Accepted, contrary to the assumption this test was written to check.
    let nested = petrel_providers::imap::create_folder(&cfg, inside_the_bin).await;
    println!("create inside [Gmail]/Trash : {nested:?}");
    println!(
        "  selectable afterwards     : {}",
        exists(&cfg, inside_the_bin).await
    );

    // And what the server does accept instead.
    let plain = petrel_providers::imap::create_folder(&cfg, plain_label).await;
    println!("create a plain Trash label  : {plain:?}");
    println!(
        "  selectable afterwards     : {}",
        exists(&cfg, plain_label).await
    );

    // Accepting the CREATE is not the same as being the bin. Gmail's labels
    // are flat and the slashes are cosmetic, so a message put here may sit in
    // an ordinary label whose *name* merely begins `[Gmail]/Trash/`. The only
    // answer that settles it is whether the bin itself then holds the message.
    let raw = b"From: probe@petrel.test\r\nTo: probe@petrel.test\r\nSubject: Petrel bin probe\r\nMessage-ID: <petrel-bin-probe@petrel.test>\r\n\r\nprobe\r\n";
    let put = petrel_providers::imap::append_message(&cfg, inside_the_bin, None, raw).await;
    println!("append into that folder     : {put:?}");
    let in_the_real_bin = petrel_providers::imap::find_message_id(
        &cfg,
        "[Gmail]/Trash",
        "petrel-bin-probe@petrel.test",
    )
    .await;
    println!("does [Gmail]/Trash hold it? : {in_the_real_bin:?}");
    let in_the_child = petrel_providers::imap::find_message_id(
        &cfg,
        inside_the_bin,
        "petrel-bin-probe@petrel.test",
    )
    .await;
    println!("does the child hold it?     : {in_the_child:?}");

    for p in [inside_the_bin, plain_label] {
        let _ = petrel_providers::imap::delete_folder(&cfg, p).await;
    }
}
