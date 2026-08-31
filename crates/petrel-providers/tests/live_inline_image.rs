//! A pasted image, sent through real servers and read back.
//!
//! The unit tests prove the bytes we *render*; this proves what actually
//! arrives once a real submission server and a real delivery chain have had
//! their say — Gmail rewrites nothing, but the only way to know is to look.
//!
//!     source .env.local && source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_inline_image -- --ignored --nocapture
//!
//! Sends from the Gmail test account to the Namecheap one, then fetches the
//! delivered copy over IMAP and walks it with our own parser.

use petrel_providers::imap::{Credential, ImapConfig, Security, fetch_raw, find_message_id};
use petrel_providers::smtp::{Outgoing, SendResult, SmtpConfig, send_tls};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} not set — source the .env files"))
}

/// A 1x1 PNG; tiny, but a real decodable image.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[tokio::test]
#[ignore = "sends real mail between the two test accounts"]
async fn a_pasted_image_survives_real_delivery() {
    use base64::Engine as _;

    let from = env("PETREL_IMAP_USER");
    let from_pass = env("PETREL_IMAP_PASS");
    let from_imap = env("PETREL_IMAP_HOST");
    let to = env("PETREL_NC_USER");
    let to_pass = env("PETREL_NC_PASS");
    let to_imap = env("PETREL_NC_IMAP_HOST");

    let data_uri = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(PNG)
    );
    let msg = Outgoing {
        from_addr: from.clone(),
        from_name: "Petrel Live Test".into(),
        to: vec![to.clone()],
        cc: vec![],
        subject: "Petrel: pasted image, end to end".into(),
        body_text: "One pasted pixel below.\n[image]".into(),
        body_html: Some(format!(
            r#"<p>One pasted pixel below.</p><img src="{data_uri}">"#
        )),
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    };
    let domain = from.split_once('@').map(|(_, d)| d).unwrap_or("localhost");
    let (message_id, raw) = msg.render(domain);
    assert!(
        !String::from_utf8_lossy(&raw).contains("data:image/"),
        "a data: URI leaked to the wire"
    );

    let smtp = SmtpConfig::for_imap_host(&from_imap, &from, &from_pass);
    match send_tls(&smtp, &msg, &raw).await {
        SendResult::Committed { response } => println!("committed: {}", response.trim()),
        other => panic!("send did not commit: {other:?}"),
    }

    // Delivery is not instant; ask the recipient until it lands.
    let rx = ImapConfig {
        host: to_imap,
        port: 993,
        user: to,
        credential: Credential::password(to_pass),
        security: Security::Tls,
    };
    let mut seq: Option<u32> = None;
    for _ in 0..24 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let found = find_message_id(&rx, "INBOX", &message_id)
            .await
            .expect("recipient search");
        if let Some(s) = found.last() {
            seq = Some(*s);
            break;
        }
        println!("not delivered yet…");
    }
    let seq = seq.expect("message never arrived within two minutes");

    // Fetch the tail of the inbox and pick ours out by Message-ID — the raw
    // fetch is by recency, and the search told us it is in there.
    let recent = fetch_raw(&rx, "INBOX", 5).await.expect("fetch");
    let delivered = recent
        .iter()
        .map(|(_, raw)| raw)
        .find(|raw| {
            petrel_mime::parse_message(raw)
                .and_then(|p| p.message_id)
                .map(|id| {
                    message_id.contains(&id) || id.contains(message_id.trim_matches(['<', '>']))
                })
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("seq {seq} found but not in the last 5 fetched"));

    // Now the reader's own walk, on what a real chain delivered.
    let parsed = petrel_mime::parse_message(delivered).expect("parses");
    let html = parsed.body_html.expect("html half survived");
    assert!(html.contains("cid:"), "inline reference lost: {html}");
    assert_eq!(parsed.attachments.len(), 1, "{:?}", parsed.attachments);
    assert!(parsed.attachments[0].is_inline);

    let sanitized = petrel_mime::sanitize_html(&html, false);
    let resolved = petrel_mime::resolve_cids(&sanitized.html, &parsed.attachments, |i| {
        format!("/attachment/tok/{i}")
    });
    assert!(
        resolved.contains("/attachment/tok/0") && !resolved.contains("cid:"),
        "reader could not resolve the delivered cid: {resolved}"
    );

    let (_, bytes) = petrel_mime::attachment_bytes(delivered, 0).expect("part bytes");
    assert_eq!(bytes, PNG, "image bytes changed in transit");
    println!("delivered, resolved, byte-identical.");
}
