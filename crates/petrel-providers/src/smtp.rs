//! SMTP submission — M0 slice.
//!
//! Deliberately minimal and hand-rolled for the spike: the interesting part is
//! not the protocol (RFC 5321 submission is small) but *classifying failures*.
//! Where a send fails decides whether a retry is safe, so this reports the
//! failure point, never a bare error.

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Where in the conversation a send stopped. The caller maps this to an
/// `AttemptOutcome`; only `Committed` means the server acknowledged the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// Server returned 2xx after the terminating dot. Definitively delivered.
    Committed { response: String },
    /// Failed before the body could be committed (connect, EHLO, MAIL, RCPT,
    /// or the DATA go-ahead). A retry cannot duplicate.
    FailedBeforeCommit { stage: &'static str, detail: String },
    /// Body was fully transmitted; no acknowledgement was read back. Ambiguous —
    /// the server may or may not have committed it.
    UnknownAfterTransmit { detail: String },
    /// Server refused permanently (5xx).
    RejectedPermanently { response: String },
}

async fn read_reply<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> std::io::Result<String> {
    // SMTP multiline replies: "250-..." continues, "250 ..." ends.
    let mut out = String::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed mid-reply",
            ));
        }
        out.push_str(&line);
        let bytes = line.as_bytes();
        if bytes.len() >= 4 && bytes[3] == b' ' {
            break;
        }
        if bytes.len() < 4 {
            break;
        }
    }
    Ok(out.trim_end().to_string())
}

fn code_of(reply: &str) -> u16 {
    reply.get(0..3).and_then(|c| c.parse().ok()).unwrap_or(0)
}

/// A file travelling with a message.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The Content-Type an attachment goes out under.
///
/// A calendar file says what it is for in its own METHOD line, and iMIP
/// (RFC 6047) wants the same word on the MIME part as a `method` parameter:
/// it is what calendar systems read to tell an invitation from a reply or a
/// cancellation, and Exchange treats a `text/calendar` part without one as an
/// ordinary file. The charset is stated for the same reason: iCalendar is
/// UTF-8 by definition, and a text part with no charset is read as US-ASCII.
fn attachment_content_type(a: &Attachment) -> mail_builder::headers::content_type::ContentType<'_> {
    use mail_builder::headers::content_type::ContentType;
    let mut ct = ContentType::new(a.content_type.as_str());
    if a.content_type.eq_ignore_ascii_case("text/calendar") {
        if let Some(method) = ical_method(&a.bytes) {
            ct = ct.attribute("method", method);
        }
        ct = ct.attribute("charset", "utf-8");
    }
    ct
}

/// The METHOD of an iCalendar object: REQUEST, REPLY, CANCEL and so on.
fn ical_method(bytes: &[u8]) -> Option<String> {
    let method = std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .find_map(|l| l.strip_prefix("METHOD:"))?
        .trim()
        .to_ascii_uppercase();
    let plain = !method.is_empty()
        && method
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    plain.then_some(method)
}

/// Roughly what a file costs on the wire.
///
/// base64 turns three bytes into four and wraps every 76 characters, so a file
/// arrives about 37% larger than it is on disk. A limit checked against the
/// size in Finder lets people attach something that is under it and watch the
/// send fail — the check has to be against what is actually transmitted.
pub fn encoded_size(raw_len: usize) -> usize {
    let base64 = raw_len.div_ceil(3) * 4;
    base64 + base64.div_ceil(76) * 2
}

/// One pasted image, split out of the HTML for the wire.
struct InlineImage {
    mime: String,
    cid: String,
    bytes: Vec<u8>,
}

/// Splits pasted images out of the HTML so the wire never carries a data: URI.
///
/// The composer embeds a pasted screenshot as `src="data:image/png;base64,…"`,
/// which is the right form for a draft — it survives save and reload with no
/// file lifecycle at all — and the wrong form for the wire: receiving clients
/// strip data: URIs as a spoofing vector, ours included. On the way out each
/// one becomes a MIME part of its own, referenced as `cid:`, which is the form
/// every client renders.
///
/// The same image pasted twice becomes one part referenced twice. Anything
/// that is not a well-formed base64 image data URI is left exactly as it was —
/// this is an extractor, not a validator.
fn extract_inline_images(html: &str, seed: &str) -> (String, Vec<InlineImage>) {
    use base64::Engine as _;
    const NEEDLE: &str = "src=\"data:image/";

    let mut out = String::with_capacity(html.len());
    let mut parts: Vec<InlineImage> = Vec::new();
    // Payload string -> index in `parts`, for the pasted-twice case.
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut rest = html;

    while let Some(pos) = rest.find(NEEDLE) {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        // The attribute runs to the closing quote; a data: URI contains none.
        let Some(close) = after["src=\"".len()..].find('"') else {
            // Unterminated attribute — pass the tail through untouched.
            out.push_str(after);
            rest = "";
            break;
        };
        let uri = &after["src=\"".len().."src=\"".len() + close];
        let advance = "src=\"".len() + close + 1;

        // data:image/png;base64,AAAA — anything else passes through as-is.
        let parsed = uri
            .strip_prefix("data:")
            .and_then(|u| u.split_once(";base64,"))
            .and_then(|(mime, payload)| {
                base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .ok()
                    .map(|bytes| (mime.to_string(), payload, bytes))
            });
        match parsed {
            Some((mime, payload, bytes)) => {
                let index = match seen.iter().find(|(p, _)| p == payload) {
                    Some((_, i)) => *i,
                    None => {
                        let cid = format!("img{}.{}", parts.len(), seed);
                        parts.push(InlineImage { mime, cid, bytes });
                        seen.push((payload.to_string(), parts.len() - 1));
                        parts.len() - 1
                    }
                };
                out.push_str(&format!("src=\"cid:{}\"", parts[index].cid));
            }
            None => out.push_str(&after[..advance]),
        }
        rest = &after[advance..];
    }
    out.push_str(rest);
    (out, parts)
}

/// What to send, before it becomes bytes.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub from_addr: String,
    pub from_name: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body_text: String,
    /// The rich-text half, when there is one.
    ///
    /// Both parts always go out together as `multipart/alternative` — never
    /// HTML alone. A message with no text alternative is unreadable in a text
    /// client, illegible to anything that indexes mail, and treated as a spam
    /// signal by more than one provider. The text is generated from the same
    /// document as the HTML, so the two cannot describe different messages.
    pub body_html: Option<String>,
    /// Set when replying, so the thread survives at the other end. Threading is
    /// a property of these headers, not of the subject line, and a reply that
    /// omits them starts a new conversation in every client that receives it.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<Attachment>,
}

