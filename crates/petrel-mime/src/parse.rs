//! RFC822 → structured view, for indexing and display.
//!
//! Deliberately total: every field is optional or defaulted, so a message that
//! violates every rule in RFC 5322 still yields *something* storable. Refusing
//! to parse would mean refusing to show the user mail that already sits in
//! their mailbox — the parser's job is to salvage, not to judge.

use mail_parser::{Address, HeaderValue, MessageParser, MimeHeaders};

use crate::encoded_word::merge_adjacent_encoded_words;

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

/// The composer refuses a pasted picture over this (`EMBED_CAP`), so a quoted
/// one gets no more room.
const QUOTE_PICTURE_CAP: usize = 8 * 1024 * 1024;
/// Every picture in one quote, together: well under the 25MB a message may
/// weigh, with room left for what the reply attaches.
const QUOTE_PICTURES_BUDGET: usize = 16 * 1024 * 1024;

/// One of the original's inline pictures, as a quote of it carries it.
pub struct QuotedPicture {
    /// The part's index in `attachments()`, the same one the store records.
    pub part: usize,
    pub cid: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// The inline pictures a quote of this message carries, in part order.
///
/// Only pictures `html` actually shows, by `cid:`; an inline part nothing
/// refers to is an attachment in all but name, and a forward attaches it.
/// Raster types only, never SVG. Each within the composer's paste limit and
/// all within one budget, so a picture past either is left out of the quote.
///
/// `html` is the sanitized body, the one [`embed_cid_images`] rewrites, so the
/// two agree on which pictures travel in the body. A forward uses this to
/// leave those out of its attachments rather than send them twice.
pub fn quoted_pictures(html: &str, raw: &[u8]) -> Vec<QuotedPicture> {
    quoted_pictures_within(html, raw, QUOTE_PICTURE_CAP, QUOTE_PICTURES_BUDGET)
}

fn quoted_pictures_within(
    html: &str,
    raw: &[u8],
    per_picture: usize,
    budget: usize,
) -> Vec<QuotedPicture> {
    let mut out = Vec::new();
    if !html.contains("cid:") {
        return out;
    }
    let Some(msg) = MessageParser::default().parse(raw) else {
        return out;
    };
    let mut spent = 0usize;
    for (part, att) in msg.attachments().enumerate() {
        let Some(cid) = att.content_id() else {
            continue;
        };
        // Referenced as the sanitizer serializes an attribute value, which is
        // also the form `resolve_cids` looks for.
        let escaped = cid
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        if !html.contains(&format!("cid:{escaped}")) {
            continue;
        }
        let mime = att
            .content_type()
            .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or_default()))
            .unwrap_or_default()
            .to_ascii_lowercase();
        // SVG can carry script, and a picture is all a quote needs.
        let mime = match mime.as_str() {
            "image/png" | "image/gif" | "image/webp" | "image/jpeg" => mime,
            "image/jpg" | "image/pjpeg" => "image/jpeg".to_string(),
            _ => continue,
        };
        let bytes = att.contents();
        if bytes.len() > per_picture || spent + bytes.len() > budget {
            continue;
        }
        spent += bytes.len();
        out.push(QuotedPicture {
            part,
            cid: cid.to_string(),
            mime,
            bytes: bytes.to_vec(),
        });
    }
    out
}

/// Writes the original's own pictures into a quote of it.
///
/// A reply or a forward quotes the original's HTML, whose `cid:` images name
/// parts of the *original* message. The new message does not carry those
/// parts, so the pictures arrived broken at the other end, and were broken in
/// the composer already, since only a mail renderer can follow a cid. Each is
/// written in as a `data:` URL instead: the composer shows it, and the send
/// turns it back into an inline part of the new message, the way a pasted
/// picture travels. Nothing is fetched; the bytes are the message's own.
///
/// A `cid:` picture left over names a part the reply will not carry, so it is
/// taken out of `src`. The composer's image node needs a `src`, so the picture
/// is left out rather than sent broken.
///
/// Runs on sanitized HTML, as [`crate::resolve_cids`] does.
pub fn embed_cid_images(html: &str, raw: &[u8]) -> String {
    embed_pictures(html, &quoted_pictures(html, raw))
}

/// A picture as a `data:` URL, the form the composer shows and the send turns
/// into an inline part.
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn embed_pictures(html: &str, pictures: &[QuotedPicture]) -> String {
    let parts: Vec<Attachment> = pictures
        .iter()
        .map(|p| Attachment {
            filename: None,
            content_type: Some(p.mime.clone()),
            size: p.bytes.len(),
            content_id: Some(p.cid.clone()),
            is_inline: true,
        })
        .collect();
    let resolved = crate::sanitize::resolve_cids(html, &parts, |i| {
        data_url(&pictures[i].mime, &pictures[i].bytes)
    });
    resolved
        .replace("src=\"cid:", "data-cid=\"")
        .replace("src='cid:", "data-cid='")
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

/// What the receiving server concluded about who sent a message.
///
/// Read from `Authentication-Results` (RFC 8601), which the server that
/// accepted the mail writes after doing the SPF, DKIM and DMARC checks
/// itself. Petrel cannot redo those checks: SPF needs the connecting IP,
/// which is gone by the time the message is stored, and DKIM needs a DNS
/// lookup against a key that may since have rotated. So this reports a
/// verdict rather than reaching one.
///
/// That makes the header only as trustworthy as the server that wrote it,
/// which is why `authserv` is kept and shown. A stamp from your own provider
/// means something; one a sender put there themselves means nothing, and the
/// two are told apart by who is named, not by the header's presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthVerdict {
    /// The check ran and the message passed it.
    Pass,
    /// The check ran and the message failed it. Worth saying out loud.
    Fail,
    /// The check ran and reached no conclusion: `none`, `neutral`,
    /// `policy`, `temperror`, `permerror`. Not a failure, and not a pass.
    Inconclusive,
}

