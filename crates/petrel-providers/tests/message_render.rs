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
        in_reply_to: None,
        references: vec![],
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