/// A header value with everything that could end the line taken out.
///
/// A header *is* a line: a CR or LF inside a value ends it early, and whatever
/// follows becomes a header of somebody else's choosing. mail-builder writes
/// most values through verbatim, so a subject that decoded from
/// `=?utf-8?q?Hi=0D=0AReply-To:=20attacker?=` and was carried into a reply
/// went out with the attacker's own `Reply-To:` on it — and with two of them,
/// their own body. Every other control character goes too: none belongs in a
/// header, and each is a chance for something downstream to disagree about
/// where the line ends.
///
/// This is the last line rather than the only one. The shell scrubs at its own
/// entry points; this is what makes it true of anything that reaches the wire.
fn clean_header(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// A Message-ID with its angle brackets off.
///
/// mail-builder writes its own, so an id that arrives wrapped comes out
/// `<<id@host>>` — which matches nothing, and threads with nobody. Both shapes
/// arrive: the shell wraps its ids and the composer passes them bare.
fn bare_id(id: &str) -> String {
    clean_header(id)
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

/// One recipient, split into what the header shows and where the mail goes.
struct Recipient {
    display: Option<String>,
    address: String,
}

/// Whether an address can be written into `RCPT TO:<…>` as it stands.
///
/// Everything refused here would either end the command early or start a
/// second one: whitespace splits it, an angle bracket closes the envelope, a
/// comma or semicolon reads as a list, and a control character begins a new
/// line. Such a recipient is dropped rather than escaped — the string was
/// never an address, and inventing what the sender meant is worse than
/// sending to the people who were named properly.
fn envelope_safe(address: &str) -> bool {
    !address.is_empty()
        && address.contains('@')
        && !address
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',' | ';'))
}

/// Splits a recipient as typed into a display name and an addr-spec.
///
/// `Jane <jane@example.com>`, `"Doe, Jane" <jane@example.com>` and a bare
/// `jane@example.com` are all written by hand into the same field. The header
/// gets both halves, so mail-builder can quote the name properly; the envelope
/// gets the address alone, because `RCPT TO:<Jane <jane@example.com>>` is not
/// an address and every server refuses it — which is how a pasted
/// `Name <addr>` came back as "rejected".
fn parse_recipient(raw: &str) -> Option<Recipient> {
    let trimmed = raw.trim();
    // A control character *inside* a recipient means it was never one address:
    // it is two lines, and the second was going to be a command. Splicing the
    // halves into `victim@example.comDATA` would only send the message to a
    // name nobody has, so the whole entry goes.
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    let (display, address) = match trimmed.rfind('<') {
        Some(open) if trimmed.ends_with('>') => {
            (&trimmed[..open], &trimmed[open + 1..trimmed.len() - 1])
        }
        _ => ("", trimmed),
    };
    let address = address.trim();
    if !envelope_safe(address) {
        return None;
    }
    let display = display.trim().trim_matches('"').trim();
    Some(Recipient {
        display: (!display.is_empty()).then(|| display.to_string()),
        address: address.to_string(),
    })
}

fn parse_recipients(list: &[String]) -> Vec<Recipient> {
    list.iter().filter_map(|r| parse_recipient(r)).collect()
}

/// The entries `parse_recipients` would leave out, so a caller can refuse
/// the whole message rather than send it to everyone else. A half-typed
/// address dropped in silence is worse than a message that does not go:
/// the person believes it reached someone it never did. Blank entries are
/// not a problem, only a trailing comma.
pub fn unsendable_recipients(list: &[String]) -> Vec<String> {
    list.iter()
        .filter(|r| !r.trim().is_empty() && parse_recipient(r).is_none())
        .cloned()
        .collect()
}

/// The pairs as one address-list header.
fn address_list(list: &[Recipient]) -> mail_builder::headers::address::Address<'_> {
    use mail_builder::headers::address::Address;
    Address::new_list(
        list.iter()
            .map(|r| Address::new_address(r.display.as_deref(), r.address.as_str()))
            .collect(),
    )
}

/// Wraps an HTML body in a document, if it is not one already.
///
/// The composer produces a fragment — `<p>…</p>`, the contenteditable's own
/// markup — and that is what went on the wire. Every other mail client sends a
/// complete document, so a bare fragment is unusual enough for some filters to
/// count against it, and it leaves the charset for the receiving client to
/// guess from the MIME header alone rather than finding it in the markup too.
///
/// Left alone if it already looks like a document: a forwarded or quoted
/// message may arrive as one, and nesting `<html>` inside `<html>` is worse
/// than either.
fn as_document(html: &str) -> String {
    let head = html.trim_start();
    // Byte slices, not `len() >= n && &head[..n]`: a composer fragment is
    // `<p>` plus CJK, and index 5 sits in the middle of a character.
    let looks_whole = head
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("<html"))
        || head
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("<!doctype"));
    if looks_whole {
        return html.to_string();
    }
    format!("<html><head><meta charset=\"utf-8\"></head><body>{html}</body></html>")
}

impl Outgoing {
    /// Renders to RFC 5322 bytes, and returns the Message-ID it stamped.
    ///
    /// The id is returned rather than looked up afterwards because it is the
    /// only handle on a message whose send outcome was ambiguous: if the
    /// connection dies after the body, this is what a later search asks the
    /// server about (spike S5).
    pub fn render(&self, domain: &str) -> (String, Vec<u8>) {
        let message_id = format!(
            "{:x}.{}@{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            std::process::id(),
            domain
        );
        let bytes = self.render_with_id(&message_id);
        (message_id, bytes)
    }