/// The verdicts a message carries, and who reached them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authentication {
    /// The server that performed the checks, verbatim from the header.
    pub authserv: Option<String>,
    pub spf: Option<AuthVerdict>,
    pub dkim: Option<AuthVerdict>,
    pub dmarc: Option<AuthVerdict>,
    /// The domain DMARC aligned against, when it says. This is the one worth
    /// showing a person: "really from example.com" is a sentence, whereas
    /// "dmarc=pass" is a log line.
    pub domain: Option<String>,
}

impl Authentication {
    /// Whether anything was actually reported. A message with the header but
    /// no method we recognise is the same as a message without it.
    pub fn is_empty(&self) -> bool {
        self.spf.is_none() && self.dkim.is_none() && self.dmarc.is_none()
    }

    /// DMARC is the only one that answers the question a person is asking.
    ///
    /// SPF and DKIM each pass for a domain that need not be the one in the
    /// From line, so neither on its own says the sender is who they appear to
    /// be. DMARC is the check that ties them to it. Where DMARC is absent,
    /// this stays quiet rather than promoting a weaker check into a claim it
    /// cannot support.
    pub fn identity_verified(&self) -> Option<bool> {
        match self.dmarc {
            Some(AuthVerdict::Pass) => Some(true),
            Some(AuthVerdict::Fail) => Some(false),
            _ => None,
        }
    }
}

fn verdict(word: &str) -> AuthVerdict {
    match word {
        "pass" => AuthVerdict::Pass,
        // `softfail` is a fail the domain owner asked to be treated gently.
        // It is still the domain saying this did not come from them.
        "fail" | "softfail" => AuthVerdict::Fail,
        _ => AuthVerdict::Inconclusive,
    }
}

/// Reads the authentication verdicts out of a raw message, if any.
///
/// Only the *first* `Authentication-Results` header is read. Mail can carry
/// several, and they are prepended in order, so the first is the one written
/// by the server closest to you. A later one was written further upstream,
/// possibly by a host you have no reason to trust.
pub fn authentication(raw: &[u8]) -> Option<Authentication> {
    let msg = MessageParser::default().parse(raw)?;
    // header_values, not header_raw. header_raw hands back the *last*
    // occurrence, and a receiving server prepends its results — so the last
    // one is the furthest upstream, which on inbound mail means the one the
    // sender could have written themselves. Taking it would let anybody claim
    // dmarc=pass by adding a header. Verified against mail-parser rather than
    // assumed: with two headers present it returned the upstream one.
    let header = msg
        .header_values("Authentication-Results")
        .next()
        .and_then(|v| v.as_text().map(|t| t.to_string()))
        .or_else(|| {
            msg.header_raw("Authentication-Results")
                .map(|h| h.to_string())
        })?;
    let flat = header.replace(['\r', '\n'], " ");
    let lower = flat.to_ascii_lowercase();

    // Everything before the first semicolon is the authserv-id.
    let authserv = flat
        .split(';')
        .next()
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty());

    // `method=result`, with the result running to the next space, semicolon or
    // bracket. Values may be quoted and may carry a `(comment)` after them.
    let read = |method: &str| -> Option<AuthVerdict> {
        let needle = format!("{method}=");
        let mut from = 0usize;
        while let Some(at) = lower[from..].find(&needle) {
            let start = from + at;
            // Must not be the tail of a longer token: `header.dkim=` and
            // `dkim=` are different things, and so is `xdmarc=`.
            let ok_before = start == 0
                || !lower.as_bytes()[start - 1].is_ascii_alphanumeric()
                    && lower.as_bytes()[start - 1] != b'.'
                    && lower.as_bytes()[start - 1] != b'-';
            let after = &lower[start + needle.len()..];
            let word: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            if ok_before && !word.is_empty() {
                return Some(verdict(&word));
            }
            from = start + needle.len();
        }
        None
    };

    // The domain DMARC aligned against, when the server names it.
    let mut domain = None;
    for key in ["header.from=", "d="] {
        if let Some(at) = lower.find(key) {
            let after = &flat[at + key.len()..];
            let found: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            if !found.is_empty() {
                domain = Some(found.to_ascii_lowercase());
                break;
            }
        }
    }

    let out = Authentication {
        authserv,
        spf: read("spf"),
        dkim: read("dkim"),
        dmarc: read("dmarc"),
        domain,
    };

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
    /// Where the author asks replies to go, when that is not the From address.
    ///
    /// RFC 5322 §3.6.2. A list that rewrites it sends your answer to the list;
    /// a notification address that cannot receive mail uses it to point at one
    /// that can. Ignoring it addresses the reply at a mailbox nobody reads,
    /// which is a silent failure — the message leaves, and no one gets it.
    pub reply_to: Vec<(Option<String>, String)>,
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
    /// Every header, lowercased name and raw value, in the order the message
    /// carries them.
    ///
    /// Kept whole rather than picked over because a filter rule can name any
    /// header it likes — `X-Spam-Status`, `Precedence`, `Auto-Submitted` —
    /// and the parser cannot know in advance which one somebody will ask
    /// about. A message with two `Received` lines keeps both: a rule that
    /// asks about Received means any of them.
    pub headers: Vec<(String, String)>,
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

