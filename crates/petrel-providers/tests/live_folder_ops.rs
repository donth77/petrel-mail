//! Folder create / rename / delete against a real server.
//!
//!     source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_folder_ops -- --ignored --nocapture
//!
//! Uses throwaway names of its own; the delete at the end is the cleanup.

use petrel_providers::imap::{ImapConfig, Security};

fn cfg() -> ImapConfig {
    ImapConfig {
        host: std::env::var("PETREL_NC_IMAP_HOST").expect("source .env.namecheap"),
        port: 993,
        user: std::env::var("PETREL_NC_USER").expect("PETREL_NC_USER"),
        pass: std::env::var("PETREL_NC_PASS").expect("PETREL_NC_PASS"),
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