    /// Renders under a caller-chosen Message-ID.
    ///
    /// A draft travels under one name for life: every autosave pushed to the
    /// server carries the same id, which is what makes the copy an edit of
    /// the previous one — and what makes it, fetched back by folder sync,
    /// dedupe onto the local draft instead of appearing beside it.
    pub fn render_with_id(&self, message_id: &str) -> Vec<u8> {
        use mail_builder::MessageBuilder;

        // Every header value is scrubbed before it is handed over, and the
        // scrubbed copies live until the message is written: mail-builder
        // borrows what it is given.
        let from_name = clean_header(&self.from_name);
        let from_addr = clean_header(&self.from_addr);
        let subject = clean_header(&self.subject);
        let message_id = bare_id(message_id);
        let to = parse_recipients(&self.to);
        let cc = parse_recipients(&self.cc);
        let in_reply_to = self.in_reply_to.as_deref().map(bare_id);
        let references: Vec<String> = self
            .references
            .iter()
            .map(|r| bare_id(r))
            .filter(|r| !r.is_empty())
            .collect();
        let filenames: Vec<String> = self
            .attachments
            .iter()
            .map(|a| clean_header(&a.filename))
            .collect();

        let mut b = MessageBuilder::new()
            .from((from_name.as_str(), from_addr.as_str()))
            .subject(subject.as_str())
            .message_id(message_id.as_str())
            // Named, but not versioned. Every ordinary client says what wrote
            // the message and mail carrying no such header is slightly the
            // odder thing; a version number would only tell a stranger which
            // build the sender is running.
            .header("User-Agent", mail_builder::headers::raw::Raw::new("Petrel"));

        // Pasted images ride the draft as data: URIs; the wire gets them as
        // parts of their own, referenced by cid. Seeded from the Message-ID so
        // the ids are as unique as the message they belong to.
        // Wrapped after the inline-image rewrite, so the cid substitution sees
        // the markup the composer produced rather than a document it did not.
        let (html, inline) = match &self.body_html {
            Some(html) => {
                let (rewritten, inline) = extract_inline_images(html, &message_id);
                (Some(as_document(&rewritten)), inline)
            }
            None => (None, Vec::new()),
        };

        if inline.is_empty() {
            // mail_builder assembles the tree: text plus html becomes
            // multipart/alternative, and attachments wrap that in
            // multipart/mixed.
            b = b.text_body(self.body_text.as_str());
            if let Some(html) = &html {
                b = b.html_body(html.as_str());
            }
            for (index, a) in self.attachments.iter().enumerate() {
                b = b.attachment(
                    attachment_content_type(a),
                    filenames[index].as_str(),
                    a.bytes.as_slice(),
                );
            }
        } else {
            // With inline images the tree is assembled by hand, because
            // mail_builder's automatic shape puts every extra part in
            // multipart/mixed — where clients list an image as an attachment
            // instead of rendering it in the body. The shape that renders
            // everywhere is the one Thunderbird writes:
            //
            //   multipart/mixed            (only when attachments exist)
            //     multipart/related
            //       multipart/alternative  (text, then html)
            //       image parts            (inline, each with its cid)
            //     attachment parts
            use mail_builder::mime::MimePart;
            let html = html.as_deref().unwrap_or_default();
            let mut related = Vec::with_capacity(inline.len() + 1);
            related.push(MimePart::new(
                "multipart/alternative",
                vec![
                    MimePart::new("text/plain", self.body_text.as_str()),
                    MimePart::new("text/html", html),
                ],
            ));
            for img in &inline {
                related.push(
                    MimePart::new(img.mime.as_str(), img.bytes.as_slice())
                        .inline()
                        .cid(img.cid.as_str()),
                );
            }
            let core = MimePart::new("multipart/related", related);
            b = b.body(if self.attachments.is_empty() {
                core
            } else {
                let mut mixed = Vec::with_capacity(self.attachments.len() + 1);
                mixed.push(core);
                for (index, a) in self.attachments.iter().enumerate() {
                    mixed.push(
                        MimePart::new(attachment_content_type(a), a.bytes.as_slice())
                            .attachment(filenames[index].as_str()),
                    );
                }
                MimePart::new("multipart/mixed", mixed)
            });
        }

        // One header per field. mail-builder appends a line on every call, and
        // mail-parser keeps only the last — so a per-address loop drops every
        // recipient but one after Sent-folder ingest.
        if !to.is_empty() {
            b = b.to(address_list(&to));
        }
        if !cc.is_empty() {
            b = b.cc(address_list(&cc));
        }
        if let Some(parent) = in_reply_to.as_deref().filter(|p| !p.is_empty()) {
            b = b.in_reply_to(parent);
        }
        if !references.is_empty() {
            b = b.references(references.iter().map(|r| r.as_str()).collect::<Vec<_>>());
        }

        b.write_to_vec().unwrap_or_default()
    }

    /// Every address the envelope has to name.
    ///
    /// Addr-specs only, and only ones that can go on the wire as they stand.
    /// A recipient whose text is not an address is dropped here rather than
    /// escaped; a message with nobody left to send to is refused by the caller
    /// rather than sent into the void.
    /// Entries in To or Cc that the envelope would drop; see
    /// `unsendable_recipients`.
    pub fn unsendable(&self) -> Vec<String> {
        let mut all: Vec<String> = self.to.clone();
        all.extend(self.cc.iter().cloned());
        unsendable_recipients(&all)
    }

    pub fn recipients(&self) -> Vec<String> {
        self.to
            .iter()
            .chain(self.cc.iter())
            .filter_map(|raw| parse_recipient(raw).map(|r| r.address))
            .collect()
    }

    /// The address the envelope says the mail is from, if it can be sent as
    /// one. `MAIL FROM` is a command like any other: a newline in it would be
    /// the start of a second.
    pub fn sender(&self) -> Option<String> {
        parse_recipient(&self.from_addr).map(|r| r.address)
    }
}