/// Collects every address from each header occurrence of the same name.
///
/// RFC 5322 still allows stacked To/Cc fields; real mail uses them, and
/// `msg.to()` / `msg.cc()` answer with only the last one. Ingest must walk
/// `all_to()` / `all_cc()` instead.
fn addr_lists<'a>(values: impl Iterator<Item = &'a Address<'a>>) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    for value in values {
        out.extend(addr_list(Some(value)));
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

/// The body, decoded ourselves when its charset is one the parser gets wrong.
///
/// mail-parser reads the charset off the part and hands the bytes to a decoder
/// that fails it in one of two ways. A refused charset answers with a single
/// U+FFFD, so a Korean message arrives as one replacement character and
/// nothing else. A charset whose name it cannot resolve is read as UTF-8
/// instead, so a Japanese message from a Windows mailer arrives as a row of
/// replacement characters. In both cases the part's transfer-decoded bytes are
/// taken straight from the part and put through our own decoder. Every charset
/// the parser handles correctly is left exactly where it was: this is a narrow
/// exception, not a second body pipeline.
fn body_of(msg: &mail_parser::Message<'_>, parsed: &[u8], html: bool) -> Option<String> {
    let part = if html {
        msg.html_bodies().next()
    } else {
        msg.text_bodies().next()
    };
    if let Some(part) = part
        && let Some(charset) = part.content_type().and_then(|ct| ct.attribute("charset"))
        && crate::charset::decodes_here(charset)
    {
        // Not `part.contents()`. mail-parser has already spent those bytes on
        // the decoder that gets this charset wrong, so by here the message is
        // gone or garbled. The part says where it sits and how it was
        // transferred, which is enough to go back to what actually arrived.
        let (from, to) = (part.offset_body as usize, part.offset_end as usize);
        if let Some(slice) = parsed.get(from..to.min(parsed.len()))
            && let Some(text) =
                crate::charset::decode(charset, &undo_transfer(part.encoding, slice))
        {
            return Some(text);
        }
    }
    if html {
        msg.body_html(0).map(|c| c.to_string())
    } else {
        msg.body_text(0).map(|c| c.to_string())
    }
}

/// Undoes the transfer encoding, which is the only thing between the wire and
/// the charset. Ordinarily mail-parser does this and we never see it; it is
/// needed here because the charset decode has to be ours and the two happen
/// together. Anything that will not decode is passed through rather than
/// dropped — a body that arrives slightly wrong beats one that does not arrive.
fn undo_transfer(encoding: mail_parser::Encoding, bytes: &[u8]) -> Vec<u8> {
    use base64::Engine as _;

    match encoding {
        mail_parser::Encoding::Base64 => {
            let tight: Vec<u8> = bytes
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            base64::engine::general_purpose::STANDARD
                .decode(&tight)
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&tight))
                .unwrap_or_else(|_| bytes.to_vec())
        }
        mail_parser::Encoding::QuotedPrintable => {
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
            while i < bytes.len() {
                if bytes[i] != b'=' {
                    out.push(bytes[i]);
                    i += 1;
                    continue;
                }
                match (bytes.get(i + 1).copied(), bytes.get(i + 2).copied()) {
                    // A soft line break carries nothing.
                    (Some(b'\r'), Some(b'\n')) => i += 3,
                    (Some(b'\n'), _) => i += 2,
                    (Some(hi), Some(lo)) => match (hex(hi), hex(lo)) {
                        (Some(hi), Some(lo)) => {
                            out.push((hi << 4) | lo);
                            i += 3;
                        }
                        // Not an escape after all: an equals sign is an
                        // equals sign.
                        _ => {
                            out.push(b'=');
                            i += 1;
                        }
                    },
                    _ => {
                        out.push(b'=');
                        i += 1;
                    }
                }
            }
            out
        }
        mail_parser::Encoding::None => bytes.to_vec(),
    }
}

