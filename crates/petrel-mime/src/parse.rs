//! RFC822 → structured view, for indexing and display.
//!
//! Deliberately total: every field is optional or defaulted, so a message that
//! violates every rule in RFC 5322 still yields *something* storable. Refusing
//! to parse would mean refusing to show the user mail that already sits in
//! their mailbox — the parser's job is to salvage, not to judge.

use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

/// One attachment's metadata. Bytes stay in the raw blob; this records where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size: usize,
    /// `Content-ID`, for inline images referenced as `cid:` in HTML bodies.
    pub content_id: Option<String>,
    pub is_inline: bool,
}

/// The bytes of one attachment, re-read from the raw message.
///
/// Attachment bodies are never stored a second time: the raw message already
/// holds them, and `ParsedMessage` keeps only what the list needs. When one is
/// opened or saved, the part is found by the same index the store recorded at
/// ingest — the position in `attachments()` — and decoded then. A message with
/// a 20MB attachment costs 20MB on disk once, not twice.
pub fn attachment_bytes(raw: &[u8], index: usize) -> Option<(Attachment, Vec<u8>)> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    let part = msg.attachments().nth(index)?;
    let meta = Attachment {
        filename: part.attachment_name().map(|s| s.to_string()),
        content_type: part.content_type().map(|ct| match ct.subtype() {
            Some(sub) => format!("{}/{}", ct.ctype(), sub),
            None => ct.ctype().to_string(),
        }),
        size: part.contents().len(),
        content_id: part.content_id().map(|s| s.to_string()),
        is_inline: part.content_id().is_some(),
    };
    Some((meta, part.contents().to_vec()))
}

/// The way out of a mailing list, as the message itself declares it.
///
/// Read from `List-Unsubscribe` (RFC 2369) and `List-Unsubscribe-Post`
/// (RFC 8058). Offering this in the chrome is the safe path: the header was
/// put there for exactly this, while the "unsubscribe" link at the bottom of
/// the body is a tracked link like every other one in the message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unsubscribe {
    /// An https URL that accepts the RFC 8058 one-click POST — leaving the
    /// list without opening anything.
    pub one_click: Option<String>,
    /// A web page to open when one-click is not offered.
    pub url: Option<String>,
    /// An address to write to when that is all the sender offers.
    pub mailto: Option<String>,
}

impl Unsubscribe {
    pub fn is_empty(&self) -> bool {
        self.one_click.is_none() && self.url.is_none() && self.mailto.is_none()
    }
}

/// Reads the unsubscribe declaration out of a raw message, if any.
pub fn unsubscribe_info(raw: &[u8]) -> Option<Unsubscribe> {
    let msg = MessageParser::default().parse(raw)?;
    let header = msg
        .header_raw("List-Unsubscribe")
        .map(|v| v.trim().to_string())?;

    let mut out = Unsubscribe::default();
    // The value is `<uri>, <uri>` — commas may also appear inside a URI, so
    // split on the angle brackets, not the commas.
    let mut rest = header.as_str();
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        let uri = after[..close].trim();
        let lower = uri.to_ascii_lowercase();
        if lower.starts_with("mailto:") && out.mailto.is_none() {
            out.mailto = Some(uri.to_string());
        } else if (lower.starts_with("https:") || lower.starts_with("http:")) && out.url.is_none() {
            out.url = Some(uri.to_string());
        }
        rest = &after[close + 1..];
    }

    // RFC 8058: the POST target must be https, and the companion header must
    // say the magic words. Anything else is a browser link, not a one-click.
    let one_click_declared = msg
        .header_raw("List-Unsubscribe-Post")
        .map(|v| {
            v.to_ascii_lowercase()
                .contains("list-unsubscribe=one-click")
        })
        .unwrap_or(false);
    if one_click_declared
        && let Some(url) = &out.url
        && url.to_ascii_lowercase().starts_with("https:")
    {
        out.one_click = Some(url.clone());
    }

    if out.is_empty() { None } else { Some(out) }
}