/// Sends one message over a plaintext connection (loopback tests only — the
/// shipping path adds TLS; see the crate's `insecure-plaintext` feature).
pub async fn send_plaintext(
    host: &str,
    port: u16,
    from: &str,
    to: &str,
    raw_message: &[u8],
) -> SendResult {
    macro_rules! fail_before {
        ($stage:expr, $e:expr) => {
            return SendResult::FailedBeforeCommit {
                stage: $stage,
                detail: $e.to_string(),
            }
        };
    }

    let stream = match TcpStream::connect((host, port)).await {
        Ok(s) => s,
        Err(e) => fail_before!("connect", e),
    };
    let (rx, mut tx) = stream.into_split();
    let mut reader = BufReader::new(rx);

    // Greeting
    match tokio::time::timeout(reply_timeout(), read_reply(&mut reader)).await {
        Ok(Ok(r)) if code_of(&r) == 220 => {}
        Ok(Ok(r)) => fail_before!("greeting", r),
        Ok(Err(e)) => fail_before!("greeting", e),
        Err(_) => fail_before!("greeting", "timed out waiting for the server"),
    }

    // Command/response pairs up to the DATA go-ahead.
    for (stage, cmd, expect) in [
        ("ehlo", "EHLO petrel.test\r\n".to_string(), 250u16),
        ("mail", format!("MAIL FROM:<{from}>\r\n"), 250),
        ("rcpt", format!("RCPT TO:<{to}>\r\n"), 250),
        ("data", "DATA\r\n".to_string(), 354),
    ] {
        if let Err(e) = tx.write_all(cmd.as_bytes()).await {
            fail_before!(stage, e);
        }
        match tokio::time::timeout(reply_timeout(), read_reply(&mut reader)).await {
            Ok(Ok(r)) if code_of(&r) == expect => {}
            Ok(Ok(r)) if (500..600).contains(&code_of(&r)) => {
                return SendResult::RejectedPermanently { response: r };
            }
            Ok(Ok(r)) => fail_before!(stage, r),
            Ok(Err(e)) => fail_before!(stage, e),
            Err(_) => fail_before!(stage, "timed out waiting for the server"),
        }
    }

    // Body + terminating dot. Everything past this point is the danger zone:
    // the server may commit at any moment, so a write or read failure here is
    // ambiguous by definition — never "failed".
    let mut body = raw_message.to_vec();
    if !body.ends_with(b"\r\n") {
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b".\r\n");

    if let Err(e) = tx.write_all(&body).await {
        return SendResult::UnknownAfterTransmit {
            detail: format!("write failed mid-body: {e}"),
        };
    }
    if let Err(e) = tx.flush().await {
        return SendResult::UnknownAfterTransmit {
            detail: format!("flush failed after body: {e}"),
        };
    }

    let acknowledged = match tokio::time::timeout(commit_timeout(), read_reply(&mut reader)).await {
        Ok(r) => r,
        Err(_) => {
            return SendResult::UnknownAfterTransmit {
                detail: "timed out waiting for the server to confirm".into(),
            };
        }
    };
    match acknowledged {
        Ok(r) if (200..300).contains(&code_of(&r)) => SendResult::Committed { response: r },
        Ok(r) if (500..600).contains(&code_of(&r)) => {
            SendResult::RejectedPermanently { response: r }
        }
        Ok(r) => SendResult::UnknownAfterTransmit {
            detail: format!("unexpected reply after body: {r}"),
        },
        // The classic: body sent, acknowledgement never arrived.
        Err(e) => SendResult::UnknownAfterTransmit {
            detail: format!("no acknowledgement after body: {e}"),
        },
    }
}

/// Credentials and endpoint for the shipping send path.
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The same two kinds as IMAP, for the same reason: a bearer token and a
    /// password go on the wire through different commands, and one handed to
    /// the other's command fails in a way that reads as a wrong password.
    pub credential: crate::imap::Credential,
}

/// The port that speaks TLS from its first byte.
///
/// Everything else is submission in the clear until STARTTLS upgrades it,
/// which is the only thing iCloud and Outlook offer: neither has anything
/// listening on 465 at all, so the implicit-TLS-only client could receive
/// their mail and never send any.
const IMPLICIT_TLS_PORT: u16 = 465;

/// What Petrel calls itself at EHLO.
///
/// An address literal, not the server's own hostname — which is what this used
/// to send, and is a small lie: EHLO names the *client*. A machine behind NAT
/// has no name worth stating, and RFC 5321 §4.1.4 provides the literal form
/// for exactly that case.
const EHLO_NAME: &str = "[127.0.0.1]";

impl SmtpConfig {
    /// The submission endpoint for an IMAP host, where the two are the same
    /// provider — which is the only case Petrel currently configures.
    ///
    /// Implicit TLS on 465 where the guess has nothing better to go on: there
    /// is no cleartext phase to strip, so a downgrade attack has nothing to
    /// attack. A port from the provider table wins over this, and providers
    /// that only offer 587 get STARTTLS.
    pub fn for_imap_host(imap_host: &str, user: &str, pass: &str) -> Self {
        let host = imap_host.replacen("imap.", "smtp.", 1);
        SmtpConfig {
            host,
            port: 465,
            user: user.to_string(),
            credential: crate::imap::Credential::password(pass),
        }
    }
}

/// Where a submission conversation stopped before the body was written.
///
/// Kept apart from `SendResult` because the same handshake serves the
/// onboarding check, which reports in words rather than outcomes.
enum OpenError {
    Failed {
        stage: &'static str,
        detail: String,
    },
    Rejected(String),
    /// The credential was refused. Its own case because it is not a fact about
    /// the message: the message is fine and will send once the account can
    /// sign in again, so dead-lettering it loses mail over an expired password.
    Auth(String),
}

impl OpenError {
    fn into_send_result(self) -> SendResult {
        match self {
            OpenError::Failed { stage, detail } => SendResult::FailedBeforeCommit { stage, detail },
            OpenError::Rejected(response) => SendResult::RejectedPermanently { response },
            OpenError::Auth(detail) => SendResult::FailedBeforeCommit {
                stage: "auth",
                detail,
            },
        }
    }

    /// The same failure as a line for the setup form.
    fn into_message(self) -> String {
        match self {
            OpenError::Failed { stage, detail } => format!("{stage}: {}", detail.trim()),
            OpenError::Rejected(response) => response.trim().to_string(),
            OpenError::Auth(detail) => format!("sign-in refused: {}", detail.trim()),
        }
    }
}

/// A submission connection past its handshake: encrypted, greeted, and with
/// the sign-in methods the server named.
struct Wire {
    reader: BufReader<tokio::io::ReadHalf<TlsStream>>,
    writer: tokio::io::WriteHalf<TlsStream>,
    /// The mechanisms from the post-TLS `250-AUTH` line, uppercased.
    auth: Vec<String>,
}

/// The stream every shipping send runs on, whichever port opened it.
type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