/// Parses raw RFC822. Returns `None` only when the bytes yield no message at
/// all; malformed-but-present mail parses with whatever could be salvaged.
pub fn parse_message(raw: &[u8]) -> Option<ParsedMessage> {
    // mail-parser decodes each encoded-word to a string. Adjacent words that
    // split a UTF-8 character (い folded as E3 | 81 84) become U+FFFD unless
    // the octets are concatenated first. Adjacent ISO-2022-JP words that are
    // each a complete JIS run must *not* be concatenated — that is the other
    // U+FFFD, from encoding_rs seeing ESC ( B ESC $ B. The blob is not rewritten.
    let rewritten = merge_adjacent_encoded_words(raw);
    let parsed = rewritten.as_ref();
    let msg = MessageParser::default().parse(parsed)?;

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
        to: addr_lists(msg.all_to()),
        cc: addr_lists(msg.all_cc()),
        reply_to: addr_list(msg.reply_to()),
        date_ms: msg.date().map(|d| d.to_timestamp() * 1000),
        body_text: body_of(&msg, parsed, false).unwrap_or_default(),
        body_html: body_of(&msg, parsed, true),
        attachments,
        list_id: msg.header_raw("List-Id").map(|v| {
            let v = v.trim();
            match (v.rfind('<'), v.rfind('>')) {
                (Some(open), Some(close)) if close > open => v[open + 1..close].to_string(),
                _ => v.to_string(),
            }
        }),
        references,
        // Sliced out of the raw bytes by the offsets the parser recorded, not
        // looked up by name: `header_raw` answers with the *first* header of
        // a given name, so a message carrying several Received lines would
        // have come back holding the same one over and over.
        headers: msg
            .headers()
            .iter()
            .map(|h| {
                let value = parsed
                    .get(h.offset_start as usize..h.offset_end as usize)
                    .map(|b| String::from_utf8_lossy(b).trim().to_string())
                    .unwrap_or_default();
                (h.name().to_ascii_lowercase(), value)
            })
            .collect(),
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
    fn stacked_cc_headers_yield_every_address() {
        let raw = b"From: a@example.com\r\n\
To: me@example.com\r\n\
Cc: first@example.com\r\n\
Cc: second@example.com\r\n\
Subject: stacked cc\r\n\r\n\
body\r\n";
        let m = parse_message(raw).expect("parses");
        let addrs: Vec<&str> = m.cc.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            addrs,
            vec!["first@example.com", "second@example.com"],
            "stacked Cc must not drop the first header"
        );
    }

    #[test]
    fn stacked_to_headers_yield_every_address() {
        let raw = b"From: a@example.com\r\n\
To: first@example.com\r\n\
To: second@example.com\r\n\
Subject: stacked to\r\n\r\n\
body\r\n";
        let m = parse_message(raw).expect("parses");
        let addrs: Vec<&str> = m.to.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(
            addrs,
            vec!["first@example.com", "second@example.com"],
            "stacked To must not drop the first header"
        );
    }

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
    fn split_utf8_encoded_words_merge_to_one_subject() {
        // A folded Subject that splits い (E3 81 84) across two Base64 words.
        let raw = b"From: billing@example.com\r\n\
To: me@example.com\r\n\
Subject: =?utf-8?B?44GU5Yip55So5paZ6YeR44Gu44GK5pSv5omV4w==?=\r\n\
 =?utf-8?B?gYTjgYzlrozkuobjgZfjgb7jgZfjgZ8=?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n\
body\r\n";
        let m = parse_message(raw).expect("parses");
        let subject = m.subject.as_deref().expect("subject");
        assert_eq!(subject, "ご利用料金のお支払いが完了しました");
        assert!(
            !subject.contains('\u{FFFD}'),
            "split octets must not become replacement chars: {subject:?}"
        );
    }

    #[test]
    fn character_aligned_adjacent_utf8_b_words_still_decode() {
        let raw = b"From: =?utf-8?B?5p2x5Lqs?= <tokyo@example.com>\r\n\
Subject: =?utf-8?B?5p2x5Lqs?= =?utf-8?B?6KiI55S7?=\r\n\r\n\
\xe6\x9d\xb1\xe4\xba\xac\xe8\xa8\x88\xe7\x94\xbb\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("東京計画"));
        assert_eq!(m.from_display.as_deref(), Some("東京"));
        assert_eq!(
            m.body_text.trim(),
            "東京計画",
            "header/body split must survive the rewrite"
        );
    }

    #[test]
    fn different_charsets_are_not_merged() {
        // Adjacent words, different charsets — independent decode only.
        let raw = b"From: a@example.com\r\n\
Subject: =?utf-8?B?5p2x5Lqs?= =?ISO-2022-JP?B?GyRCMnE1RCRON28bKEI=?=\r\n\r\n\
x\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("東京会議の件"));

        let raw = b"From: a@example.com\r\n\
Subject: =?utf-8?B?5p2x5Lqs?= =?iso-8859-1?Q?caf=E9?=\r\n\r\n\
x\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("東京café"));
    }

    #[test]
    fn q_encoded_words_that_split_a_character_merge_too() {
        // The same い, split after its first byte, but as Q words.
        let raw = b"From: a@example.com\r\n\
Subject: =?utf-8?Q?=E3=81=8A=E6=94=AF=E6=89=95=E3?=\r\n\
 =?utf-8?Q?=81=84?=\r\n\r\n\
x\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("お支払い"));
    }

    #[test]
    fn self_contained_iso_2022_jp_folds_do_not_insert_fffd() {
        // これは / テスト / です, each a complete ESC $ B … ESC ( B run.
        // Concatenating the payloads is what inserts U+FFFD at the two joins.
        let raw = b"From: a@example.com\r\n\
Subject: =?ISO-2022-JP?B?GyRCJDMkbCRPGyhC?=\r\n\
 =?ISO-2022-JP?B?GyRCJUYlOSVIGyhC?=\r\n\
 =?ISO-2022-JP?B?GyRCJEckORsoQg==?=\r\n\r\n\
x\r\n";
        let m = parse_message(raw).expect("parses");
        let subject = m.subject.as_deref().expect("subject");
        assert_eq!(subject, "これはテストです");
        assert!(
            !subject.contains('\u{FFFD}'),
            "complete JIS runs must not grow replacement chars: {subject:?}"
        );
    }

    #[test]
    fn charset_case_does_not_keep_a_split_run_apart() {
        // One word says UTF-8, the next utf-8; the run is still one run.
        let raw = b"From: a@example.com\r\n\
Subject: =?UTF-8?B?44GK5pSv5omV4w==?=\r\n\
 =?utf-8?B?gYQ=?=\r\n\r\n\
x\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("お支払い"));
    }

    #[test]
    fn encoded_word_in_body_stays_literal() {
        let raw = b"From: a@example.com\r\n\
Subject: plain\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\n\
See =?utf-8?B?5p2x5Lqs?= in the body.\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(
            m.body_text.contains("=?utf-8?B?5p2x5Lqs?="),
            "body encoded-word must not be rewritten: {:?}",
            m.body_text
        );
        assert!(!m.body_text.contains("東京"));
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
    fn decodes_iso_2022_jp_headers_and_body() {
        // Japanese hosts still send this. Without mail-parser's full_encoding
        // feature the encoded-word Base64 unwraps and the JIS payload is left
        // as ESC sequences in the subject and the body.
        let mut raw = b"From: =?ISO-2022-JP?B?GyRCMnE1RCRON28bKEI=?= <info@example.jp>\r\n\
Subject: =?ISO-2022-JP?B?GyRCMnE1RCRON28bKEI=?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=ISO-2022-JP\r\n\
Content-Transfer-Encoding: 7bit\r\n\r\n"
            .to_vec();
        raw.extend_from_slice(b"\x1b$B$3$l$OK\\J8$G$9!#\x1b(B\r\n");
        let m = parse_message(&raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("会議の件"));
        assert_eq!(m.from_display.as_deref(), Some("会議の件"));
        assert!(m.body_text.contains("本文"), "body was {:?}", m.body_text);
    }

    #[test]
    fn decodes_shift_jis_body() {
        let mut raw = b"From: info@example.jp\r\n\
Subject: sjis\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=Shift_JIS\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n"
            .to_vec();
        raw.extend_from_slice(b"\x82\xb1\x82\xea\x82\xcd\x96{\x95\xb6\x82\xc5\x82\xb7\x81B\r\n");
        let m = parse_message(&raw).expect("parses");
        assert!(m.body_text.contains("本文"), "body was {:?}", m.body_text);
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

#[cfg(test)]
mod reply_to {
    use super::*;

    fn raw(headers: &str) -> Vec<u8> {
        format!(
            "From: Jordan Atwood <notifications@github.example>\r\n{headers}\
To: runelite/plugin-hub <plugin-hub@noreply.github.example>\r\n\
Subject: Re: a pull request\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain\r\n\r\nMerged.\r\n"
        )
        .into_bytes()
    }

    /// The header a notification address uses to point at one that works.
    #[test]
    fn reply_to_is_read_when_it_is_there() {
        let m = parse_message(&raw(
            "Reply-To: runelite/plugin-hub <reply+abc@reply.github.example>\r\n",
        ))
        .expect("parses");
        assert_eq!(
            m.reply_to,
            vec![(
                Some("runelite/plugin-hub".into()),
                "reply+abc@reply.github.example".into()
            )]
        );
        // The From is still the author, and still separate from it.
        assert_eq!(m.from_addr.as_deref(), Some("notifications@github.example"));
    }

    #[test]
    fn a_message_without_one_has_none() {
        let m = parse_message(&raw("")).expect("parses");
        assert!(
            m.reply_to.is_empty(),
            "invented a reply-to: {:?}",
            m.reply_to
        );
    }

    /// The field takes an address list, and a discussion list may name several.
    #[test]
    fn several_reply_to_addresses_all_arrive() {
        let m = parse_message(&raw(
            "Reply-To: One <one@example.com>, Two <two@example.com>\r\n",
        ))
        .expect("parses");
        let addrs: Vec<&str> = m.reply_to.iter().map(|(_, a)| a.as_str()).collect();
        assert_eq!(addrs, vec!["one@example.com", "two@example.com"]);
    }

    /// It is not a participant: keeping it out of the index means no schema
    /// change and no re-extraction, and searching for a list's reply address
    /// was never a thing anyone asked for.
    #[test]
    fn reply_to_stays_out_of_the_address_index() {
        let m =
            parse_message(&raw("Reply-To: <reply+abc@reply.github.example>\r\n")).expect("parses");
        let roles: Vec<&str> = m.addresses().iter().map(|(r, _, _)| *r).collect();
        assert!(!roles.contains(&"reply-to"), "roles were {roles:?}");
    }
}

/// The four charsets the Encoding Standard answers with a single U+FFFD.
///
/// Not a theoretical gap: ISO-2022-KR was the registered charset for Korean
/// mail for years, so it is what an old archive is written in. Before this, the
/// subject and the body of such a message both arrived as one replacement
/// character — not garbled, gone.
#[cfg(test)]
mod replacement_charsets {
    use super::*;

    #[test]
    fn a_korean_subject_and_body_both_read() {
        let raw = b"From: a@example.com\r\n\
Subject: =?ISO-2022-KR?B?G yRCKUM=?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=ISO-2022-KR\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n\
\x1b$)CHello \x0eGQ1[\x0f!\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(
            m.body_text.contains("한글"),
            "the body was lost: {:?}",
            m.body_text
        );
        assert!(
            !m.body_text.trim().starts_with('\u{FFFD}'),
            "the body is still a replacement character: {:?}",
            m.body_text
        );
    }

    #[test]
    fn a_korean_encoded_word_subject_reads() {
        // "한글" as ISO-2022-KR inside one encoded-word.
        let raw = b"From: a@example.com\r\n\
Subject: =?ISO-2022-KR?B?DkdRMVsP?=\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nx\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("한글"));
    }

    #[test]
    fn an_hz_body_reads() {
        let raw = b"From: a@example.com\r\nSubject: hz\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=HZ-GB-2312\r\n\
Content-Transfer-Encoding: 7bit\r\n\r\n\
Hi ~{VPND~}!\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(m.body_text.contains("中文"), "body was {:?}", m.body_text);
    }

    /// The charsets that were already fine must not have moved.
    #[test]
    fn the_working_charsets_are_untouched() {
        let raw = b"From: a@example.com\r\nSubject: =?EUC-KR?B?x9Gx2w==?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=EUC-KR\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n\xc7\xd1\xb1\xdb\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("한글"));
        assert!(m.body_text.contains("한글"), "body was {:?}", m.body_text);
    }
}