/// A parsed view of a message. Never authoritative — the raw bytes are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedMessage {
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub from_display: Option<String>,
    pub to: Vec<(Option<String>, String)>,
    pub cc: Vec<(Option<String>, String)>,
    pub date_ms: Option<i64>,
    pub body_text: String,
    pub body_html: Option<String>,
    pub attachments: Vec<Attachment>,
    /// `List-Id`, for filter rules and the future Newsletters split. The
    /// angle-bracketed id without its display name: `<news.example.com>`
    /// becomes `news.example.com`.
    pub list_id: Option<String>,
    /// Threading parents, oldest first (References, then In-Reply-To).
    pub references: Vec<String>,
}

impl ParsedMessage {
    /// Text used for full-text indexing: the plain body when present, else a
    /// crude de-tagging of the HTML part so HTML-only mail is still findable.
    pub fn index_text(&self) -> String {
        let raw = if !self.body_text.trim().is_empty() {
            self.body_text.clone()
        } else {
            self.body_html
                .as_deref()
                .map(strip_tags)
                .unwrap_or_default()
        };
        strip_placeholders(&raw)
    }

    /// Every address on the message, for the address table and `from:`/`to:`
    /// search filters.
    pub fn addresses(&self) -> Vec<(&'static str, String, Option<String>)> {
        let mut out = Vec::new();
        if let Some(a) = &self.from_addr {
            out.push(("from", a.to_lowercase(), self.from_display.clone()));
        }
        for (name, addr) in &self.to {
            out.push(("to", addr.to_lowercase(), name.clone()));
        }
        for (name, addr) in &self.cc {
            out.push(("cc", addr.to_lowercase(), name.clone()));
        }
        out
    }
}

/// Removes the image placeholders a plain-text alternative is padded with.
///
/// Every generator that produces a text half of an HTML message leaves a mark
/// where each image was: Gmail writes `[image: Alt Text]`, others `[cid:…]` or
/// a bare `[IMAGE]`. In a marketing message there can be dozens, and they
/// crowd out the words in a search snippet — a result reading
/// "…[image: Google] [image: Search]…" says nothing about why it matched.
///
/// Dropped from the index as well as the snippet, deliberately. Keeping them
/// searchable would mean a query for a company name matching the alt text of
/// its logo in every newsletter it has ever sent, which is not the mail anyone
/// was looking for.
fn strip_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        // Only spans that look like a placeholder. A bracket in prose — and
        // people do write them — must survive.
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let inner = &after[..close];
        let lower = inner.to_ascii_lowercase();
        let is_placeholder = lower.starts_with("image:")
            || lower.starts_with("cid:")
            || lower == "image"
            || lower.starts_with("image ");
        out.push_str(&rest[..open]);
        if !is_placeholder {
            out.push('[');
            out.push_str(inner);
            out.push(']');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    // Placeholders sat between words, so removing them leaves double spaces.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Minimal tag stripper for indexing HTML-only bodies. This is **not** a
/// sanitizer and its output is never rendered — display goes through the
/// allowlist sanitizer and a sandboxed frame instead.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut depth = 0usize;
    let mut in_script = false;
    // Lowercased for the tag checks only; ASCII lowering never changes byte
    // offsets, so the two strings stay index-aligned.
    let lower = html.to_ascii_lowercase();
    let lbytes = lower.as_bytes();
    // Chars with their byte offsets, never a bare byte counter. The previous
    // version walked bytes and sliced the string at each one, which panics
    // the moment the counter lands inside a multibyte character — and mail
    // is full of them. One HTML-only newsletter with an emoji took down the
    // whole ingest thread, and the store lock it held poisoned every pane.
    // The byte-at-a-time push also mangled every non-ASCII character into
    // mojibake in the index, so a search for a name with an accent could
    // never match HTML-only mail; pushing the real char fixes that too.
    for (i, ch) in html.char_indices() {
        if lbytes[i..].starts_with(b"<script") || lbytes[i..].starts_with(b"<style") {
            in_script = true;
        }
        if lbytes[i..].starts_with(b"</script") || lbytes[i..].starts_with(b"</style") {
            in_script = false;
        }
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 && !in_script => out.push(c),
            _ => {}
        }
    }
    // Collapse whitespace so the index isn't full of layout padding.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn addr_list(value: Option<&Address<'_>>) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    let Some(value) = value else { return out };
    for a in value.iter() {
        if let Some(addr) = a.address() {
            out.push((a.name().map(|n| n.to_string()), addr.to_string()));
        }
    }
    out
}