/// One reply, checked for the code this stage expects.
async fn expect_reply<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    stage: &'static str,
    want: u16,
) -> std::result::Result<String, OpenError> {
    let reply = match tokio::time::timeout(reply_timeout(), read_reply(reader)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return Err(OpenError::Failed {
                stage,
                detail: e.to_string(),
            });
        }
        Err(_) => {
            return Err(OpenError::Failed {
                stage,
                detail: "timed out waiting for the server".into(),
            });
        }
    };
    let code = code_of(&reply);
    if code == want {
        return Ok(reply);
    }
    // Nothing about the message has been said yet, so no refusal here can
    // be about it: 535 is a bad password, 530 "authentication required",
    // 534 Gmail asking for an app password or a browser sign-in, 454 a
    // temporary refusal. All of them are the account, and dead-lettering the
    // message over any of them told the person the wrong thing.
    if stage == "auth" {
        return Err(OpenError::Auth(reply));
    }
    if code / 100 == 5 {
        return Err(OpenError::Rejected(reply));
    }
    Err(OpenError::Failed {
        stage,
        detail: reply,
    })
}

/// One command line, written and flushed.
async fn say_line<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    stage: &'static str,
    line: &str,
) -> std::result::Result<(), OpenError> {
    match tokio::time::timeout(reply_timeout(), async {
        writer.write_all(line.as_bytes()).await?;
        writer.flush().await
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(OpenError::Failed {
            stage,
            detail: e.to_string(),
        }),
        Err(_) => Err(OpenError::Failed {
            stage,
            detail: "timed out sending to the server".into(),
        }),
    }
}

/// Whether an EHLO reply advertises a keyword.
///
/// Matched as a whole token, not a prefix: the first line of an EHLO reply is
/// the server's own hostname, and a host called `starttls.example.net` would
/// otherwise be read as an offer to upgrade — which is exactly the mistake
/// that must not be made in this direction.
fn ehlo_offers(ehlo: &str, keyword: &str) -> bool {
    ehlo.lines().any(|line| {
        line.get(4..)
            .and_then(|k| k.split_whitespace().next())
            .is_some_and(|k| k.eq_ignore_ascii_case(keyword))
    })
}

/// The mechanisms named on the EHLO reply's AUTH line.
///
/// Both spellings are still out there: `250-AUTH PLAIN LOGIN` and the older
/// `250-AUTH=PLAIN`. A server that names none gets the benefit of the doubt
/// from the caller rather than a refusal here.
fn auth_mechanisms(ehlo: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in ehlo.lines() {
        let keyword = line.get(4..).unwrap_or("").trim().to_ascii_uppercase();
        let list = match keyword
            .strip_prefix("AUTH ")
            .or_else(|| keyword.strip_prefix("AUTH="))
        {
            Some(list) => list,
            None => continue,
        };
        out.extend(list.split_whitespace().map(|m| m.to_string()));
    }
    out
}

/// The 587 half: greet in the clear, ask for STARTTLS, and hand back a socket
/// that is encrypted before anything worth stealing crosses it.
///
/// A server that does not offer STARTTLS is refused outright. There is no
/// fallback to plaintext submission, because the fallback is the attack: a
/// stripped capability line would otherwise put the password on the wire.
async fn starttls_stream(cfg: &SmtpConfig) -> std::result::Result<TlsStream, OpenError> {
    let tcp = match tokio::time::timeout(
        connect_timeout(),
        TcpStream::connect((cfg.host.as_str(), cfg.port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return Err(OpenError::Failed {
                stage: "connect",
                detail: e.to_string(),
            });
        }
        Err(_) => {
            return Err(OpenError::Failed {
                stage: "connect",
                detail: "timed out".into(),
            });
        }
    };
    let (rx, mut tx) = tcp.into_split();
    let mut reader = BufReader::new(rx);
    expect_reply(&mut reader, "greeting", 220).await?;
    say_line(&mut tx, "ehlo", &format!("EHLO {EHLO_NAME}\r\n")).await?;
    let ehlo = expect_reply(&mut reader, "ehlo", 250).await?;
    if !ehlo_offers(&ehlo, "STARTTLS") {
        return Err(OpenError::Failed {
            stage: "starttls",
            detail: "this port offers no STARTTLS, so the password cannot be sent safely".into(),
        });
    }
    say_line(&mut tx, "starttls", "STARTTLS\r\n").await?;
    expect_reply(&mut reader, "starttls", 220).await?;
    // Nothing may be waiting in the buffer: bytes sent before the handshake
    // are not part of it, and carrying them across would mean trusting
    // plaintext the upgrade exists to end.
    if !reader.buffer().is_empty() {
        return Err(OpenError::Failed {
            stage: "starttls",
            detail: "the server sent data before the TLS handshake".into(),
        });
    }
    let tcp = reader
        .into_inner()
        .reunite(tx)
        .map_err(|e| OpenError::Failed {
            stage: "starttls",
            detail: e.to_string(),
        })?;
    crate::imap::tls_upgrade(&cfg.host, tcp)
        .await
        .map_err(|e| OpenError::Failed {
            stage: "starttls",
            detail: e.to_string(),
        })
}

/// Opens a submission connection and gets as far as the post-TLS EHLO.
///
/// Implicit TLS on 465, STARTTLS everywhere else — and either way what comes
/// back is encrypted, so there is one path from here on and no way to reach
/// AUTH without TLS underneath it.
async fn open_submission(
    cfg: &SmtpConfig,
    stage: &mut impl FnMut(&'static str),
) -> std::result::Result<Wire, OpenError> {
    stage("connect");
    let (stream, greeted) = if cfg.port == IMPLICIT_TLS_PORT {
        let stream = match tokio::time::timeout(
            connect_timeout(),
            crate::imap::tls_stream_for(&cfg.host, cfg.port),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(OpenError::Failed {
                    stage: "connect",
                    detail: e.to_string(),
                });
            }
            Err(_) => {
                return Err(OpenError::Failed {
                    stage: "connect",
                    detail: "timed out".into(),
                });
            }
        };
        (stream, false)
    } else {
        stage("starttls");
        (starttls_stream(cfg).await?, true)
    };

    let (rx, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(rx);
    if !greeted {
        stage("greeting");
        expect_reply(&mut reader, "greeting", 220).await?;
    }
    // A second EHLO after STARTTLS is not politeness: the capability list from
    // before the upgrade is not to be trusted, and AUTH usually only appears
    // in the second one.
    stage("ehlo");
    say_line(&mut writer, "ehlo", &format!("EHLO {EHLO_NAME}\r\n")).await?;
    let ehlo = expect_reply(&mut reader, "ehlo", 250).await?;
    Ok(Wire {
        auth: auth_mechanisms(&ehlo),
        reader,
        writer,
    })
}

/// Signs in with a mechanism this server actually offers.
///
/// PLAIN where it is offered — one round trip — and LOGIN where it is not:
/// Outlook's submission server names `LOGIN XOAUTH2` and refuses PLAIN
/// outright, which is why hard-coding PLAIN meant no Outlook account could
/// ever send. A server that named nothing gets PLAIN, which is what it almost
/// certainly speaks.
async fn authenticate<R, W>(
    reader: &mut R,
    writer: &mut W,
    cfg: &SmtpConfig,
    mechanisms: &[String],
) -> std::result::Result<(), OpenError>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    use base64::Engine as _;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let offers = |m: &str| mechanisms.iter().any(|a| a == m);

    match &cfg.credential {
        // A token has exactly one way onto the wire, whatever was advertised.
        crate::imap::Credential::Bearer(token) => {
            let payload = b64(&crate::imap::xoauth2_payload(&cfg.user, token));
            say_line(writer, "auth", &format!("AUTH XOAUTH2 {payload}\r\n")).await?;
            expect_reply(reader, "auth", 235).await?;
        }
        crate::imap::Credential::Password(pass) => {
            if offers("PLAIN") || mechanisms.is_empty() {
                let payload = b64(&format!("\0{}\0{}", cfg.user, pass));
                say_line(writer, "auth", &format!("AUTH PLAIN {payload}\r\n")).await?;
                expect_reply(reader, "auth", 235).await?;
            } else if offers("LOGIN") {
                // Three round trips, each answered with a 334 carrying the
                // base64 of "Username:" and then "Password:". The prompts are
                // not read: they are decoration, and a client that matched on
                // their wording would break on the first server that phrased
                // them differently.
                say_line(writer, "auth", "AUTH LOGIN\r\n").await?;
                expect_reply(reader, "auth", 334).await?;
                say_line(writer, "auth", &format!("{}\r\n", b64(&cfg.user))).await?;
                expect_reply(reader, "auth", 334).await?;
                say_line(writer, "auth", &format!("{}\r\n", b64(pass))).await?;
                expect_reply(reader, "auth", 235).await?;
            } else {
                return Err(OpenError::Auth(format!(
                    "the server accepts none of the sign-in methods Petrel can use ({})",
                    mechanisms.join(" ")
                )));
            }
        }
    }
    Ok(())
}

/// Connects, signs in and hangs up.
///
/// The onboarding test's second half. Getting as far as a 235 after AUTH is
/// the whole question: the host answers on this port over TLS, and these
/// credentials are accepted for sending. No MAIL FROM is issued — a test
/// that left a half-open envelope on the server would be a worse test.
pub async fn login_check(cfg: &SmtpConfig) -> std::result::Result<(), String> {
    // One ceiling over the whole check. A host that accepts the connection
    // and then says nothing, or a black-holed port, used to leave the setup
    // form spinning for as long as the socket lived.
    match tokio::time::timeout(check_timeout(), login_check_inner(cfg)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "no answer from {}:{} after {}s",
            cfg.host,
            cfg.port,
            check_timeout().as_secs()
        )),
    }
}

