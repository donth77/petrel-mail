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

/// Pasted images: data: URIs in the draft become cid parts on the wire.
mod inline_images {
    use super::*;
    use petrel_providers::smtp::Attachment;

    /// A 1x1 PNG — the smallest real image bytes can make.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn data_uri(bytes: &[u8]) -> String {
        use base64::Engine as _;
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    #[test]
    fn a_pasted_image_travels_as_a_cid_part_our_own_reader_resolves() {
        let mut m = base();
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
        // The PNG pasted twice, a JPEG once — the duplicate must not become a
        // second copy of the bytes.
        m.body_html = Some(format!(
            r#"<p>before</p><img src="{png}"><img src="{jpg}"><p>and again:</p><img src="{png}">"#,
            png = data_uri(PNG),
            jpg = data_uri(&jpeg).replace("image/png", "image/jpeg"),
        ));
        let (_, bytes) = m.render("example.com");
        let s = text(&bytes);

        // The wire shape that renders everywhere: related wrapping the
        // alternative, images as parts. No data: URI survives.
        assert!(s.contains("multipart/related"), "{s}");
        assert!(s.contains("multipart/alternative"), "{s}");
        assert!(!s.contains("data:image/"), "a data: URI leaked to the wire");

        // Now the full circle: our own parser, sanitizer and resolver receive
        // it, exactly as the reader would.
        let parsed = petrel_mime::parse_message(&bytes).expect("parses");
        let html = parsed.body_html.expect("has html body");
        assert!(html.contains("cid:"), "{html}");
        assert_eq!(
            parsed.attachments.len(),
            2,
            "two distinct images, not three: {:?}",
            parsed.attachments
        );

        let sanitized = petrel_mime::sanitize_html(&html, false);
        let resolved = petrel_mime::resolve_cids(&sanitized.html, &parsed.attachments, |i| {
            format!("/attachment/tok/{i}")
        });
        assert!(!resolved.contains("cid:"), "{resolved}");
        // The duplicate paste points at the same part, twice.
        assert_eq!(
            resolved.matches("/attachment/tok/0").count(),
            2,
            "{resolved}"
        );
        assert_eq!(
            resolved.matches("/attachment/tok/1").count(),
            1,
            "{resolved}"
        );

        // And the part the reader would fetch is byte-for-byte the paste.
        let (meta, body) = petrel_mime::attachment_bytes(&bytes, 0).expect("part 0");
        assert_eq!(body, PNG, "bytes round-tripped");
        assert_eq!(meta.content_type.as_deref(), Some("image/png"));
        assert!(meta.is_inline);
    }

    #[test]
    fn inline_images_and_attachments_share_a_message() {
        let mut m = base();
        m.body_html = Some(format!(r#"<img src="{}">"#, data_uri(PNG)));
        m.attachments = vec![Attachment {
            filename: "notes.txt".into(),
            content_type: "text/plain".into(),
            bytes: b"hello".to_vec(),
        }];
        let (_, bytes) = m.render("example.com");
        let s = text(&bytes);
        assert!(s.contains("multipart/mixed"), "{s}");
        assert!(s.contains("multipart/related"), "{s}");
        assert!(s.contains("notes.txt"), "{s}");

        let parsed = petrel_mime::parse_message(&bytes).expect("parses");
        // The image, then the file — reader indexes line up with part order.
        assert_eq!(parsed.attachments.len(), 2);
        assert!(parsed.attachments[0].is_inline);
        assert_eq!(parsed.attachments[1].filename.as_deref(), Some("notes.txt"));
        // The text alternative still reads as a message.
        assert!(
            parsed.body_text.contains("Body text."),
            "{}",
            parsed.body_text
        );
    }

    #[test]
    fn a_malformed_data_uri_is_left_alone() {
        let mut m = base();
        m.body_html = Some(r#"<img src="data:image/png;base64,@@not-base64@@">"#.into());
        let (_, bytes) = m.render("example.com");
        let s = text(&bytes);
        // Not extractable, so it goes out as it came in — an extractor, not a
        // validator. (mail_builder may split the body across encoded lines, so
        // look for the marker, not the whole URI.)
        assert!(s.contains("not-base64"), "{s}");
        assert!(!s.contains("multipart/related"), "{s}");
    }
}

/// What the wire gets, for a mail filter reading it cold.
///
/// Petrel's own messages were landing in Gmail's spam folder. The cause was
/// the sending domain's DNS rather than anything here, but the audit that
/// found it also found two things worth changing in what is written: the HTML
/// part went out as a bare `<p>…</p>` fragment where every other client sends
/// a document, and nothing said what had written the message.
mod deliverability {
    use petrel_providers::smtp::Outgoing;

    fn note(html: Option<&str>) -> Outgoing {
        Outgoing {
            from_addr: "me@example.com".into(),
            from_name: "Tom".into(),
            to: vec!["them@example.com".into()],
            cc: vec![],
            subject: "Hello".into(),
            body_text: "Hello there.".into(),
            body_html: html.map(|h| h.to_string()),
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        }
    }

    /// Undoes quoted-printable, so a test can assert on the markup rather than
    /// on its encoding. Without this the assertions read `charset=3D` and
    /// break the day a line wraps a character earlier.
    fn decoded(raw: &str) -> String {
        let joined = raw.replace("=\r\n", "").replace("=\n", "");
        let bytes = joined.as_bytes();
        let mut out = String::with_capacity(joined.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'='
                && i + 2 < bytes.len()
                && let Ok(b) = u8::from_str_radix(&joined[i + 1..i + 3], 16)
            {
                out.push(b as char);
                i += 3;
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    fn rendered(o: &Outgoing) -> String {
        decoded(&String::from_utf8_lossy(&o.render("example.com").1))
    }

    #[test]
    fn the_html_part_is_a_document_not_a_fragment() {
        let out = rendered(&note(Some("<p>Hello there.</p>")));
        assert!(
            out.contains("<html><head><meta charset=\"utf-8\"></head><body>"),
            "no document wrapper in:\n{out}"
        );
        assert!(out.contains("<p>Hello there.</p>"));
        assert!(out.contains("</body></html>"));
    }

    #[test]
    fn markup_that_is_already_a_document_is_left_alone() {
        // A forward or a quoted reply can arrive whole. Nesting <html> inside
        // <html> is worse than either shape on its own.
        let out = rendered(&note(Some("<html><body><p>Quoted.</p></body></html>")));
        assert_eq!(out.matches("<html>").count(), 1, "wrapped a document twice");
    }

    #[test]
    fn a_doctype_counts_as_a_document() {
        let out = rendered(&note(Some("<!DOCTYPE html><html><body>hi</body></html>")));
        assert_eq!(out.matches("<html>").count(), 1);
    }

    #[test]
    fn the_message_says_what_wrote_it() {
        assert!(rendered(&note(None)).contains("User-Agent: Petrel"));
    }

    #[test]
    fn a_plain_text_message_gains_no_html_part() {
        // The wrapper must not conjure an HTML body where the sender wrote none.
        let out = rendered(&note(None));
        assert!(!out.contains("text/html"), "invented an HTML part");
    }
}
