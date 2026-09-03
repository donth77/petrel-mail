//! What a crafted message must not be able to make Petrel send.
//!
//! Two ways in, both proven. A subject encoded as
//! `=?utf-8?q?Hi=0D=0AReply-To:=20attacker?=` decodes with a CRLF in it; the
//! reader prefixes `Re: `, the composer keeps the string, and the reply went
//! out carrying an attacker-authored `Reply-To:` — or, with a second CRLF, an
//! attacker-authored body. An invitation's `SUMMARY:` reaches the same field
//! with no interface in between. And a recipient with a newline in it wrote a
//! second `RCPT TO` line of its own.
//!
//! Alongside those, the two shapes threading ids arrive in and the
//! `Name <addr>` a recipient is usually pasted as.

use petrel_providers::smtp::{Attachment, Outgoing};

fn message() -> Outgoing {
    Outgoing {
        from_addr: "me@example.com".into(),
        from_name: "Me".into(),
        to: vec!["you@example.com".into()],
        cc: vec![],
        subject: "s".into(),
        body_text: "hi".into(),
        body_html: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
    }
}

/// The header block, as text.
fn headers(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Header field names in order, folded lines already joined.
fn field_names(raw: &[u8]) -> Vec<String> {
    headers(raw)
        .lines()
        .filter(|l| !l.starts_with([' ', '\t']))
        .filter_map(|l| l.split_once(':').map(|(name, _)| name.to_ascii_lowercase()))
        .collect()
}

#[test]
fn a_subject_carrying_a_newline_stays_one_subject() {
    let mut m = message();
    m.subject = "Hi\r\nReply-To: attacker@evil.example\r\n\r\nbody of their choosing".into();
    let raw = m.render_with_id("id@example.com");
    let names = field_names(&raw);

    assert_eq!(
        names.iter().filter(|n| *n == "subject").count(),
        1,
        "one subject: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "reply-to"),
        "no header the sender did not ask for: {names:?}"
    );
    // Whatever survives, survives inside the subject: the body is still the
    // one the sender wrote.
    let text = String::from_utf8_lossy(&raw).to_string();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default();
    assert_eq!(body.trim(), "hi", "the body is the sender's:\n{text}");
}

#[test]
fn a_name_and_a_filename_cannot_carry_a_header_either() {
    let mut m = message();
    m.from_name = "Me\r\nBcc: victim@example.com".into();
    m.attachments = vec![Attachment {
        filename: "notes\r\nX-Injected: yes.txt".into(),
        content_type: "text/plain".into(),
        bytes: b"x".to_vec(),
    }];
    let raw = m.render_with_id("id@example.com");
    let text = String::from_utf8_lossy(&raw).to_string();
    let names = field_names(&raw);

    // What is left of the injection is inside a quoted display name and a
    // filename parameter, where it is text rather than a header.
    assert!(!names.iter().any(|n| n == "bcc"), "{names:?}\n{text}");
    assert!(
        !names.iter().any(|n| n == "x-injected"),
        "{names:?}\n{text}"
    );
    assert!(
        text.contains("notesX-Injected: yes.txt"),
        "the filename survives, minus the controls:\n{text}"
    );
    let parsed = petrel_mime::parse_message(&raw).expect("parses");
    assert_eq!(
        parsed.from_display.as_deref(),
        Some("MeBcc: victim@example.com"),
        "the name is one name"
    );
}

#[test]
fn a_recipient_with_a_newline_in_it_never_reaches_the_envelope() {
    let mut m = message();
    m.to = vec![
        "good@example.com".into(),
        "a@b.example>\r\nRCPT TO:<smuggled@evil.example".into(),
    ];
    m.cc = vec!["victim@example.com\r\nDATA".into()];

    assert_eq!(
        m.recipients(),
        vec!["good@example.com".to_string()],
        "only the address that is one"
    );
    let raw = m.render_with_id("id@example.com");
    let text = String::from_utf8_lossy(&raw).to_string();
    assert!(!text.contains("smuggled@evil.example"), "{text}");
    assert!(!text.contains("RCPT TO"), "{text}");
    assert_eq!(
        field_names(&raw).iter().filter(|n| *n == "to").count(),
        1,
        "one To header"
    );
}

/// The shell wraps the ids it passes; the composer passes them bare.
/// mail-builder adds brackets of its own, so one of those used to arrive as
/// `<<id@host>>` — an id that matches nothing and threads with nobody.
#[test]
fn threading_ids_are_bracketed_exactly_once_whichever_way_they_arrive() {
    for (in_reply_to, reference) in [
        ("<parent@example.com>", "<root@example.com>"),
        ("parent@example.com", "root@example.com"),
    ] {
        let mut m = message();
        m.in_reply_to = Some(in_reply_to.into());
        m.references = vec![reference.into(), "<parent@example.com>".into()];
        let raw = m.render_with_id("id@example.com");
        let text = headers(&raw);

        assert!(
            text.contains("In-Reply-To: <parent@example.com>\r\n"),
            "{in_reply_to} rendered as: {text}"
        );
        assert!(
            text.contains("References: <root@example.com> <parent@example.com>"),
            "{reference} rendered as: {text}"
        );
        assert!(!text.contains("<<"), "never doubly wrapped: {text}");

        // And the ids come back out of the message as they went in.
        let parsed = petrel_mime::parse_message(&raw).expect("parses");
        assert_eq!(
            parsed.references,
            vec![
                "root@example.com".to_string(),
                "parent@example.com".to_string()
            ],
            "{text}"
        );
    }
}

#[test]
fn a_pasted_name_and_address_becomes_a_proper_header_and_a_bare_envelope() {
    let mut m = message();
    m.to = vec![
        "Jane Roe <jane@example.com>".into(),
        "\"Doe, John\" <john@example.com>".into(),
    ];
    m.cc = vec!["plain@example.com".into()];

    assert_eq!(
        m.recipients(),
        vec![
            "jane@example.com".to_string(),
            "john@example.com".to_string(),
            "plain@example.com".to_string(),
        ],
        "the envelope names addresses, not the text around them"
    );

    let raw = m.render_with_id("id@example.com");
    let parsed = petrel_mime::parse_message(&raw).expect("parses");
    assert_eq!(
        parsed.to,
        vec![
            (Some("Jane Roe".to_string()), "jane@example.com".to_string()),
            (
                Some("Doe, John".to_string()),
                "john@example.com".to_string()
            ),
        ],
        "two recipients, the comma in the name and all: {}",
        headers(&raw)
    );
}