async fn login_check_inner(cfg: &SmtpConfig) -> std::result::Result<(), String> {
    // The same handshake and the same mechanism choice a real send makes:
    // onboarding has to test what the account will actually do, or a token
    // account passes its setup check and fails on the first message.
    let mut wire = open_submission(cfg, &mut |_| {})
        .await
        .map_err(OpenError::into_message)?;
    let Wire {
        ref mut reader,
        ref mut writer,
        ref auth,
    } = wire;
    authenticate(reader, writer, cfg, auth)
        .await
        .map_err(OpenError::into_message)?;
    let _ = writer.write_all(b"QUIT\r\n").await;
    Ok(())
}

/// Sends over implicit TLS with AUTH PLAIN.
///
/// Shares its outcome taxonomy with the plaintext path deliberately: the whole
/// point of `SendResult` is that "we never heard back" is a distinct answer
/// from "it failed", and that distinction is what spike S5's reconciliation
/// rule turns into "ask the server whether it has the message" rather than a
/// retry that might duplicate it.
/// How long each phase of an SMTP conversation may take.
///
/// Without these the client waited forever. Nothing in the exchange had a
/// deadline, so a server that accepted the TCP connection and then went quiet
/// left `read_reply` pending for good — and because sending is awaited inside
/// the same worker that delivers queued triage, one hung send stopped archives
/// and moves going out too. A hang is the failure mode with no symptom: no
/// error, no retry, just a message that never leaves.
///
/// The handshake values are deliberately tighter than RFC 5321's recommended
/// minimums, which are written for relaying MTAs. A submission server that
/// cannot answer EHLO inside a minute is not slow, it is broken.
///
/// The acknowledgement after the final dot keeps the RFC's ten minutes,
/// because that is the one wait where slowness is legitimate: the server is
/// scanning and queueing the message, and a large attachment through a spam
/// filter really can take minutes. Cutting that one short would turn healthy
/// sends into ambiguous ones, which is the expensive direction to be wrong in.
/// Overridable so a test can prove the policy on loopback without waiting ten
/// minutes for it. Same shape as the sync loop's knobs.
pub(crate) fn phase_timeout(var: &str, default_secs: u64) -> Duration {
    Duration::from_secs(
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_secs),
    )
}

/// The onboarding connection test, start to finish.
pub(crate) fn check_timeout() -> Duration {
    phase_timeout("PETREL_CHECK_SECONDS", 30)
}

fn connect_timeout() -> Duration {
    phase_timeout("PETREL_SMTP_CONNECT_SECONDS", 30)
}
fn reply_timeout() -> Duration {
    phase_timeout("PETREL_SMTP_REPLY_SECONDS", 60)
}
fn body_write_timeout() -> Duration {
    phase_timeout("PETREL_SMTP_BODY_SECONDS", 120)
}
fn commit_timeout() -> Duration {
    phase_timeout("PETREL_SMTP_COMMIT_SECONDS", 600)
}

pub async fn send_tls(cfg: &SmtpConfig, msg: &Outgoing, raw: &[u8]) -> SendResult {
    send_tls_with(cfg, msg, raw, |_| {}).await
}