/// The other way a charset fails: not refused, but unrecognised. `encoding_rs`
/// resolves labels from a fixed list, and the vendor spellings Windows mailers
/// send are not on it. An unresolvable label falls back to reading the bytes as
/// UTF-8, so these messages were garbled rather than erased — a row of
/// replacement characters where Japanese, Korean or Chinese text should be.
#[cfg(test)]
mod vendor_charset_names {
    use super::*;

    #[test]
    fn a_cp932_subject_and_body_both_read() {
        let raw = b"From: a@example.com\r\n\
Subject: =?CP932?B?grGC6g==?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=CP932\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n\
\x82\xb1\x82\xea\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("これ"));
        assert!(m.body_text.contains("これ"), "body was {:?}", m.body_text);
        assert!(
            !m.body_text.contains('\u{FFFD}'),
            "body still garbled: {:?}",
            m.body_text
        );
    }

    #[test]
    fn the_korean_and_chinese_vendor_names_read_too() {
        for (label, b64, bytes, want) in [
            ("CP949", "x9Gx2w==", &b"\xc7\xd1\xb1\xdb"[..], "한글"),
            ("UHC", "x9Gx2w==", &b"\xc7\xd1\xb1\xdb"[..], "한글"),
            ("CP936", "1tDOxA==", &b"\xd6\xd0\xce\xc4"[..], "中文"),
            ("CP950", "pKSk5Q==", &b"\xa4\xa4\xa4\xe5"[..], "中文"),
            ("windows-950", "pKSk5Q==", &b"\xa4\xa4\xa4\xe5"[..], "中文"),
        ] {
            let mut raw = Vec::new();
            raw.extend_from_slice(b"From: a@example.com\r\n");
            raw.extend_from_slice(format!("Subject: =?{label}?B?{b64}?=\r\n").as_bytes());
            raw.extend_from_slice(b"MIME-Version: 1.0\r\n");
            raw.extend_from_slice(
                format!("Content-Type: text/plain; charset={label}\r\n").as_bytes(),
            );
            raw.extend_from_slice(b"Content-Transfer-Encoding: 8bit\r\n\r\n");
            raw.extend_from_slice(bytes);
            raw.extend_from_slice(b"\r\n");
            let m = parse_message(&raw).expect("parses");
            assert_eq!(m.subject.as_deref(), Some(want), "{label} subject");
            assert!(
                m.body_text.contains(want),
                "{label} body was {:?}",
                m.body_text
            );
        }
    }