fn id_list(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Parses raw RFC822. Returns `None` only when the bytes yield no message at
/// all; malformed-but-present mail parses with whatever could be salvaged.
pub fn parse_message(raw: &[u8]) -> Option<ParsedMessage> {
    let msg = MessageParser::default().parse(raw)?;

    let from_list = addr_list(msg.from());
    let (from_display, from_addr) = match from_list.first() {
        Some((name, addr)) => (name.clone(), Some(addr.clone())),
        None => (None, None),
    };

    let mut references = id_list(msg.references());
    for id in id_list(msg.in_reply_to()) {
        if !references.contains(&id) {
            references.push(id);
        }
    }

    let attachments = msg
        .attachments()
        .map(|part| Attachment {
            filename: part.attachment_name().map(|s| s.to_string()),
            content_type: part.content_type().map(|ct| match ct.subtype() {
                Some(sub) => format!("{}/{}", ct.ctype(), sub),
                None => ct.ctype().to_string(),
            }),
            size: part.contents().len(),
            content_id: part.content_id().map(|s| s.to_string()),
            // Inline images are the ones HTML bodies reference as `cid:`.
            is_inline: part.content_id().is_some(),
        })
        .collect();

    Some(ParsedMessage {
        message_id: msg.message_id().map(|s| s.to_string()),
        subject: msg.subject().map(|s| s.to_string()),
        from_addr,
        from_display,
        to: addr_list(msg.to()),
        cc: addr_list(msg.cc()),
        date_ms: msg.date().map(|d| d.to_timestamp() * 1000),
        body_text: msg.body_text(0).map(|c| c.to_string()).unwrap_or_default(),
        body_html: msg.body_html(0).map(|c| c.to_string()),
        attachments,
        list_id: msg.header_raw("List-Id").map(|v| {
            let v = v.trim();
            match (v.rfind('<'), v.rfind('>')) {
                (Some(open), Some(close)) if close > open => v[open + 1..close].to_string(),
                _ => v.to_string(),
            }
        }),
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"From: Dana Wu <dana@example.com>\r\n\
To: me@example.com\r\n\
Cc: Sam <sam@example.com>\r\n\
Subject: Q3 vendor contracts\r\n\
Date: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
Message-ID: <abc123@example.com>\r\n\
In-Reply-To: <parent@example.com>\r\n\r\n\
Let's lock pricing before Friday.\r\n";

    #[test]
    fn parses_headers_bodies_and_threading() {
        let m = parse_message(SIMPLE).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("Q3 vendor contracts"));
        assert_eq!(m.from_addr.as_deref(), Some("dana@example.com"));
        assert_eq!(m.from_display.as_deref(), Some("Dana Wu"));
        assert_eq!(m.to.len(), 1);
        assert_eq!(m.cc[0].1, "sam@example.com");
        assert_eq!(m.message_id.as_deref(), Some("abc123@example.com"));
        assert_eq!(m.references, vec!["parent@example.com".to_string()]);
        assert!(m.body_text.contains("lock pricing"));
        assert!(m.date_ms.unwrap() > 1_700_000_000_000);
        assert_eq!(m.addresses().len(), 3);
    }

    #[test]
    fn decodes_encoded_words_and_utf8() {
        let raw = b"From: =?utf-8?B?5p2x5Lqs?= <tokyo@example.jp>\r\n\
Subject: =?utf-8?B?5p2x5Lqs6KiI55S7?=\r\n\r\n\
\xe6\x9d\xb1\xe4\xba\xac\xe8\xa8\x88\xe7\x94\xbb\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("東京計画"));
        assert_eq!(m.from_display.as_deref(), Some("東京"));
        assert!(m.index_text().contains("東京計画"));
    }

    #[test]
    fn the_unsubscribe_header_is_read_in_all_its_forms() {
        let raw = |headers: &str| {
            format!(
                "From: news@sender.example\r\nTo: me@example.com\r\nSubject: weekly\r\n{headers}MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n"
            )
            .into_bytes()
        };

        // Both forms offered, with the RFC 8058 companion: one-click stands.
        let u = unsubscribe_info(&raw(
            "List-Unsubscribe: <mailto:leave@sender.example>, <https://sender.example/u?id=1&x=2>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        ))
        .expect("parsed");
        assert_eq!(
            u.one_click.as_deref(),
            Some("https://sender.example/u?id=1&x=2")
        );
        assert_eq!(u.mailto.as_deref(), Some("mailto:leave@sender.example"));

        // No companion header: the URL is a page to open, not a POST target.
        let u = unsubscribe_info(&raw("List-Unsubscribe: <https://sender.example/unsub>\r\n"))
            .expect("parsed");
        assert!(u.one_click.is_none());
        assert_eq!(u.url.as_deref(), Some("https://sender.example/unsub"));

        // One-click declared over plain http: refused — RFC 8058 says https,
        // and POSTing credentialless over cleartext is not the safe path.
        let u = unsubscribe_info(&raw("List-Unsubscribe: <http://sender.example/unsub>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n"))
        .expect("parsed");
        assert!(u.one_click.is_none());

        // Mailto only — all some senders offer.
        let u = unsubscribe_info(&raw(
            "List-Unsubscribe: <mailto:leave@sender.example?subject=unsubscribe>\r\n",
        ))
        .expect("parsed");
        assert!(u.url.is_none());
        assert!(u.mailto.is_some());

        // No header at all: no affordance, rather than an empty one.
        assert!(unsubscribe_info(&raw("")).is_none());
    }

    #[test]
    fn html_only_mail_full_of_multibyte_text_indexes_without_panicking() {
        // The regression that poisoned the store: an HTML-only body (no text
        // part) whose markup carries emoji, CJK and accented letters. The old
        // stripper walked bytes and sliced at each one — the first multibyte
        // character killed the ingest thread.
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\n\
Subject: newsletter\r\nMIME-Version: 1.0\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n\
<html><head><style>p { color: red; }</style></head>\
<body><p>caf\xc3\xa9 \xf0\x9f\x92\x8c \xe4\xbc\x9a\xe8\xad\xb0<script>var x = 1;</script> after</p></body></html>";
        let parsed = parse_message(raw).expect("parses");
        let text = parsed.index_text();
        // The words survive as themselves — not as mojibake — and the markup,
        // styles and scripts do not.
        assert!(text.contains("caf\u{e9}"), "{text}");
        assert!(text.contains("\u{4f1a}\u{8b70}"), "{text}");
        assert!(text.contains("after"), "{text}");
        assert!(!text.contains("color"), "{text}");
        assert!(!text.contains("var x"), "{text}");
    }

    #[test]
    fn html_only_mail_is_still_indexable() {
        let raw = b"From: a@example.com\r\n\
Subject: html only\r\n\
Content-Type: text/html; charset=utf-8\r\n\r\n\
<html><head><style>p{color:red}</style></head><body>\
<p>Quarterly <b>report</b> attached</p><script>alert(1)</script></body></html>\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(m.body_text.trim().is_empty() || !m.body_text.contains('<'));
        let indexed = m.index_text();
        assert!(indexed.contains("Quarterly"), "got {indexed:?}");
        assert!(indexed.contains("report"));
        // Script and style contents must not pollute the search index.
        assert!(!indexed.contains("alert"), "got {indexed:?}");
        assert!(!indexed.contains("color:red"), "got {indexed:?}");
    }

    #[test]
    fn multipart_with_attachment_and_inline_image() {
        let raw = b"From: a@example.com\r\n\
Subject: with attachment\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=BOUND\r\n\r\n\
--BOUND\r\n\
Content-Type: text/plain\r\n\r\n\
See attached.\r\n\
--BOUND\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"contract.pdf\"\r\n\r\n\
%PDF-1.4 fake\r\n\
--BOUND--\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(m.body_text.contains("See attached"));
        assert_eq!(m.attachments.len(), 1);
        let a = &m.attachments[0];
        assert_eq!(a.filename.as_deref(), Some("contract.pdf"));
        assert_eq!(a.content_type.as_deref(), Some("application/pdf"));
        assert!(a.size > 0);
    }

    #[test]
    fn attachment_bytes_come_back_by_the_same_index_the_list_uses() {
        // Two attachments, so the index has to mean something. Base64 on the
        // second, because that is how real attachments arrive and the bytes
        // handed back must be the decoded ones.
        let raw = b"From: a@example.com\r\n\
Subject: two files\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=B\r\n\r\n\
--B\r\n\
Content-Type: text/plain\r\n\r\n\
Both attached.\r\n\
--B\r\n\
Content-Type: text/csv\r\n\
Content-Disposition: attachment; filename=\"a.csv\"\r\n\r\n\
x,y\r\n1,2\r\n\
--B\r\n\
Content-Type: image/png\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"b.png\"\r\n\r\n\
iVBORw0KGgo=\r\n\
--B--\r\n";
        let listed = parse_message(raw).unwrap().attachments;
        assert_eq!(listed.len(), 2);

        let (meta0, bytes0) = attachment_bytes(raw, 0).unwrap();
        assert_eq!(meta0.filename.as_deref(), listed[0].filename.as_deref());
        assert_eq!(bytes0, b"x,y\r\n1,2");

        let (meta1, bytes1) = attachment_bytes(raw, 1).unwrap();
        assert_eq!(meta1.filename.as_deref(), Some("b.png"));
        // Decoded: the PNG magic, not the base64 text.
        assert_eq!(&bytes1[..4], b"\x89PNG");
        assert_eq!(
            meta1.size,
            bytes1.len(),
            "the listed size is the decoded size"
        );

        assert!(
            attachment_bytes(raw, 2).is_none(),
            "past the end is None, not a panic"
        );
    }

    /// Hostile and broken input must degrade, never panic — this is the engine
    /// consuming bytes chosen by a stranger.
    #[test]
    fn malformed_input_never_panics() {
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"\r\n\r\n".to_vec(),
            b"Subject: no body".to_vec(),
            b"From: <<<>>>\r\nSubject: \x00\x01\x02\r\n\r\nbody".to_vec(),
            b"Content-Type: multipart/mixed; boundary=X\r\n\r\n--X\r\n".to_vec(),
            b"Content-Type: multipart/mixed; boundary=X\r\n\r\n--X\r\nContent-Type: multipart/mixed; boundary=X\r\n\r\n--X\r\n".to_vec(),
            vec![0xFF; 4096],
            b"Subject: =?utf-8?B?bm90LXZhbGlkLWJhc2U2NCEhIQ==?=\r\n\r\nx".to_vec(),
        ];
        for raw in cases {
            // The contract is "does not panic"; a None result is acceptable.
            let _ = parse_message(&raw);
        }
    }

    /// A search snippet full of image placeholders says nothing about why the
    /// message matched. Marketing mail carries dozens of them.
    #[test]
    fn image_placeholders_leave_the_index() {
        let m = ParsedMessage {
            body_text: "[image: Google] Your receipt [image: Search] is attached [cid:part1]"
                .into(),
            ..Default::default()
        };
        let text = m.index_text();
        assert_eq!(text, "Your receipt is attached");
        assert!(!text.contains("image:"), "{text}");
        assert!(!text.contains("cid:"), "{text}");
    }

    /// A bracket in prose is prose. People write them, and a stripper that
    /// eats "[see below]" is worse than the noise it removes.
    #[test]
    fn brackets_in_ordinary_writing_survive() {
        let m = ParsedMessage {
            body_text: "The clause [see section 4] still stands".into(),
            ..Default::default()
        };
        assert_eq!(m.index_text(), "The clause [see section 4] still stands");
    }

    #[test]
    fn an_unclosed_bracket_does_not_eat_the_rest() {
        let m = ParsedMessage {
            body_text: "half [image: open and then more words".into(),
            ..Default::default()
        };
        assert!(m.index_text().contains("more words"));
    }
}