/// Same conversation as `send_tls`, with a hook at each stage.
///
/// The desktop logs these so a send that sits in `Transmitting` is not a
/// blank: you can see whether it is still shaking hands, writing the body,
/// or waiting for the server to accept it. The hook is told a stage name
/// only — never a host, address, or anything from the message.
pub async fn send_tls_with(
    cfg: &SmtpConfig,
    msg: &Outgoing,
    raw: &[u8],
    mut stage: impl FnMut(&'static str),
) -> SendResult {
    macro_rules! fail_before {
        ($stage:expr, $e:expr) => {
            return SendResult::FailedBeforeCommit {
                stage: $stage,
                detail: $e.to_string(),
            }
        };
    }

    // Connect, upgrade if the port needs it, greet, EHLO — and sign in with a
    // mechanism this server named rather than the one mechanism this used to
    // know.
    let wire = match open_submission(cfg, &mut stage).await {
        Ok(w) => w,
        Err(e) => return e.into_send_result(),
    };
    let Wire {
        mut reader,
        writer: mut tx,
        auth,
    } = wire;
    stage("auth");
    if let Err(e) = authenticate(&mut reader, &mut tx, cfg, &auth).await {
        return e.into_send_result();
    }

    macro_rules! expect {
        ($stage:expr, $want:expr) => {{
            let reply = match tokio::time::timeout(reply_timeout(), read_reply(&mut reader)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => fail_before!($stage, e),
                Err(_) => fail_before!($stage, "timed out waiting for the server"),
            };
            let code = code_of(&reply);
            if code / 100 == 5 {
                return SendResult::RejectedPermanently { response: reply };
            }
            if code != $want {
                fail_before!($stage, reply);
            }
            reply
        }};
    }
    // write_all of a few bytes can complete with the TLS record still in
    // rustls's buffer. The plaintext path flushes; this one did not, and a
    // DATA terminator that never left the buffer left the server waiting
    // for the end of the message while the client waited ten minutes for
    // a 250 that could not come.
    macro_rules! say {
        ($stage:expr, $line:expr) => {
            match tokio::time::timeout(reply_timeout(), async {
                tx.write_all($line.as_bytes()).await?;
                tx.flush().await
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => fail_before!($stage, e),
                Err(_) => fail_before!($stage, "timed out sending to the server"),
            }
        };
    }

    stage("mail");
    // The envelope carries addr-specs and nothing else. A sender or recipient
    // that is not one never reaches the wire: it would be a second command,
    // written by whoever supplied the string.
    let Some(sender) = msg.sender() else {
        fail_before!(
            "mail",
            "the account's own address is not one that can be sent from"
        );
    };
    say!("mail", format!("MAIL FROM:<{sender}>\r\n"));
    expect!("mail", 250);
    for rcpt in msg.recipients() {
        stage("rcpt");
        say!("rcpt", format!("RCPT TO:<{rcpt}>\r\n"));
        expect!("rcpt", 250);
    }
    stage("data");
    say!("data", "DATA\r\n".to_string());
    expect!("data", 354);

    // Past this point a failure is ambiguous rather than safe to retry: the
    // server may have committed the message even if we never hear so.
    // Body and terminating dot in one write, then flush: a terminator that
    // sat in the TLS buffer left the server in DATA and the client waiting
    // the full ten-minute commit timeout.
    stage("body");
    let mut payload = dot_stuff(raw);
    if !payload.ends_with(b"\r\n") {
        payload.extend_from_slice(b"\r\n");
    }
    payload.extend_from_slice(b".\r\n");
    match tokio::time::timeout(body_write_timeout(), async {
        tx.write_all(&payload).await?;
        tx.flush().await
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return SendResult::UnknownAfterTransmit {
                detail: e.to_string(),
            };
        }
        Err(_) => {
            return SendResult::UnknownAfterTransmit {
                detail: "timed out sending the message body".into(),
            };
        }
    }
    stage("confirm");
    let acknowledged = match tokio::time::timeout(commit_timeout(), read_reply(&mut reader)).await {
        Ok(r) => r,
        // The message went and the answer never came. Exactly the case the
        // outcome exists for: retrying could duplicate it, so somebody has to
        // look rather than guess.
        Err(_) => {
            return SendResult::UnknownAfterTransmit {
                detail: "timed out waiting for the server to confirm".into(),
            };
        }
    };
    match acknowledged {
        Ok(reply) if code_of(&reply) / 100 == 2 => {
            let _ = tx.write_all(b"QUIT\r\n").await;
            SendResult::Committed { response: reply }
        }
        Ok(reply) if code_of(&reply) / 100 == 5 => {
            SendResult::RejectedPermanently { response: reply }
        }
        Ok(reply) => SendResult::UnknownAfterTransmit { detail: reply },
        Err(e) => SendResult::UnknownAfterTransmit {
            detail: e.to_string(),
        },
    }
}

/// RFC 5321 dot-stuffing: a line that begins with "." gets another, or it ends
/// the message early. A body containing a line of just "." is rare and the bug
/// it causes — a silently truncated message — is not one the sender ever sees.
fn dot_stuff(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 16);
    let mut at_line_start = true;
    for &b in raw {
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        at_line_start = b == b'\n';
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::Credential;

    const OUTLOOK_EHLO: &str = "250-DUB01.prod.protection.outlook.com Hello\r\n\
                                250-SIZE 157286400\r\n\
                                250-PIPELINING\r\n\
                                250-AUTH LOGIN XOAUTH2\r\n\
                                250 SMTPUTF8";

    /// What the envelope would leave out is what the sender has to be told
    /// about first. A blank from a trailing comma is not a recipient at all,
    /// and a display name in front of a good address is not a problem.
    #[test]
    fn the_entries_the_envelope_would_drop_are_named() {
        let list: Vec<String> = [
            "sam@example.com",
            "dan",
            "Dana Wu",
            "<>",
            "bob@",
            "",
            "  ",
            "Dana Wu <dana@example.com>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(unsendable_recipients(&list), vec!["dan", "Dana Wu", "<>"]);
    }

    #[test]
    fn the_auth_line_is_read_off_the_ehlo_reply() {
        assert_eq!(auth_mechanisms(OUTLOOK_EHLO), vec!["LOGIN", "XOAUTH2"]);
        // The older spelling, one mechanism per line, and lowercase.
        assert_eq!(
            auth_mechanisms("250-mail\r\n250-auth=plain\r\n250 SIZE 10"),
            vec!["PLAIN"]
        );
        // A server that names nothing is not the same as one that offers
        // nothing: the caller falls back to PLAIN rather than refusing.
        assert!(auth_mechanisms("250-mail\r\n250 SIZE 10").is_empty());
        assert!(ehlo_offers(
            "250-mail\r\n250-STARTTLS\r\n250 SIZE 1",
            "STARTTLS"
        ));
        assert!(!ehlo_offers(OUTLOOK_EHLO, "STARTTLS"));
        // The greeting text is not a capability list, whatever it contains.
        assert!(!ehlo_offers("220 starttls.example ESMTP", "STARTTLS"));
    }

    /// Drives `authenticate` against a scripted peer and reports what the
    /// client said.
    async fn exchange(mechanisms: &[&str], scripted: &[&str], cred: Credential) -> Vec<String> {
        let (client, server) = tokio::io::duplex(4096);
        let script: Vec<String> = scripted.iter().map(|s| s.to_string()).collect();
        let heard = tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server);
            let mut reader = BufReader::new(rx);
            let mut said = Vec::new();
            for reply in script {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                said.push(line.trim_end().to_string());
                if tx.write_all(reply.as_bytes()).await.is_err() {
                    break;
                }
            }
            said
        });
        let (rx, mut tx) = tokio::io::split(client);
        let mut reader = BufReader::new(rx);
        let cfg = SmtpConfig {
            host: "mail.example".into(),
            port: 587,
            user: "tom@example.com".into(),
            credential: cred,
        };
        let mechanisms: Vec<String> = mechanisms.iter().map(|m| m.to_string()).collect();
        authenticate(&mut reader, &mut tx, &cfg, &mechanisms)
            .await
            .map_err(OpenError::into_message)
            .expect("the scripted server accepts");
        heard.await.unwrap()
    }

    #[tokio::test]
    async fn a_server_without_plain_is_signed_in_to_with_auth_login() {
        use base64::Engine as _;
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        // Outlook's own shape: LOGIN and XOAUTH2, no PLAIN. Sending PLAIN here
        // earned a 504 and no account could send.
        let said = exchange(
            &["LOGIN", "XOAUTH2"],
            &[
                &format!("334 {}\r\n", b64("Username:")),
                &format!("334 {}\r\n", b64("Password:")),
                "235 2.7.0 Authentication successful\r\n",
            ],
            Credential::password("s3cret"),
        )
        .await;
        assert_eq!(
            said,
            vec![
                "AUTH LOGIN".to_string(),
                b64("tom@example.com"),
                b64("s3cret"),
            ],
            "the two prompts are answered in order, each base64"
        );
    }

    #[tokio::test]
    async fn plain_is_preferred_where_it_is_offered_and_assumed_where_nothing_is() {
        use base64::Engine as _;
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let expected = format!("AUTH PLAIN {}", b64("\0tom@example.com\0s3cret"));
        let said = exchange(
            &["PLAIN", "LOGIN"],
            &["235 ok\r\n"],
            Credential::password("s3cret"),
        )
        .await;
        assert_eq!(
            said,
            vec![expected.clone()],
            "one round trip where it can be"
        );
        let said = exchange(&[], &["235 ok\r\n"], Credential::password("s3cret")).await;
        assert_eq!(said, vec![expected]);
    }

    #[tokio::test]
    async fn a_token_goes_out_as_xoauth2_whatever_was_advertised() {
        use base64::Engine as _;
        let said = exchange(
            &["LOGIN"],
            &["235 ok\r\n"],
            Credential::Bearer("ya29.TOKEN".into()),
        )
        .await;
        let payload = base64::engine::general_purpose::STANDARD.encode(
            crate::imap::xoauth2_payload("tom@example.com", "ya29.TOKEN"),
        );
        assert_eq!(said, vec![format!("AUTH XOAUTH2 {payload}")]);
    }

    /// A refused password is not a bad message. Dead-lettering it — which is
    /// what a plain 5xx does — loses mail because a token expired.
    #[tokio::test]
    async fn a_refused_credential_is_reported_as_a_sign_in_failure_not_a_rejection() {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server);
            let mut reader = BufReader::new(rx);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;
            let _ = tx
                .write_all(b"535 5.7.3 Authentication unsuccessful\r\n")
                .await;
        });
        let (rx, mut tx) = tokio::io::split(client);
        let mut reader = BufReader::new(rx);
        let cfg = SmtpConfig {
            host: "mail.example".into(),
            port: 587,
            user: "tom@example.com".into(),
            credential: Credential::password("wrong"),
        };
        let err = authenticate(&mut reader, &mut tx, &cfg, &["PLAIN".to_string()])
            .await
            .expect_err("535 is a refusal");
        match err.into_send_result() {
            SendResult::FailedBeforeCommit { stage, detail } => {
                assert_eq!(stage, "auth");
                assert!(detail.contains("535"), "{detail}");
            }
            other => panic!("a refused password must not dead-letter the message: {other:?}"),
        }
    }

    /// Gmail's other refusals at AUTH — an app password required, a browser
    /// sign-in wanted — are 534, not 535, and they used to dead-letter the
    /// message as a rejection. Nothing about the message has been said at
    /// that stage, so no refusal there can be about it.
    #[tokio::test]
    async fn every_refusal_at_auth_is_about_the_account_not_the_message() {
        let (client, server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let (rx, mut tx) = tokio::io::split(server);
            let mut reader = BufReader::new(rx);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await;
            let _ = tx
                .write_all(
                    b"534-5.7.9 Application-specific password required.\r\n\
                      534 5.7.9 Learn more at https://support.example\r\n",
                )
                .await;
        });
        let (rx, mut tx) = tokio::io::split(client);
        let mut reader = BufReader::new(rx);
        let cfg = SmtpConfig {
            host: "mail.example".into(),
            port: 587,
            user: "tom@example.com".into(),
            credential: Credential::password("account-password-not-app-password"),
        };
        let err = authenticate(&mut reader, &mut tx, &cfg, &["PLAIN".to_string()])
            .await
            .expect_err("534 is a refusal");
        match err.into_send_result() {
            SendResult::FailedBeforeCommit { stage, detail } => {
                assert_eq!(stage, "auth");
                assert!(detail.contains("534"), "{detail}");
            }
            other => panic!("an app-password demand must not dead-letter the message: {other:?}"),
        }
    }
}