    /// A lone encoded-word is normally left alone, because there is nothing to
    /// rejoin. That is exactly the case this used to miss: nothing to merge,
    /// but still a name the parser cannot resolve.
    #[test]
    fn a_single_encoded_word_is_enough_to_need_rescuing() {
        let raw = b"From: a@example.com\r\nSubject: =?CP932?B?grGC6g==?=\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nx\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("これ"));
    }

    /// An ISO-2022-JP extension names sets we have no table for, but its
    /// common case is the base one. Before this a lone word in it showed the
    /// reader raw escape sequences.
    #[test]
    fn an_iso_2022_jp_extension_reads_its_base_repertoire() {
        let raw = b"From: a@example.com\r\nSubject: =?ISO-2022-JP-2?B?GyRCJDMkbBsoQg==?=\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nx\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("これ"));
    }

    /// The reason the run is rejoined before it is decoded rather than after.
    /// Here the encoder split これ down the middle, so the second byte of the
    /// first character is in one word and its partner is in the next.
    #[test]
    fn a_character_split_across_two_words_survives() {
        let raw = b"From: a@example.com\r\n\
Subject: =?CP932?B?grGC?= =?CP932?B?6g==?=\r\n\
MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nx\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("これ"));
    }

    /// The names that already resolved must decode exactly as before.
    #[test]
    fn the_names_the_parser_knows_are_untouched() {
        let raw = b"From: a@example.com\r\nSubject: =?Shift_JIS?B?grGC6g==?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=Shift_JIS\r\n\
Content-Transfer-Encoding: 8bit\r\n\r\n\x82\xb1\x82\xea\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("これ"));
        assert!(m.body_text.contains("これ"), "body was {:?}", m.body_text);
    }

