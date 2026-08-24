//! The draft push protocol, against a real Drafts folder.
//!
//!     source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_draft_push -- --ignored --nocapture

use petrel_providers::imap::{
    ImapConfig, Security, append_message, expunge_uid, uids_for_message_id,
};
use petrel_providers::smtp::Outgoing;

#[tokio::test]
#[ignore = "writes to and cleans up the real Drafts folder"]
async fn a_draft_edit_replaces_its_server_copy() {
    let cfg = ImapConfig {
        host: std::env::var("PETREL_NC_IMAP_HOST").expect("source .env.namecheap"),
        port: 993,
        user: std::env::var("PETREL_NC_USER").unwrap(),
        pass: std::env::var("PETREL_NC_PASS").unwrap(),
        security: Security::Tls,
    };
    let msgid = format!(
        "draft-live-{}@petrel.test",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let draft = |body: &str| Outgoing {
        from_addr: cfg.user.clone(),
        from_name: "Petrel".into(),
        to: vec![],
        cc: vec![],
        subject: "petrel draft push test".into(),
        body_text: body.into(),
        body_html: Some(format!("<p>{body}</p>")),
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };

    // First save.
    let raw = draft("first words").render_with_id(&msgid);
    append_message(&cfg, "Drafts", Some("(\\Draft \\Seen)"), &raw)
        .await
        .expect("first append");
    let first = uids_for_message_id(&cfg, "Drafts", &msgid)
        .await
        .expect("search");
    assert_eq!(first.len(), 1, "one copy after the first push: {first:?}");
    let old_uid = *first.last().unwrap();

    // An edit: append the new revision, then remove the old one — the same
    // order the app uses, so a crash between the two leaves both rather
    // than neither.
    let raw = draft("second thoughts").render_with_id(&msgid);
    append_message(&cfg, "Drafts", Some("(\\Draft \\Seen)"), &raw)
        .await
        .expect("second append");
    let both = uids_for_message_id(&cfg, "Drafts", &msgid)
        .await
        .expect("search");
    assert_eq!(both.len(), 2);
    let new_uid = *both.iter().find(|u| **u != old_uid).unwrap();
    assert!(
        expunge_uid(&cfg, "Drafts", old_uid, true)
            .await
            .expect("expunge old"),
        "old copy removed"
    );
    let after = uids_for_message_id(&cfg, "Drafts", &msgid)
        .await
        .expect("search");
    assert_eq!(after, vec![new_uid], "exactly the new revision remains");

    // Discard: the copy goes too.
    expunge_uid(&cfg, "Drafts", new_uid, true)
        .await
        .expect("cleanup");
    let gone = uids_for_message_id(&cfg, "Drafts", &msgid)
        .await
        .expect("search");
    assert!(gone.is_empty(), "{gone:?}");
    println!("push → edit-replaces → discard, all confirmed by the server.");
}
