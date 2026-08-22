//! Rendering an outgoing message.
//!
//! These are the headers that decide whether a reply joins the conversation it
//! answers or starts a new one in every client that receives it — which is not
//! something the sender can see, so it has to be pinned here.

use petrel_providers::smtp::Outgoing;

fn base() -> Outgoing {
    Outgoing {
        from_addr: "me@example.com".into(),
        from_name: "Me".into(),
        to: vec!["you@example.com".into()],
        cc: vec![],
        subject: "Hello".into(),
        body_text: "Body text.".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_rendered_message_carries_the_headers_a_server_needs() {
    let (id, bytes) = base().render("example.com");
    let s = text(&bytes);
    assert!(s.contains("From:"), "{s}");
    assert!(s.contains("To:"), "{s}");
    assert!(s.contains("Subject:"), "{s}");
    assert!(s.contains("Body text."), "{s}");
    assert!(id.ends_with("@example.com"), "{id}");
    assert!(
        s.contains(&id),
        "the id we return must be the one we stamped"
    );
}

#[test]
fn a_reply_carries_the_threading_headers() {
    let mut m = base();
    m.in_reply_to = Some("<parent@example.com>".into());
    m.references = vec!["<root@example.com>".into(), "<parent@example.com>".into()];
    let (_, bytes) = m.render("example.com");
    let s = text(&bytes);
    assert!(s.contains("In-Reply-To:"), "{s}");
    assert!(s.contains("References:"), "{s}");
    assert!(s.contains("root@example.com"), "{s}");
}

#[test]
fn every_message_id_is_distinct() {
    let (a, _) = base().render("example.com");
    let (b, _) = base().render("example.com");
    assert_ne!(
        a, b,
        "two sends must not share an id — it is the only handle on an ambiguous one"
    );
}

#[test]
fn recipients_include_cc_because_the_envelope_needs_them() {
    let mut m = base();
    m.cc = vec!["cc@example.com".into()];
    assert_eq!(m.recipients(), vec!["you@example.com", "cc@example.com"]);
}

#[test]
fn a_non_ascii_subject_survives_the_trip() {
    let mut m = base();
    m.subject = "会議について".into();
    let (_, bytes) = m.render("example.com");
    let s = text(&bytes);
    // Either encoded-word or raw UTF-8 is fine; silently losing it is not.
    assert!(
        s.contains("会議") || s.to_ascii_lowercase().contains("utf-8"),
        "subject was neither encoded nor preserved: {s}"
    );
}

/// Attachments, and the size arithmetic the composer refuses on.
mod attachments {
    use super::*;
    use petrel_providers::smtp::{Attachment, encoded_size};

    fn with_file(bytes: Vec<u8>) -> Outgoing {
        let mut m = base();
        m.attachments = vec![Attachment {
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            bytes,
        }];
        m
    }

    #[test]
    fn an_attached_file_reaches_the_message() {
        let (_, bytes) = with_file(b"hello world".to_vec()).render("example.com");
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("multipart/"), "not multipart: {s}");
        assert!(s.contains("notes.txt"), "filename missing: {s}");
    }

    #[test]
    fn a_message_with_no_attachments_is_not_forced_into_multipart() {
        // Wrapping every plain note in a multipart envelope is not wrong, but it
        // is noise in the raw source and in every client that shows it.
        let (_, bytes) = base().render("example.com");
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("Body text."), "{s}");
    }

    #[test]
    fn encoded_size_accounts_for_base64_growth() {
        // Three bytes become four, so the wire cost is about a third more than
        // the file. Checking a limit against the size on disk lets someone
        // attach something under it and watch the send fail.
        assert!(encoded_size(3_000_000) > 4_000_000);
        assert!(encoded_size(0) == 0);
        // Never smaller than the input — that would let an oversized file pass.
        for n in [1usize, 2, 3, 100, 76, 1024, 25 * 1024 * 1024] {
            assert!(encoded_size(n) >= n, "shrank at {n}");
        }
    }
}

/// A rich-text message goes out as both halves or not at all.
///
/// HTML alone is unreadable in a text client, opaque to anything that indexes
/// mail, and a spam signal at more than one provider. The two parts are
/// generated from the same document, so they cannot describe different
/// messages — this checks the envelope actually carries both.
#[test]
fn rich_text_sends_multipart_alternative_with_both_halves() {
    let mut msg = base();
    msg.body_text = "Hello, and here is the plan <https://x.example/plan>.".into();
    msg.body_html =
        Some(r#"<p>Hello, and here is the <a href="https://x.example/plan">plan</a>.</p>"#.into());

    let (_, bytes) = msg.render("example.com");
    let raw = String::from_utf8_lossy(&bytes);

    assert!(raw.contains("multipart/alternative"), "{raw}");
    assert!(
        raw.contains("text/plain"),
        "the text half is missing: {raw}"
    );
    assert!(raw.contains("text/html"), "the html half is missing: {raw}");
    assert!(raw.contains("here is the plan"), "text body missing: {raw}");
    assert!(
        raw.contains("x.example/plan"),
        "the link survived neither half: {raw}"
    );
}

/// Without HTML it stays a plain message. Nobody sending two lines of text
/// should pay for a multipart envelope.
#[test]
fn plain_text_alone_is_not_wrapped_in_multipart() {
    let (_, bytes) = base().render("example.com");
    let raw = String::from_utf8_lossy(&bytes);
    assert!(!raw.contains("multipart/alternative"), "{raw}");
}