    /// Plain UTF-8 under an unknown label still reads, because reading it as
    /// UTF-8 was the fallback all along. Nothing here may break that.
    #[test]
    fn utf8_under_a_name_we_cannot_map_is_still_read_as_utf8() {
        let raw = "From: a@example.com\r\nSubject: t\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=x-nonsense-9\r\n\r\nこれ\r\n";
        let m = parse_message(raw.as_bytes()).expect("parses");
        assert!(m.body_text.contains("これ"), "body was {:?}", m.body_text);
    }
}

/// The names nobody can place, where the old answer was to read the bytes as
/// UTF-8 and replace whatever would not decode. That threw the message away
/// twice over: the reader saw nothing and the index kept nothing.
#[cfg(test)]
mod unplaceable_charsets {
    use super::*;

    fn message(charset: &str, body: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"From: a@example.com\r\nSubject: t\r\nMIME-Version: 1.0\r\n");
        raw.extend_from_slice(
            format!("Content-Type: text/plain; charset={charset}\r\n").as_bytes(),
        );
        raw.extend_from_slice(b"Content-Transfer-Encoding: 8bit\r\n\r\n");
        raw.extend_from_slice(body);
        raw.extend_from_slice(b"\r\n");
        raw
    }

    /// `unknown-8bit` is RFC 1428's way of saying the sender does not know
    /// either. Western text is much the likeliest thing behind it.
    #[test]
    fn latin1_under_an_unknown_label_reads_correctly() {
        let m = parse_message(&message("unknown-8bit", b"caf\xe9 na\xefve")).expect("parses");
        assert!(
            m.body_text.contains("café naïve"),
            "body was {:?}",
            m.body_text
        );
        assert!(!m.body_text.contains('\u{FFFD}'), "body was replaced");
    }

    #[test]
    fn utf8_under_an_unknown_label_is_still_utf8() {
        let m = parse_message(&message("x-nonsense-9", "日本語".as_bytes())).expect("parses");
        assert!(m.body_text.contains("日本語"), "body was {:?}", m.body_text);
    }

    /// Not readable, but not destroyed either: the bytes survive into the
    /// index and View Source, which U+FFFD does not allow.
    #[test]
    fn a_charset_with_no_table_here_is_preserved_rather_than_replaced() {
        let m = parse_message(&message("euc-tw", b"\xa4\xa4\xa4\xe5")).expect("parses");
        assert!(
            !m.body_text.contains('\u{FFFD}'),
            "bytes destroyed: {:?}",
            m.body_text
        );
    }

    #[test]
    fn utf7_reads_in_the_body_and_the_subject() {
        let raw = b"From: a@example.com\r\nSubject: =?UTF-7?Q?+ZeVnLIqe-?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=UTF-7\r\n\
Content-Transfer-Encoding: 7bit\r\n\r\n\
Hi Mom -+Jjo--!\r\n";
        let m = parse_message(raw).expect("parses");
        assert_eq!(m.subject.as_deref(), Some("日本語"));
        assert!(
            m.body_text.contains("Hi Mom -\u{263A}-!"),
            "body was {:?}",
            m.body_text
        );
    }

    /// A body that really was UTF-8 and lost a byte must keep the rest of its
    /// text rather than be thrown wholesale at windows-1252.
    #[test]
    fn a_clipped_utf8_body_keeps_the_text_around_the_damage() {
        let mut body = "Ready for review, and the rest reads fine. "
            .as_bytes()
            .to_vec();
        body.extend_from_slice(b"\xe2\x80");
        body.extend_from_slice(b" Ask Ren\xc3\xa9e about the rollout schedule.");
        let m = parse_message(&message("unknown-8bit", &body)).expect("parses");
        assert!(
            m.body_text.contains("Ready for review"),
            "{:?}",
            m.body_text
        );
        assert!(m.body_text.contains("Renée"), "mangled: {:?}", m.body_text);
        assert!(
            !m.body_text.contains("â€"),
            "fell to 1252: {:?}",
            m.body_text
        );
    }

    /// Nothing above may reach a charset the parser handles perfectly well.
    #[test]
    fn the_names_the_parser_knows_are_untouched() {
        let m = parse_message(&message("iso-8859-1", b"caf\xe9")).expect("parses");
        assert!(m.body_text.contains("café"), "body was {:?}", m.body_text);
        let u = parse_message(&message("utf-8", "日本語".as_bytes())).expect("parses");
        assert!(u.body_text.contains("日本語"), "body was {:?}", u.body_text);
    }
}

#[cfg(test)]
mod replacement_charset_transfers {
    use super::*;
    use base64::Engine as _;

    fn message(transfer: &str, body: &[u8]) -> Vec<u8> {
        let mut raw = format!(
            "From: a@example.com\r\nSubject: t\r\nMIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=ISO-2022-KR\r\n\
Content-Transfer-Encoding: {transfer}\r\n\r\n"
        )
        .into_bytes();
        raw.extend_from_slice(body);
        raw.extend_from_slice(b"\r\n");
        raw
    }

