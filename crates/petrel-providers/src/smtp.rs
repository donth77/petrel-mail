//! SMTP submission — M0 slice.
//!
//! Deliberately minimal and hand-rolled for the spike: the interesting part is
//! not the protocol (RFC 5321 submission is small) but *classifying failures*.
//! Where a send fails decides whether a retry is safe, so this reports the
//! failure point, never a bare error.

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

        let mut b = MessageBuilder::new()
            .from((self.from_name.as_str(), self.from_addr.as_str()))
            .subject(self.subject.as_str())
            .message_id(message_id);

        // Pasted images ride the draft as data: URIs; the wire gets them as
        // parts of their own, referenced by cid. Seeded from the Message-ID so
        // the ids are as unique as the message they belong to.
        let (html, inline) = match &self.body_html {
            Some(html) => {
                let (rewritten, inline) = extract_inline_images(html, message_id);
                (Some(rewritten), inline)
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
            for a in &self.attachments {
                b = b.attachment(
                    a.content_type.as_str(),
                    a.filename.as_str(),
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
                for a in &self.attachments {
                    mixed.push(
                        MimePart::new(a.content_type.as_str(), a.bytes.as_slice())
                            .attachment(a.filename.as_str()),
                    );
                }
                MimePart::new("multipart/mixed", mixed)
            });
        }

        for addr in &self.to {
            b = b.to(addr.as_str());
        }
        for addr in &self.cc {
            b = b.cc(addr.as_str());
        }
        if let Some(parent) = &self.in_reply_to {
            b = b.in_reply_to(parent.as_str());
        }
        if !self.references.is_empty() {
            b = b.references(
                self.references
                    .iter()
                    .map(|r| r.as_str())
                    .collect::<Vec<_>>(),
            );
        }

        b.write_to_vec().unwrap_or_default()
    }

    /// Every address the envelope has to name.
    pub fn recipients(&self) -> Vec<String> {
        self.to.iter().chain(self.cc.iter()).cloned().collect()
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
    match read_reply(&mut reader).await {
        Ok(r) if code_of(&r) == 220 => {}
        Ok(r) => fail_before!("greeting", r),
        Err(e) => fail_before!("greeting", e),
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
        match read_reply(&mut reader).await {
            Ok(r) if code_of(&r) == expect => {}
            Ok(r) if (500..600).contains(&code_of(&r)) => {
                return SendResult::RejectedPermanently { response: r };
            }
            Ok(r) => fail_before!(stage, r),
            Err(e) => fail_before!(stage, e),
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

    match read_reply(&mut reader).await {
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
    pub pass: String,
}

impl SmtpConfig {
    /// The submission endpoint for an IMAP host, where the two are the same
    /// provider — which is the only case Petrel currently configures.
    ///
    /// Implicit TLS on 465 rather than STARTTLS on 587: there is no cleartext
    /// phase to strip, so a downgrade attack has nothing to attack.
    pub fn for_imap_host(imap_host: &str, user: &str, pass: &str) -> Self {
        let host = imap_host.replacen("imap.", "smtp.", 1);
        SmtpConfig {
            host,
            port: 465,
            user: user.to_string(),
            pass: pass.to_string(),
        }
    }
}

/// Connects, signs in and hangs up.
///
/// The onboarding test's second half. Getting as far as a 235 after AUTH is
/// the whole question: the host answers on this port over TLS, and these
/// credentials are accepted for sending. No MAIL FROM is issued — a test
/// that left a half-open envelope on the server would be a worse test.
pub async fn login_check(cfg: &SmtpConfig) -> std::result::Result<(), String> {
    use base64::Engine as _;
    use tokio::io::{AsyncWriteExt, BufReader};
    let tls = crate::imap::tls_stream_for(&cfg.host, cfg.port)
        .await
        .map_err(|e| e.to_string())?;
    let (r, mut w) = tokio::io::split(tls);
    let mut reader = BufReader::new(r);
    let banner = read_reply(&mut reader).await.map_err(|e| e.to_string())?;
    if !banner.starts_with("220") {
        return Err(format!("greeting: {}", banner.trim()));
    }
    w.write_all(b"EHLO petrel\r\n")
        .await
        .map_err(|e| e.to_string())?;
    let ehlo = read_reply(&mut reader).await.map_err(|e| e.to_string())?;
    if !ehlo.starts_with("250") {
        return Err(format!("EHLO: {}", ehlo.trim()));
    }
    let plain =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{}", cfg.user, cfg.pass));
    w.write_all(format!("AUTH PLAIN {plain}\r\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let auth = read_reply(&mut reader).await.map_err(|e| e.to_string())?;
    if !auth.starts_with("235") {
        return Err(format!("sign-in refused: {}", auth.trim()));
    }
    let _ = w.write_all(b"QUIT\r\n").await;
    Ok(())
}

/// Sends over implicit TLS with AUTH PLAIN.
///
/// Shares its outcome taxonomy with the plaintext path deliberately: the whole
/// point of `SendResult` is that "we never heard back" is a distinct answer
/// from "it failed", and that distinction is what spike S5's reconciliation
/// rule turns into "ask the server whether it has the message" rather than a
/// retry that might duplicate it.
pub async fn send_tls(cfg: &SmtpConfig, msg: &Outgoing, raw: &[u8]) -> SendResult {
    use base64::Engine as _;
    use tokio::io::{AsyncWriteExt, BufReader};

    macro_rules! fail_before {
        ($stage:expr, $e:expr) => {
            return SendResult::FailedBeforeCommit {
                stage: $stage,
                detail: $e.to_string(),
            }
        };
    }

    let stream = match crate::imap::tls_stream_for(&cfg.host, cfg.port).await {
        Ok(s) => s,
        Err(e) => fail_before!("connect", e),
    };
    let (rx, mut tx) = tokio::io::split(stream);
    let mut reader = BufReader::new(rx);

    macro_rules! expect {
        ($stage:expr, $want:expr) => {{
            let reply = match read_reply(&mut reader).await {
                Ok(r) => r,
                Err(e) => fail_before!($stage, e),
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
    macro_rules! say {
        ($stage:expr, $line:expr) => {
            if let Err(e) = tx.write_all($line.as_bytes()).await {
                fail_before!($stage, e);
            }
        };
    }

    expect!("greeting", 220);
    say!("ehlo", format!("EHLO {}\r\n", cfg.host));
    expect!("ehlo", 250);

    // AUTH PLAIN is \0user\0pass, base64. Only ever over TLS, which is why
    // this function has no plaintext sibling.
    let token =
        base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{}", cfg.user, cfg.pass));
    say!("auth", format!("AUTH PLAIN {token}\r\n"));
    expect!("auth", 235);

    say!("mail", format!("MAIL FROM:<{}>\r\n", msg.from_addr));
    expect!("mail", 250);
    for rcpt in msg.recipients() {
        say!("rcpt", format!("RCPT TO:<{rcpt}>\r\n"));
        expect!("rcpt", 250);
    }
    say!("data", "DATA\r\n".to_string());
    expect!("data", 354);

    // Past this point a failure is ambiguous rather than safe to retry: the
    // server may have committed the message even if we never hear so.
    let dotted = dot_stuff(raw);
    if let Err(e) = tx.write_all(&dotted).await {
        return SendResult::UnknownAfterTransmit {
            detail: e.to_string(),
        };
    }
    if let Err(e) = tx.write_all(b"\r\n.\r\n").await {
        return SendResult::UnknownAfterTransmit {
            detail: e.to_string(),
        };
    }
    match read_reply(&mut reader).await {
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