    /// The charset decode is ours, so undoing the transfer has to be too.
    #[test]
    fn it_reads_through_every_transfer_encoding() {
        let korean: &[u8] = b"\x1b$)C\x0eGQ1[\x0f";
        let plain = message("8bit", korean);
        assert!(parse_message(&plain).unwrap().body_text.contains("한글"));

        let b64 = base64::engine::general_purpose::STANDARD.encode(korean);
        let encoded = message("base64", b64.as_bytes());
        assert!(
            parse_message(&encoded).unwrap().body_text.contains("한글"),
            "base64 body was {:?}",
            parse_message(&encoded).unwrap().body_text
        );

        // Quoted-printable, with a soft break in the middle of the run.
        let qp = message("quoted-printable", b"=1B$)C=0EGQ=\r\n1[=0F");
        assert!(
            parse_message(&qp).unwrap().body_text.contains("한글"),
            "quoted-printable body was {:?}",
            parse_message(&qp).unwrap().body_text
        );
    }

    /// One part of a multipart message, not the whole thing.
    #[test]
    fn it_finds_the_part_inside_a_multipart() {
        let raw = b"From: a@example.com\r\nSubject: t\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain; charset=ISO-2022-KR\r\n\r\n\
\x1b$)C\x0eGQ1[\x0f\r\n\
--b\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n\
<p>plain ascii</p>\r\n--b--\r\n";
        let m = parse_message(raw).expect("parses");
        assert!(m.body_text.contains("한글"), "text was {:?}", m.body_text);
        // The other part was never ours and must be untouched.
        assert!(
            m.body_html.as_deref().unwrap_or("").contains("plain ascii"),
            "html was {:?}",
            m.body_html
        );
    }

    /// A message that says one of these and then carries nothing is still a
    /// message; it must not come back as a parse failure.
    #[test]
    fn an_empty_body_is_still_a_message() {
        let m = parse_message(&message("8bit", b"")).expect("parses");
        assert!(m.body_text.trim().is_empty(), "body was {:?}", m.body_text);
    }
}

#[cfg(test)]
mod quoted_pictures {
    use super::*;

    /// The PNG signature, eight bytes: all a part needs to be a picture here.
    const PNG: &str = "iVBORw0KGgo=";

    fn message(parts: &str) -> Vec<u8> {
        format!(
            "From: a@example.com\r\nTo: me@example.com\r\nSubject: pictures\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>see</p>\r\n\
{parts}--b--\r\n"
        )
        .into_bytes()
    }

    fn picture(cid: &str, mime: &str) -> String {
        format!(
            "--b\r\nContent-Type: {mime}\r\nContent-Transfer-Encoding: base64\r\n\
Content-ID: <{cid}>\r\n\r\n{PNG}\r\n"
        )
    }

    #[test]
    fn a_picture_the_body_shows_is_written_in_as_data() {
        let raw = message(&picture("logo@x", "image/png"));
        let out = embed_cid_images(r#"<p>Logo</p><img src="cid:logo@x">"#, &raw);
        assert_eq!(
            out,
            format!(r#"<p>Logo</p><img src="data:image/png;base64,{PNG}">"#)
        );
    }

    /// Sent as it was, a cid the reply does not carry arrives broken.
    #[test]
    fn a_picture_the_reply_cannot_carry_is_left_out() {
        let raw = message(&picture("logo@x", "image/png"));
        let out = embed_cid_images(r#"<img src="cid:gone@x">"#, &raw);
        assert_eq!(out, r#"<img data-cid="gone@x">"#);
    }

    #[test]
    fn svg_is_never_embedded() {
        let raw = message(&picture("art@x", "image/svg+xml"));
        let out = embed_cid_images(r#"<img src="cid:art@x">"#, &raw);
        assert!(!out.contains("data:"), "{out}");
        assert!(!out.contains("src="), "{out}");
    }

    /// An inline part nothing refers to is an attachment in all but name, and
    /// a forward attaches it. Only what the body shows travels in the body.
    #[test]
    fn only_the_pictures_the_body_shows_are_quoted() {
        let raw = message(&format!(
            "{}{}",
            picture("shown@x", "image/png"),
            picture("unshown@x", "image/png")
        ));
        let pictures = quoted_pictures(r#"<img src="cid:shown@x">"#, &raw);
        let parts: Vec<usize> = pictures.iter().map(|p| p.part).collect();
        assert_eq!(parts, vec![0]);
    }

    #[test]
    fn pictures_past_either_limit_are_left_out() {
        let raw = message(&format!(
            "{}{}",
            picture("one@x", "image/png"),
            picture("two@x", "image/png")
        ));
        let html = r#"<img src="cid:one@x"><img src="cid:two@x">"#;
        assert!(quoted_pictures_within(html, &raw, 4, 1024).is_empty());
        let fitted: Vec<usize> = quoted_pictures_within(html, &raw, 8, 8)
            .iter()
            .map(|p| p.part)
            .collect();
        assert_eq!(fitted, vec![0], "the second would overspend the budget");
    }

    #[test]
    fn an_image_jpg_part_is_embedded_as_jpeg() {
        let raw = message(&picture("photo@x", "image/jpg"));
        let out = embed_cid_images(r#"<img src="cid:photo@x">"#, &raw);
        assert!(out.contains("data:image/jpeg;base64,"), "{out}");
    }
}
