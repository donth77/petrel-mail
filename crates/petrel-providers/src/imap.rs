//! IMAP backend — M0 slice.
//!
//! Establishes a session, reads the server's capabilities, and derives the sync
//! strategy from them: QRESYNC → CONDSTORE → full reconcile. The ladder is the
//! whole point; Gmail-over-IMAP has no QRESYNC and Microsoft 365 has no
//! CONDSTORE, so the bottom rung is the common case, not an edge case.

use std::sync::Arc;
use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use async_imap::{Client, Session};
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("imap: {0}")]
    Imap(#[from] async_imap::error::Error),
    #[error("tls: {0}")]
    Tls(String),
    #[error("protocol: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, ImapError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Implicit TLS (RFC 8314, port 993) — the shipping path.
    Tls,
    /// Loopback-only, test builds only. See the crate feature of the same name.
    #[cfg(feature = "insecure-plaintext")]
    InsecurePlaintext,
}

#[derive(Debug, Clone)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub credential: Credential,
    pub security: Security,
}

/// How an account proves who it is.
///
/// A password and a bearer token are not interchangeable strings: they go on
/// the wire through different commands, and a token is short-lived where a
/// password is not. Keeping them apart in the type is what stops a refreshed
/// token being handed to LOGIN, which fails in a way that reads exactly like a
/// wrong password.
///
/// The token is expected to be *fresh*. Nothing down here knows about the
/// keychain or when a token expires; the caller refreshes and hands over one
/// that works, because a provider that tried to renew its own credentials
/// would need to know where they are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    Password(String),
    Bearer(String),
}

impl Credential {
    pub fn password(p: impl Into<String>) -> Self {
        Credential::Password(p.into())
    }
}

/// How the engine must sync a given server, derived from its capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStrategy {
    /// Cheapest: resync a mailbox from a modseq, including VANISHED (EARLIER).
    Qresync,
    /// Diff by HIGHESTMODSEQ; expunges still need reconciliation.
    Condstore,
    /// Nothing to lean on: compare the full UID/flag set against local state.
    FullReconcile,
}

#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub raw: Vec<String>,
    pub idle: bool,
    pub condstore: bool,
    pub qresync: bool,
    pub objectid: bool,
    pub compress: bool,
    pub uidplus: bool,
    pub move_: bool,
    pub special_use: bool,
}

impl Capabilities {
    pub fn strategy(&self) -> SyncStrategy {
        if self.qresync {
            SyncStrategy::Qresync
        } else if self.condstore {
            SyncStrategy::Condstore
        } else {
            SyncStrategy::FullReconcile
        }
    }
}

#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub name: String,
    pub delimiter: Option<String>,
    pub attributes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SelectedFolder {
    pub exists: u32,
    pub uid_validity: Option<u32>,
    pub uid_next: Option<u32>,
    pub highest_modseq: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct FetchedHeader {
    pub uid: Option<u32>,
    pub subject: String,
    pub from: String,
    pub size: Option<u32>,
    pub seen: bool,
}

/// One round trip's worth of evidence about a server, used by the M0 spike and
/// (later) by account setup to record a provider profile.
#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub greeting_capabilities: Capabilities,
    pub strategy: SyncStrategy,
    pub folders: Vec<FolderInfo>,
    pub inbox: SelectedFolder,
    pub headers: Vec<FetchedHeader>,
}

/// async-imap surfaces capabilities as typed variants; their Debug forms are
/// `Imap4rev1`, `Atom("IDLE")`, `Auth("XOAUTH2")`. Normalise to wire tokens so
/// stored provider profiles read like the server's own CAPABILITY line.
fn capability_token(debug: &str) -> String {
    match (debug.find('"'), debug.rfind('"')) {
        (Some(start), Some(end)) if end > start => {
            let inner = &debug[start + 1..end];
            if debug.starts_with("Auth(") {
                format!("AUTH={inner}")
            } else {
                inner.to_string()
            }
        }
        _ => debug.to_ascii_uppercase(),
    }
}

fn parse_capabilities(items: impl Iterator<Item = String>) -> Capabilities {
    let raw: Vec<String> = items.collect();
    let has = |needle: &str| raw.iter().any(|c| c.eq_ignore_ascii_case(needle));
    Capabilities {
        idle: has("IDLE"),
        condstore: has("CONDSTORE"),
        qresync: has("QRESYNC"),
        objectid: has("OBJECTID"),
        compress: has("COMPRESS=DEFLATE"),
        uidplus: has("UIDPLUS"),
        move_: has("MOVE"),
        special_use: has("SPECIAL-USE"),
        raw,
    }
}

/// The same TLS setup, for callers outside this module (SMTP submission).
pub(crate) async fn tls_stream_for(
    host: &str,
    port: u16,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    tls_stream(host, port).await
}

async fn tls_stream(host: &str, port: u16) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // TODO(M4): swap for the OS trust store so corporate/self-signed CAs work,
    // alongside the explicit per-host pinning flow for self-hosters.
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect((host, port)).await?;
    let name = ServerName::try_from(host.to_string()).map_err(|e| ImapError::Tls(e.to_string()))?;
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| ImapError::Tls(e.to_string()))
}

/// Runs the probe over an established transport. Generic so the TLS and
/// (test-only) plaintext paths share one implementation.
async fn probe_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    fetch_limit: u32,
) -> Result<ProbeReport>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;

    let caps = session.capabilities().await?;
    let capabilities = parse_capabilities(caps.iter().map(|c| capability_token(&format!("{c:?}"))));

    let mut folders = Vec::new();
    {
        let mut names = session.list(Some(""), Some("*")).await?;
        while let Some(name) = names.next().await {
            let name = name?;
            folders.push(FolderInfo {
                name: name.name().to_string(),
                delimiter: name.delimiter().map(|d| d.to_string()),
                attributes: name.attributes().iter().map(|a| format!("{a:?}")).collect(),
            });
        }
    }

    let mailbox = session.select("INBOX").await?;
    let inbox = SelectedFolder {
        exists: mailbox.exists,
        uid_validity: mailbox.uid_validity,
        uid_next: mailbox.uid_next,
        highest_modseq: mailbox.highest_modseq,
    };

    let mut headers = Vec::new();
    // fetch_limit 0 is LIST + capabilities + SELECT only — no header sampling.
    if fetch_limit > 0 && mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(fetch_limit.saturating_sub(1)).max(1);
        let range = format!("{first}:{last}");
        // ENVELOPE chokes on UTF-8 quoted-strings some servers emit; parse headers locally.
        if let Ok(mut fetches) = session
            .fetch(
                range,
                "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (DATE FROM SUBJECT)])",
            )
            .await
        {
            while let Some(fetch) = fetches.next().await {
                let Ok(fetch) = fetch else { continue };
                let Some(uid) = fetch.uid else { continue };
                let (subject, from) = fetch
                    .header()
                    .and_then(petrel_mime::parse_message)
                    .map(|parsed| {
                        (
                            parsed.subject.unwrap_or_default(),
                            parsed.from_addr.unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default();
                headers.push(FetchedHeader {
                    uid: Some(uid),
                    subject,
                    from,
                    size: fetch.size,
                    seen: fetch
                        .flags()
                        .any(|f| matches!(f, async_imap::types::Flag::Seen)),
                });
            }
        }
    }

    session.logout().await?;

    Ok(ProbeReport {
        strategy: capabilities.strategy(),
        greeting_capabilities: capabilities,
        folders,
        inbox,
        headers,
    })
}

/// Appends a message to a folder (used by tests to seed a mailbox).
pub async fn append_message(
    cfg: &ImapConfig,
    folder: &str,
    flags: Option<&str>,
    raw: &[u8],
) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            let mut session = sign_in(client, cfg).await?;
            session.append(folder, flags, None, raw).await?;
            session.logout().await?;
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            let client = Client::new(tcp);
            let mut session = sign_in(client, cfg).await?;
            session.append(folder, flags, None, raw).await?;
            session.logout().await?;
        }
    }
    Ok(())
}

/// Fetches whole messages (`RFC822`) with their UIDs — the bytes the engine
/// stores verbatim and parses. Newest `limit` messages in the folder.
/// The IMAP system flags we care about, as the engine's bits.
///
/// Deliberately the engine's numbering rather than a provider type: a message's
/// read state is a fact about the mailbox, not about the protocol that
/// delivered it, and every caller downstream already speaks these bits.
pub mod flag_bits {
    pub const SEEN: i64 = 1 << 0;
    pub const ANSWERED: i64 = 1 << 1;
    pub const FLAGGED: i64 = 1 << 2;
    pub const DRAFT: i64 = 1 << 3;
    pub const DELETED: i64 = 1 << 4;
}

/// Maps a fetch's FLAGS into those bits. Unknown and custom flags — Gmail's
/// labels arrive this way — are ignored rather than guessed at.
fn flags_to_bits<'a>(flags: impl Iterator<Item = async_imap::types::Flag<'a>>) -> i64 {
    use async_imap::types::Flag;
    let mut bits = 0;
    for f in flags {
        bits |= match f {
            Flag::Seen => flag_bits::SEEN,
            Flag::Answered => flag_bits::ANSWERED,
            Flag::Flagged => flag_bits::FLAGGED,
            Flag::Draft => flag_bits::DRAFT,
            Flag::Deleted => flag_bits::DELETED,
            _ => 0,
        };
    }
    bits
}

/// The custom keywords a fetch carries: everything that is not a system
/// flag. Gmail's labels arrive by this door too, which is why the caller
/// only asks on accounts whose tags are keywords rather than labels.
fn keywords_of<'a>(flags: impl Iterator<Item = async_imap::types::Flag<'a>>) -> Vec<String> {
    use async_imap::types::Flag;
    flags
        .filter_map(|f| match f {
            Flag::Custom(name) => {
                let n = name.trim().to_string();
                // `\*` is the server saying "custom keywords allowed", not a
                // keyword; nothing wears it.
                (!n.is_empty() && n != "*").then_some(n)
            }
            _ => None,
        })
        .collect()
}

/// Fetches recent messages, handing each one to `on_message` as it arrives.
///
/// The buffering version holds every message in memory and returns only once
/// the last one lands, so a caller can show no progress and hold no mail until
/// the whole batch is done — which reads as a hang, and on a large mailbox is
/// one in every sense that matters. Streaming lets the store take each message
/// as it comes and the UI count climb while it does.
/// Fetches the newest `limit` messages, reporting the mailbox's UIDVALIDITY
/// alongside the count so a first sync can adopt it — UIDs recorded without
/// their validity are UIDs a later reset cannot be detected against.
pub async fn fetch_raw_each<F>(
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    on_message: F,
) -> Result<(usize, Option<u32>)>
where
    F: FnMut(u32, i64, &[u8]),
{
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            fetch_each_session(client, cfg, folder, limit, on_message).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            fetch_each_session(Client::new(tcp), cfg, folder, limit, on_message).await
        }
    }
}

async fn fetch_each_session<S, F>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    mut on_message: F,
) -> Result<(usize, Option<u32>)>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(u32, i64, &[u8]),
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(folder).await?;
    let mut n = 0usize;
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        // FLAGS, not just the body. Without them every message ingests with no
        // read state and shows as unread — a mailbox with nothing unread in it
        // arrives looking like hundreds of unread conversations.
        let mut fetches = session
            .fetch(format!("{first}:{last}"), "(UID FLAGS RFC822)")
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            let bits = flags_to_bits(fetch.flags());
            if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                on_message(uid, bits, body);
                n += 1;
            }
        }
    }
    let uid_validity = mailbox.uid_validity;
    session.logout().await?;
    Ok((n, uid_validity))
}

pub async fn fetch_raw(cfg: &ImapConfig, folder: &str, limit: u32) -> Result<Vec<(u32, Vec<u8>)>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            fetch_raw_session(client, cfg, folder, limit).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            fetch_raw_session(Client::new(tcp), cfg, folder, limit).await
        }
    }
}

async fn fetch_raw_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
) -> Result<Vec<(u32, Vec<u8>)>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(folder).await?;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        let mut fetches = session
            .fetch(format!("{first}:{last}"), "(UID RFC822)")
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                out.push((uid, body.to_vec()));
            }
        }
    }
    session.logout().await?;
    Ok(out)
}

/// Where Gmail keeps every message, keyed by its Message-ID header.
///
/// Swept from All Mail, which is the only mailbox that holds everything, and
/// the only place the `\Inbox` label is reported — Gmail omits the label of the
/// mailbox you are already looking at, so asking INBOX whether a message is in
/// the inbox always answers no.
///
/// Keyed on the Message-ID rather than the UID because UIDs are per-mailbox: a
/// message has one number in All Mail and a different one in INBOX, and nothing
/// connects them. The header is the same wherever it is read from.
///
/// Labels only, plus one header field — a few dozen bytes per message against
/// twelve kilobytes for a body.
///
/// Incremental where the server allows it. Sweeping the whole mailbox every
/// sync is fine at fifteen hundred messages and hopeless at a hundred thousand,
/// so with CONDSTORE the second sweep onward asks only for what has changed
/// since the last one — which is usually nothing, and costs a round trip.
///
/// Returns the mailbox's modification sequence alongside the labels, to be
/// handed back on the next call.
pub struct LabelSweep {
    pub labels: Vec<(String, Vec<String>)>,
    /// Pass to the next sweep as `since`. None when the server has no CONDSTORE
    /// and every sweep must therefore be a full one.
    pub modseq: Option<u64>,
}

/// One folder's Gmail thread ids, `(uid, thrid)` pairs.
pub struct ThridSweep {
    pub thrids: Vec<(u32, u64)>,
    /// Pass to the next sweep as `since`; None without CONDSTORE.
    pub modseq: Option<u64>,
}

/// Gmail's own conversation ids, straight from `X-GM-THRID`.
///
/// The typed IMAP client parses this attribute but gives no way to read it
/// back, so this speaks the four lines of protocol itself over a raw
/// connection: LOGIN, EXAMINE with CONDSTORE, one FETCH, LOGOUT. Safe to
/// hand-parse because a THRID fetch response is numbers on one line — no
/// literals, no continuations. Same shape as the label sweep: bounded on the
/// first pass, CHANGEDSINCE after, so it costs one round trip when nothing
/// changed.
pub async fn sweep_gmail_thrids(
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    since: Option<u64>,
) -> Result<ThridSweep> {
    match cfg.security {
        Security::Tls => {
            let stream = tls_stream(&cfg.host, cfg.port).await?;
            raw_thrid_exchange(stream, cfg, folder, limit, since).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let stream = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            raw_thrid_exchange(stream, cfg, folder, limit, since).await
        }
    }
}

async fn raw_thrid_exchange<S>(
    stream: S,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    since: Option<u64>,
) -> Result<ThridSweep>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let quote = |v: &str| format!("\"{}\"", v.replace('\\', "\\\\").replace('\"', "\\\""));
    let (read_half, mut write) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();

    // One tagged command, drained to its tag. Returns the untagged lines.
    async fn exchange(
        lines: &mut tokio::io::Lines<impl AsyncBufReadExt + Unpin>,
        write: &mut (impl AsyncWriteExt + Unpin),
        tag: &str,
        cmd: &str,
    ) -> Result<Vec<String>> {
        write
            .write_all(format!("{tag} {cmd}\r\n").as_bytes())
            .await?;
        write.flush().await?;
        let mut out = Vec::new();
        while let Some(line) = lines.next_line().await? {
            if let Some(rest) = line.strip_prefix(tag) {
                let rest = rest.trim_start();
                if rest.starts_with("OK") {
                    return Ok(out);
                }
                return Err(ImapError::Protocol(format!(
                    "{cmd_name}: {rest}",
                    cmd_name = cmd.split(' ').next().unwrap_or(cmd)
                )));
            }
            out.push(line);
        }
        Err(ImapError::Protocol("connection closed mid-command".into()))
    }

    // Greeting first.
    let greeting = lines.next_line().await?.unwrap_or_default();
    if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
        return Err(ImapError::Protocol(format!("greeting: {greeting}")));
    }
    // This path writes the protocol by hand rather than going through the
    // client, so it needs its own line for each credential kind. SASL-IR
    // (RFC 4959) lets the token ride on the AUTHENTICATE command itself,
    // which every server offering XOAUTH2 supports.
    let sign_in_line = match &cfg.credential {
        Credential::Password(pass) => {
            format!("LOGIN {} {}", quote(&cfg.user), quote(pass))
        }
        Credential::Bearer(token) => {
            use base64::Engine as _;
            format!(
                "AUTHENTICATE XOAUTH2 {}",
                base64::engine::general_purpose::STANDARD.encode(xoauth2_payload(&cfg.user, token))
            )
        }
    };
    exchange(&mut lines, &mut write, "t1", &sign_in_line).await?;
    let opened = exchange(
        &mut lines,
        &mut write,
        "t2",
        &format!("EXAMINE {} (CONDSTORE)", quote(folder)),
    )
    .await?;
    let mut modseq: Option<u64> = None;
    let mut exists: u32 = 0;
    for line in &opened {
        if let Some(i) = line.find("HIGHESTMODSEQ ") {
            modseq = line[i + 14..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|d| d.parse().ok());
        }
        if let Some(n) = line
            .strip_prefix("* ")
            .and_then(|r| r.strip_suffix(" EXISTS"))
        {
            exists = n.trim().parse().unwrap_or(0);
        }
    }

    let mut thrids = Vec::new();
    if exists > 0 {
        let cmd = match since {
            Some(m) => format!("FETCH 1:* (UID X-GM-THRID) (CHANGEDSINCE {m})"),
            None => {
                let first = exists.saturating_sub(limit.saturating_sub(1)).max(1);
                format!("FETCH {first}:{exists} (UID X-GM-THRID)")
            }
        };
        let rows = exchange(&mut lines, &mut write, "t3", &cmd).await?;
        for line in rows {
            if !line.starts_with("* ") || !line.contains(" FETCH ") {
                continue;
            }
            let uid = line.find("UID ").and_then(|i| {
                line[i + 4..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            });
            let thrid = line.find("X-GM-THRID ").and_then(|i| {
                line[i + 11..]
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            });
            if let (Some(u), Some(t)) = (uid, thrid) {
                thrids.push((u, t));
            }
        }
    }
    let _ = exchange(&mut lines, &mut write, "t4", "LOGOUT").await;
    Ok(ThridSweep { thrids, modseq })
}

pub async fn sweep_gmail_labels(
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    since: Option<u64>,
) -> Result<LabelSweep> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            sweep_labels_session(client, cfg, folder, limit, since).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            sweep_labels_session(Client::new(tcp), cfg, folder, limit, since).await
        }
    }
}

async fn sweep_labels_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    since: Option<u64>,
) -> Result<LabelSweep>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    // EXAMINE: reading where mail lives must not mark any of it seen.
    let mailbox = session.examine(folder).await?;
    let modseq = mailbox.highest_modseq;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        const ITEMS: &str = "(X-GM-LABELS BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])";
        // Everything that changed, wherever it sits — against the newest slice
        // when there is no watermark to work from.
        let (range, query) = match since {
            Some(m) => ("1:*".to_string(), format!("{ITEMS} (CHANGEDSINCE {m})")),
            None => {
                let last = mailbox.exists;
                let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
                (format!("{first}:{last}"), ITEMS.to_string())
            }
        };
        let mut fetches = session.fetch(range, query).await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            let Some(header) = fetch.header() else {
                continue;
            };
            let Some(id) = message_id_of(header) else {
                continue;
            };
            out.push((
                id,
                fetch
                    .gmail_labels()
                    .map(|ls| ls.iter().map(|l| l.to_string()).collect())
                    .unwrap_or_default(),
            ));
        }
    }
    session.logout().await?;
    Ok(LabelSweep {
        labels: out,
        modseq,
    })
}

/// Pulls the Message-ID value out of a one-field header block.
fn message_id_of(header: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(header);
    let line = text
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("message-id:"))?;
    let value = line.split_once(':')?.1.trim();
    // Stored without the angle brackets, as ingest stores it.
    Some(
        value
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string(),
    )
}

/// Gmail's own labels for the newest `limit` messages in a folder.
///
/// `X-GM-LABELS` is the only way to know where Gmail actually keeps a message.
/// Over plain IMAP a message is only ever "in" the mailbox you fetched it from,
/// so archived — which on Gmail means *not carrying the Inbox label* — is not
/// something the protocol can express. This is Gmail telling us directly.
pub async fn fetch_gmail_labels(
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
) -> Result<Vec<Vec<String>>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            gmail_labels_session(client, cfg, folder, limit).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            gmail_labels_session(Client::new(tcp), cfg, folder, limit).await
        }
    }
}

async fn gmail_labels_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
) -> Result<Vec<Vec<String>>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.examine(folder).await?;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        let mut fetches = session
            .fetch(format!("{first}:{last}"), "(X-GM-LABELS)")
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            out.push(
                fetch
                    .gmail_labels()
                    .map(|ls| ls.iter().map(|l| l.to_string()).collect())
                    .unwrap_or_default(),
            );
        }
    }
    session.logout().await?;
    Ok(out)
}

/// Just the flags of the newest `limit` messages in a folder.
///
/// For answering "is this message starred on the server" without pulling any
/// bodies: a diagnostic, and cheap enough to run against a real mailbox.
pub async fn fetch_flags_only(cfg: &ImapConfig, folder: &str, limit: u32) -> Result<Vec<i64>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            flags_only_session(client, cfg, folder, limit).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            flags_only_session(Client::new(tcp), cfg, folder, limit).await
        }
    }
}

async fn flags_only_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
) -> Result<Vec<i64>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.examine(folder).await?;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        let mut fetches = session.fetch(format!("{first}:{last}"), "(FLAGS)").await?;
        while let Some(fetch) = fetches.next().await {
            out.push(flags_to_bits(fetch?.flags()));
        }
    }
    session.logout().await?;
    Ok(out)
}

/// How many messages each folder holds.
///
/// EXAMINE rather than SELECT: read-only, so counting cannot mark anything seen
/// or otherwise disturb a mailbox we are only measuring.
pub async fn folder_counts(cfg: &ImapConfig, folders: &[String]) -> Result<Vec<(String, u32)>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            folder_counts_session(client, cfg, folders).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            folder_counts_session(Client::new(tcp), cfg, folders).await
        }
    }
}

async fn folder_counts_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folders: &[String],
) -> Result<Vec<(String, u32)>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mut out = Vec::new();
    for name in folders {
        // A folder that cannot be examined is reported as absent rather than
        // failing the whole survey — \Noselect containers are normal.
        match session.examine(name).await {
            Ok(mb) => out.push((name.clone(), mb.exists)),
            Err(_) => continue,
        }
    }
    session.logout().await?;
    Ok(out)
}

/// Waits on the server for something to happen, or until `timeout`.
///
/// This is IMAP's push (RFC 2177): the connection is held open and the server
/// speaks first when mail arrives, so delivery is immediate instead of however
/// long is left on a poll interval.
///
/// Returns true when the server reported activity, false on a clean timeout.
/// The caller treats both the same way — go and look — because IDLE says only
/// that *something* changed, never what: a new message, a flag set from
/// another client, an expunge. Deciding what happened is the resync's job.
///
/// The 29-minute ceiling in RFC 2177 is why this returns rather than looping
/// internally. A connection held longer is one many servers quietly drop, and a
/// dropped IDLE fails in the worst way — it simply stops delivering, with no
/// error to notice. Coming back up for air on a timer makes that failure a
/// reconnect rather than a silence.
pub async fn idle_once(cfg: &ImapConfig, folder: &str, timeout: Duration) -> Result<bool> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            idle_session(client, cfg, folder, timeout).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            idle_session(Client::new(tcp), cfg, folder, timeout).await
        }
    }
}

/// Holds one connection open and reports every time the server speaks.
///
/// The difference from `idle_once` is what happens *after* a wake. `idle_once`
/// logs out, so its caller opens a fresh connection for the next watch and a
/// mailbox that speaks often costs a TLS handshake and a LOGIN per message.
/// On a live account that showed up as thirteen `Can't assign requested
/// address` failures in a week — the local ephemeral ports were being spent
/// on reconnects. Here the session is kept: DONE, hand the wake over, IDLE
/// again, all on the same socket.
///
/// Returns `Ok(())` when `ceiling` is reached and the connection should be
/// rebuilt. The ceiling is measured from the login, not reset per wake,
/// because RFC 2177's limit is on how long a *connection* may sit in IDLE —
/// a busy mailbox must still come up for air on the same schedule as a quiet
/// one, or the reconnect that keeps the socket alive never happens.
///
/// `on_wake` is called once per report and must not block: it runs between
/// DONE and the next IDLE, which is exactly the window this exists to keep
/// short. Hand the work to somebody else and return.
pub async fn idle_watch<F>(
    cfg: &ImapConfig,
    folder: &str,
    ceiling: Duration,
    on_wake: F,
) -> Result<()>
where
    F: FnMut(),
{
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            idle_watch_session(client, cfg, folder, ceiling, on_wake).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            idle_watch_session(Client::new(tcp), cfg, folder, ceiling, on_wake).await
        }
    }
}

/// The SASL exchange for XOAUTH2.
///
/// One initial response and nothing after it: the server either accepts, or
/// sends a base64 error blob and expects an empty line before it will report
/// the failure. Answering that with more credentials would hang the exchange,
/// so every challenge after the first gets nothing.
struct XOauth2 {
    initial: String,
    sent: bool,
}

impl async_imap::Authenticator for XOauth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        if self.sent {
            // The error path. The server has already refused; this empty line
            // is what lets it say so rather than waiting for us.
            return String::new();
        }
        self.sent = true;
        self.initial.clone()
    }
}

/// The credential string XOAUTH2 carries, before base64.
///
/// `user=<address>^Aauth=Bearer <token>^A^A`, where ^A is a single 0x01 byte.
/// Written out here rather than inline because the shape is exact — the two
/// trailing separators are not a typo, and a version with one of them is
/// refused by every server that implements this.
pub fn xoauth2_payload(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// Signs in, by whichever means the account carries.
///
/// One function because there were twenty-nine copies of `.login(user, pass)`
/// scattered through this file, and adding a second way to authenticate to
/// twenty-nine places is how one of them gets missed — the one that then fails
/// only for accounts using the new way, on whichever code path nobody tried.
async fn sign_in<S>(client: Client<S>, cfg: &ImapConfig) -> Result<Session<S>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    match &cfg.credential {
        Credential::Password(pass) => client
            .login(&cfg.user, pass)
            .await
            .map_err(|(e, _)| e)
            .map_err(Into::into),
        Credential::Bearer(token) => client
            .authenticate(
                "XOAUTH2",
                XOauth2 {
                    initial: xoauth2_payload(&cfg.user, token),
                    sent: false,
                },
            )
            .await
            .map_err(|(e, _)| e)
            .map_err(Into::into),
    }
}

async fn idle_watch_session<S, F>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    ceiling: Duration,
    mut on_wake: F,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug + 'static,
    F: FnMut(),
{
    let started = std::time::Instant::now();
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;

    loop {
        let left = ceiling.saturating_sub(started.elapsed());
        // Zero would be an IDLE that times out the instant it is issued, which
        // is a round trip spent to learn nothing.
        if left.is_zero() {
            break;
        }
        let mut handle = session.idle();
        handle.init().await?;
        let woke = {
            // Two timeouts, for the reason given on `idle_session`: the
            // library's is reset by any response, including keepalives, so
            // only the outer wall-clock one can enforce the ceiling.
            let (idle_wait, _interrupt) = handle.wait_with_timeout(left);
            match tokio::time::timeout(left, idle_wait).await {
                Ok(r) => matches!(r?, IdleResponse::NewData(_)),
                Err(_) => false,
            }
        };
        // DONE before anything else: the connection is in IDLE until it is
        // sent, and a session in IDLE will not take another command.
        session = handle.done().await?;
        if woke {
            on_wake();
        }
    }
    session.logout().await?;
    Ok(())
}

async fn idle_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    timeout: Duration,
) -> Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug + 'static,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;

    let mut handle = session.idle();
    handle.init().await?;
    let woke = {
        // Two timeouts, because the library's is an *inactivity* timeout: it is
        // reset by any response, including the "* OK Still here" keepalives
        // Gmail sends. Left to itself a 20-minute idle can outlive RFC 2177's
        // 29-minute ceiling indefinitely, which is precisely the case that ends
        // in a silently dropped connection. The outer one is wall-clock and
        // cannot be reset by anything the server says.
        let (idle_wait, _interrupt) = handle.wait_with_timeout(timeout);
        match tokio::time::timeout(timeout, idle_wait).await {
            Ok(r) => matches!(r?, IdleResponse::NewData(_)),
            Err(_) => false,
        }
    };
    // DONE before logout: leaving a connection in IDLE and dropping it makes
    // the server hold resources until its own timeout expires.
    let mut session = handle.done().await?;
    session.logout().await?;
    Ok(woke)
}

/// Fetches everything above `since_uid`, streaming each message as it arrives.
///
/// Addressed by UID rather than sequence number, because sequence numbers shift
/// as mail arrives and is expunged — an offset that was correct when the poll
/// started can name a different message by the time it runs.
///
/// A `{n}:*` range always returns at least one message even when nothing is
/// new (the server clamps it to the last one), so anything at or below
/// `since_uid` is dropped here rather than re-ingested every poll.
/// One folder's slice of a sync cycle.
pub struct FolderPass {
    pub path: String,
    /// Fetch only above this UID. Zero means the folder has never synced,
    /// which fetches the newest `seed_window` by sequence instead of
    /// everything since the beginning of the account.
    pub since_uid: u32,
    pub expected_validity: Option<u32>,
    /// The UIDNEXT the store last saw. This, not the watermark, is the "any
    /// new mail?" test: UIDNEXT only moves when a message arrives, while a
    /// folder whose last messages were deleted keeps a UIDNEXT permanently
    /// above its highest surviving UID — comparing against the watermark
    /// would call that folder changed on every cycle forever.
    pub since_uidnext: Option<u32>,
    /// The HIGHESTMODSEQ the store last saw, for CONDSTORE flag diffs.
    pub since_modseq: Option<u64>,
    pub seed_window: u32,
}

/// What one folder's slice found.
#[derive(Debug)]
pub enum PassOutcome {
    /// STATUS said nothing moved: no select, no fetch, one line on the wire.
    Unchanged {
        uid_validity: Option<u32>,
        /// Reported so a folder with no baseline can adopt one while quiet.
        highest_modseq: Option<u64>,
        uid_next: Option<u32>,
        total: u32,
    },
    Fetched {
        fetched: usize,
        uid_validity: Option<u32>,
        highest_modseq: Option<u64>,
        uid_next: Option<u32>,
        /// Flags that changed on mail we already hold, by UID — read on the
        /// phone, starred on the web. Empty unless the server has CONDSTORE.
        flag_updates: Vec<(u32, i64)>,
        /// The custom keywords those same messages now wear, by UID —
        /// tags applied in another client. Empty unless the caller asked
        /// (`want_keywords`) and the flag diff ran.
        keyword_updates: Vec<(u32, Vec<String>)>,
        total: u32,
    },
    /// The folder was renumbered; nothing fetched. See `FetchOutcome`.
    ValidityChanged { now: Option<u32> },
    /// This folder failed; the others in the pass continue.
    Failed { detail: String },
}

/// Syncs every folder over **one** connection.
///
/// The per-folder cost used to be a TLS handshake, a LOGIN, a SELECT and a
/// full fetch — over a hundred handshakes a cycle on a mailbox with a deep
/// folder tree, almost all of it to learn that nothing had happened. Now the
/// cycle logs in once and asks one STATUS line per folder; only folders whose
/// UIDNEXT or HIGHESTMODSEQ actually moved get selected and fetched. Bodies
/// are fetched with BODY.PEEK, which is also a correctness fix: RFC822 marks
/// mail \Seen on the server as a side effect, so merely syncing was quietly
/// reading your mail for you everywhere else.
pub async fn sync_pass<F>(
    cfg: &ImapConfig,
    passes: &[FolderPass],
    // Whether to read custom keywords alongside the flag diff. Gmail says
    // no: there, custom flags are labels, and the label sweep owns them.
    want_keywords: bool,
    on_message: F,
) -> Result<Vec<PassOutcome>>
where
    F: FnMut(usize, u32, i64, &[u8]),
{
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            sync_pass_session(client, cfg, passes, want_keywords, on_message).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            sync_pass_session(Client::new(tcp), cfg, passes, want_keywords, on_message).await
        }
    }
}

async fn sync_pass_session<S, F>(
    client: Client<S>,
    cfg: &ImapConfig,
    passes: &[FolderPass],
    want_keywords: bool,
    mut on_message: F,
) -> Result<Vec<PassOutcome>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(usize, u32, i64, &[u8]),
{
    let mut session = sign_in(client, cfg).await?;
    let condstore = session
        .capabilities()
        .await
        .map(|caps| {
            caps.iter()
                .any(|c| format!("{c:?}").to_ascii_uppercase().contains("CONDSTORE"))
        })
        .unwrap_or(false);

    let mut out = Vec::with_capacity(passes.len());
    for (index, pass) in passes.iter().enumerate() {
        let status = match session
            .status(&pass.path, "(MESSAGES UIDNEXT UIDVALIDITY HIGHESTMODSEQ)")
            .await
        {
            Ok(s) => s,
            Err(e) => {
                out.push(PassOutcome::Failed {
                    detail: e.to_string(),
                });
                continue;
            }
        };

        if let Some(expected) = pass.expected_validity
            && status.uid_validity != Some(expected)
        {
            out.push(PassOutcome::ValidityChanged {
                now: status.uid_validity,
            });
            continue;
        }

        let new_mail = if pass.since_uid == 0 {
            status.exists > 0
        } else {
            match (pass.since_uidnext, status.uid_next) {
                // The precise test: UIDNEXT moved, mail arrived.
                (Some(seen), Some(now)) => now != seen,
                // No baseline or no answer: the STATUS cannot vouch for
                // quiet, so look properly rather than assume.
                _ => status.uid_next.is_none_or(|n| pass.since_uid + 1 < n),
            }
        };
        let flags_moved = condstore
            && match (pass.since_modseq, status.highest_modseq) {
                (Some(seen), Some(now)) => now > seen,
                // No baseline yet: adopt one below without a diff.
                _ => false,
            };

        if !new_mail && !flags_moved {
            out.push(PassOutcome::Unchanged {
                uid_validity: status.uid_validity,
                highest_modseq: status.highest_modseq,
                uid_next: status.uid_next,
                total: status.exists,
            });
            continue;
        }

        // Read-only select: fetching mail must not change it.
        let mailbox = match session.examine(&pass.path).await {
            Ok(m) => m,
            Err(e) => {
                out.push(PassOutcome::Failed {
                    detail: e.to_string(),
                });
                continue;
            }
        };

        let mut fetched = 0usize;
        if new_mail {
            let query = "(UID FLAGS BODY.PEEK[])";
            if pass.since_uid == 0 {
                if mailbox.exists > 0 {
                    let first = mailbox
                        .exists
                        .saturating_sub(pass.seed_window.saturating_sub(1))
                        .max(1);
                    let range = format!("{first}:{last}", last = mailbox.exists);
                    let mut fetches = session.fetch(range, query).await?;
                    while let Some(fetch) = fetches.next().await {
                        let fetch = fetch?;
                        let bits = flags_to_bits(fetch.flags());
                        if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                            on_message(index, uid, bits, body);
                            fetched += 1;
                        }
                    }
                }
            } else {
                let range = format!("{}:*", pass.since_uid.saturating_add(1));
                let mut fetches = session.uid_fetch(range, query).await?;
                while let Some(fetch) = fetches.next().await {
                    let fetch = fetch?;
                    let bits = flags_to_bits(fetch.flags());
                    if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                        if uid <= pass.since_uid {
                            continue;
                        }
                        on_message(index, uid, bits, body);
                        fetched += 1;
                    }
                }
            }
        }

        let mut flag_updates = Vec::new();
        let mut keyword_updates: Vec<(u32, Vec<String>)> = Vec::new();
        if flags_moved && let Some(seen) = pass.since_modseq {
            // A server that advertised CONDSTORE and then refuses the fetch
            // leaves flags stale until the next full pass — the state they
            // were in before this existed.
            if let Ok(mut fetches) = session
                .uid_fetch("1:*", format!("(FLAGS) (CHANGEDSINCE {seen})"))
                .await
            {
                while let Some(fetch) = fetches.next().await {
                    let Ok(fetch) = fetch else { break };
                    if let Some(uid) = fetch.uid {
                        flag_updates.push((uid, flags_to_bits(fetch.flags())));
                        if want_keywords {
                            keyword_updates.push((uid, keywords_of(fetch.flags())));
                        }
                    }
                }
            }
        }

        out.push(PassOutcome::Fetched {
            fetched,
            uid_validity: mailbox.uid_validity.or(status.uid_validity),
            highest_modseq: mailbox.highest_modseq.or(status.highest_modseq),
            uid_next: mailbox.uid_next.or(status.uid_next),
            flag_updates,
            keyword_updates,
            total: mailbox.exists,
        });
    }
    session.logout().await?;
    Ok(out)
}

/// What a watermark fetch found when it looked at the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The folder is the one the stored UIDs belong to; fetched normally.
    Fetched {
        count: usize,
        /// The UIDVALIDITY the mailbox reported, for a first sync to adopt.
        uid_validity: Option<u32>,
    },
    /// The server renumbered the folder. Nothing was fetched: a watermark
    /// against renumbered UIDs would skip real mail and misread the rest,
    /// so the caller must re-map before any UID is trusted again.
    ValidityChanged { now: Option<u32> },
}

pub async fn fetch_since_each<F>(
    cfg: &ImapConfig,
    folder: &str,
    since_uid: u32,
    expected_validity: Option<u32>,
    on_message: F,
) -> Result<FetchOutcome>
where
    F: FnMut(u32, i64, &[u8]),
{
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            fetch_since_session(
                client,
                cfg,
                folder,
                since_uid,
                expected_validity,
                on_message,
            )
            .await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            fetch_since_session(
                Client::new(tcp),
                cfg,
                folder,
                since_uid,
                expected_validity,
                on_message,
            )
            .await
        }
    }
}

async fn fetch_since_session<S, F>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    since_uid: u32,
    expected_validity: Option<u32>,
    mut on_message: F,
) -> Result<FetchOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(u32, i64, &[u8]),
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(folder).await?;
    let uid_validity = mailbox.uid_validity;
    // Checked before a single byte is fetched: after a reset the watermark
    // is meaningless, and `{since+1}:*` in the new numbering could be
    // anything — most of the folder, or none of it.
    if let Some(expected) = expected_validity
        && uid_validity != Some(expected)
    {
        session.logout().await?;
        return Ok(FetchOutcome::ValidityChanged { now: uid_validity });
    }
    let mut n = 0usize;
    {
        let mut fetches = session
            .uid_fetch(
                format!("{}:*", since_uid.saturating_add(1)),
                "(UID FLAGS RFC822)",
            )
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            let bits = flags_to_bits(fetch.flags());
            if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                if uid <= since_uid {
                    continue;
                }
                on_message(uid, bits, body);
                n += 1;
            }
        }
    }
    session.logout().await?;
    Ok(FetchOutcome::Fetched {
        count: n,
        uid_validity,
    })
}

/// The identity of a folder's mail, without its bytes.
///
/// What recovery needs after a UIDVALIDITY reset: which Message-IDs the
/// server holds, under which new UIDs. Header-only, so listing a folder
/// costs a line per message, not a body per message.
#[derive(Debug, Clone, Default)]
pub struct IdMap {
    pub uid_validity: Option<u32>,
    /// (uid, Message-ID without brackets) — the id is absent when the
    /// message never had one.
    pub entries: Vec<(u32, Option<String>)>,
    /// Whether the listing covered the whole mailbox. A depth-limited
    /// listing proves nothing about mail older than its window, and the
    /// store must not evict on it.
    pub complete: bool,
}

/// Lists the newest `depth` messages as (uid, Message-ID) pairs.
pub async fn fetch_id_map(cfg: &ImapConfig, folder: &str, depth: u32) -> Result<IdMap> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            id_map_session(client, cfg, folder, depth).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            id_map_session(Client::new(tcp), cfg, folder, depth).await
        }
    }
}

async fn id_map_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    depth: u32,
) -> Result<IdMap>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(folder).await?;
    let mut map = IdMap {
        uid_validity: mailbox.uid_validity,
        entries: Vec::new(),
        complete: mailbox.exists <= depth,
    };
    if mailbox.exists > 0 {
        // Sequence numbers, deliberately: they are the one address that is
        // valid in any numbering, which is the whole situation here.
        let start = mailbox
            .exists
            .saturating_sub(depth.saturating_sub(1))
            .max(1);
        let mut fetches = session
            .fetch(
                format!("{start}:*"),
                "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])",
            )
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            if let Some(uid) = fetch.uid {
                map.entries
                    .push((uid, fetch.header().and_then(message_id_of)));
            }
        }
    }
    session.logout().await?;
    Ok(map)
}

/// Lists one UID range as (uid, Message-ID) pairs — the cheap half of the
/// All Mail walk: identity costs a line per message where a body costs the
/// message itself, and All Mail is mostly mail already held.
pub async fn fetch_id_map_range(
    cfg: &ImapConfig,
    folder: &str,
    first: u32,
    last: u32,
) -> Result<Vec<(u32, Option<String>)>> {
    if first > last {
        return Ok(Vec::new());
    }
    let set = format!("{first}:{last}");
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            id_range_session(client, cfg, folder, &set).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            id_range_session(Client::new(tcp), cfg, folder, &set).await
        }
    }
}

async fn id_range_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    set: &str,
) -> Result<Vec<(u32, Option<String>)>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let mut out = Vec::new();
    {
        let mut fetches = session
            .uid_fetch(set, "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])")
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            if let Some(uid) = fetch.uid {
                out.push((uid, fetch.header().and_then(message_id_of)));
            }
        }
    }
    session.logout().await?;
    Ok(out)
}

/// One folder's UIDNEXT, by STATUS — where a fresh All Mail walk starts.
pub async fn folder_uidnext(cfg: &ImapConfig, folder: &str) -> Result<Option<u32>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            let mut session = sign_in(client, cfg).await?;
            let s = session.status(folder, "(UIDNEXT)").await?;
            session.logout().await?;
            Ok(s.uid_next)
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            let client = Client::new(tcp);
            let mut session = sign_in(client, cfg).await?;
            let s = session.status(folder, "(UIDNEXT)").await?;
            session.logout().await?;
            Ok(s.uid_next)
        }
    }
}

/// Fetches one contiguous UID range in full — the backfill's stride.
///
/// A range asks for what history held; what it returns is whatever still
/// exists there, which after years of expunges can be less, or nothing.
/// Nothing is not failure: it means that stretch of numbers is spent.
pub async fn fetch_uid_range_each<F>(
    cfg: &ImapConfig,
    folder: &str,
    first: u32,
    last: u32,
    mut on_message: F,
) -> Result<usize>
where
    F: FnMut(u32, i64, &[u8]),
{
    if first > last {
        return Ok(0);
    }
    let set = format!("{first}:{last}");
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            uid_set_session(client, cfg, folder, &set, &mut on_message).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            uid_set_session(Client::new(tcp), cfg, folder, &set, &mut on_message).await
        }
    }
}

/// Fetches an explicit set of messages in full, by UID.
///
/// The second half of recovery: the UIDs the store could not re-map are
/// downloaded again and handed to ingest, whose own dedupe decides whether
/// each is truly new. Chunked so a large mend never builds one enormous
/// command line.
pub async fn fetch_uids_each<F>(
    cfg: &ImapConfig,
    folder: &str,
    uids: &[u32],
    mut on_message: F,
) -> Result<usize>
where
    F: FnMut(u32, i64, &[u8]),
{
    let mut n = 0usize;
    for chunk in uids.chunks(200) {
        let set = chunk
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        match cfg.security {
            Security::Tls => {
                let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
                n += uid_set_session(client, cfg, folder, &set, &mut on_message).await?;
            }
            #[cfg(feature = "insecure-plaintext")]
            Security::InsecurePlaintext => {
                let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
                n += uid_set_session(Client::new(tcp), cfg, folder, &set, &mut on_message).await?;
            }
        }
    }
    Ok(n)
}

async fn uid_set_session<S, F>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    set: &str,
    on_message: &mut F,
) -> Result<usize>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(u32, i64, &[u8]),
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let mut n = 0usize;
    {
        // PEEK, as everywhere: fetching mail must not mark it read.
        let mut fetches = session.uid_fetch(set, "(UID FLAGS BODY.PEEK[])").await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            let bits = flags_to_bits(fetch.flags());
            if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                on_message(uid, bits, body);
                n += 1;
            }
        }
    }
    session.logout().await?;
    Ok(n)
}

/// Applies a flag change to one message, by UID.
///
/// `add` decides between +FLAGS and -FLAGS. Silent (.SILENT) because we already
/// know what we asked for and do not want the server echoing every flag back
/// for every message in a drain.
pub async fn store_flag(
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    flag: &str,
    add: bool,
) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            store_flag_session(client, cfg, folder, uid, flag, add).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            store_flag_session(Client::new(tcp), cfg, folder, uid, flag, add).await
        }
    }
}

async fn store_flag_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    flag: &str,
    add: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let op = if add {
        "+FLAGS.SILENT"
    } else {
        "-FLAGS.SILENT"
    };
    {
        let mut updates = session
            .uid_store(uid.to_string(), format!("{op} ({flag})"))
            .await?;
        while updates.next().await.is_some() {}
    }
    session.logout().await?;
    Ok(())
}

/// Quotes a label for an IMAP command.
///
/// Labels are user-written, so they arrive with spaces, quotes and backslashes
/// in them. An unquoted one with a space would be read as two labels, and an
/// unescaped quote would end the string early and leave the rest of the name
/// being parsed as commands.
fn quote_imap(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Adds or removes a Gmail label on one message.
///
/// `X-GM-LABELS` is Gmail's own extension and the only way to write a label
/// over IMAP: a label is not a flag and not a folder, so neither `STORE FLAGS`
/// nor a copy would express it. On any other server this does not exist, which
/// is why the caller decides whether to call it.
pub async fn store_gmail_labels(
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    label: &str,
    add: bool,
) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            store_labels_session(client, cfg, folder, uid, label, add).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            store_labels_session(Client::new(tcp), cfg, folder, uid, label, add).await
        }
    }
}

async fn store_labels_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    label: &str,
    add: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let op = if add { "+X-GM-LABELS" } else { "-X-GM-LABELS" };
    {
        let mut updates = session
            .uid_store(uid.to_string(), format!("{op} ({})", quote_imap(label)))
            .await?;
        while updates.next().await.is_some() {}
    }
    session.logout().await?;
    Ok(())
}

/// Removes one message from the server for good.
///
/// UID EXPUNGE (RFC 4315) when the server has UIDPLUS, because a bare EXPUNGE
/// is not a per-message operation: it removes *every* message in the mailbox
/// carrying \Deleted, including ones some other client marked and has not
/// committed yet. Deleting one message is not a licence to commit someone
/// else's pending deletions.
///
/// Without UIDPLUS the message is marked \Deleted and left there. That is the
/// conservative half of the job — it disappears from Petrel either way, and the
/// server drops it at the next compaction — and it is a great deal better than
/// taking unrelated mail with it. The caller is told which happened.
pub async fn expunge_uid(
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    server_has_uidplus: bool,
) -> Result<bool> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            expunge_uid_session(client, cfg, folder, uid, server_has_uidplus).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            expunge_uid_session(Client::new(tcp), cfg, folder, uid, server_has_uidplus).await
        }
    }
}

async fn expunge_uid_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    uid: u32,
    server_has_uidplus: bool,
) -> Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    {
        let mut updates = session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await?;
        while updates.next().await.is_some() {}
    }
    let expunged = if server_has_uidplus {
        // Pinned because the expunge stream is not Unpin, unlike the store one.
        let updates = session.uid_expunge(uid.to_string()).await?;
        futures::pin_mut!(updates);
        while updates.next().await.is_some() {}
        true
    } else {
        false
    };
    session.logout().await?;
    Ok(expunged)
}

/// Moves one message, by UID, into another folder.
///
/// Prefers UID MOVE (RFC 6851) and falls back to COPY + \Deleted + EXPUNGE for
/// servers without it. The fallback is not equivalent — an expunge affects the
/// whole mailbox, not just this message — so the capability is checked rather
/// than assumed, and the slow path is taken only when there is no choice.
/// Creates a folder on the server. Already-exists is success: the folder the
/// user asked for is there, which is what they asked for.
pub async fn create_folder(cfg: &ImapConfig, path: &str) -> Result<()> {
    folder_op(cfg, FolderOp::Create, path, "").await
}

/// Renames a folder on the server. On IMAP a rename *is* a move: nesting a
/// folder somewhere else is the same RENAME with a different path.
pub async fn rename_folder(cfg: &ImapConfig, from: &str, to: &str) -> Result<()> {
    folder_op(cfg, FolderOp::Rename, from, to).await
}

/// Deletes a folder on the server, and whatever mail it still holds — which
/// is why the UI confirms first and the store keeps its copies regardless.
pub async fn delete_folder(cfg: &ImapConfig, path: &str) -> Result<()> {
    folder_op(cfg, FolderOp::Delete, path, "").await
}

#[derive(Clone, Copy)]
enum FolderOp {
    Create,
    Rename,
    Delete,
}

async fn folder_op(cfg: &ImapConfig, op: FolderOp, a: &str, b: &str) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            folder_op_session(client, cfg, op, a, b).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            folder_op_session(Client::new(tcp), cfg, op, a, b).await
        }
    }
}

async fn folder_op_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    op: FolderOp,
    a: &str,
    b: &str,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let result = match op {
        FolderOp::Create => session.create(a).await,
        FolderOp::Rename => session.rename(a, b).await,
        FolderOp::Delete => session.delete(a).await,
    };
    match result {
        Ok(()) => {}
        // "Already exists" answers a CREATE the way success does: the folder
        // the caller wanted is there. Everything else is a real failure.
        Err(e)
            if matches!(op, FolderOp::Create)
                && e.to_string().to_ascii_lowercase().contains("exist") => {}
        Err(e) => {
            let _ = session.logout().await;
            return Err(e.into());
        }
    }
    session.logout().await?;
    Ok(())
}

pub async fn move_uid(
    cfg: &ImapConfig,
    from: &str,
    uid: u32,
    to: &str,
    server_has_move: bool,
) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            move_uid_session(client, cfg, from, uid, to, server_has_move).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            move_uid_session(Client::new(tcp), cfg, from, uid, to, server_has_move).await
        }
    }
}

async fn move_uid_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    from: &str,
    uid: u32,
    to: &str,
    server_has_move: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(from).await?;
    if server_has_move {
        session.uid_mv(uid.to_string(), to).await?;
    } else {
        session.uid_copy(uid.to_string(), to).await?;
        {
            let mut updates = session
                .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
                .await?;
            while updates.next().await.is_some() {}
        }
        {
            // The expunge stream is not Unpin, so it has to be pinned before it
            // can be driven.
            let expunged = session.expunge().await?;
            futures::pin_mut!(expunged);
            while expunged.next().await.is_some() {}
        }
    }
    session.logout().await?;
    Ok(())
}

/// Searches a folder for a Message-ID. This is the evidence-gathering half of
/// the ambiguous-send rule: after a send whose outcome we could not read, we
/// ask the server whether it actually has the message rather than guessing.
/// Returns the matching sequence numbers (empty = provably absent).
/// Like `find_message_id`, but answers in UIDs — the currency of APPEND,
/// FETCH and EXPUNGE. `find_message_id` answers in sequence numbers, which
/// are only good for counting; recording one of those as "the copy to delete
/// later" deletes whatever message happens to be standing at that position.
pub async fn uids_for_message_id(
    cfg: &ImapConfig,
    folder: &str,
    message_id: &str,
) -> Result<Vec<u32>> {
    let query = format!("HEADER Message-ID \"{}\"", message_id.replace('"', ""));
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            uid_search_session(client, cfg, folder, &query).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            uid_search_session(Client::new(tcp), cfg, folder, &query).await
        }
    }
}

async fn uid_search_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    query: &str,
) -> Result<Vec<u32>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let hits = session.uid_search(query).await?;
    let mut found: Vec<u32> = hits.into_iter().collect();
    found.sort_unstable();
    session.logout().await?;
    Ok(found)
}

/// Every UID the folder currently holds — the ground truth a placement
/// sweep compares against. One SEARCH ALL; the response is a number list,
/// cheap even for a mailbox in the tens of thousands.
pub async fn uids_in_folder(cfg: &ImapConfig, folder: &str) -> Result<Vec<u32>> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            uid_search_session(client, cfg, folder, "ALL").await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            uid_search_session(Client::new(tcp), cfg, folder, "ALL").await
        }
    }
}

pub async fn find_message_id(cfg: &ImapConfig, folder: &str, message_id: &str) -> Result<Vec<u32>> {
    // Message-ID values are generated by us; quote defensively regardless.
    let query = format!("HEADER Message-ID \"{}\"", message_id.replace('"', ""));
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            search_session(client, cfg, folder, &query).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            search_session(Client::new(tcp), cfg, folder, &query).await
        }
    }
}

async fn search_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    query: &str,
) -> Result<Vec<u32>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(folder).await?;
    let hits = session.search(query).await?;
    let mut found: Vec<u32> = hits.into_iter().collect();
    found.sort_unstable();
    session.logout().await?;
    Ok(found)
}

/// Connects, authenticates, and reports what the server supports and holds.
/// Connects and signs in, and does nothing else.
///
/// The onboarding connection test. A full probe lists folders and fetches
/// mail; this answers the only question the form is asking — are the host,
/// the port and the password right — and answers it in one round trip, so a
/// wrong password is reported before anything has been stored.
pub async fn login_check(cfg: &ImapConfig) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            let session = sign_in(client, cfg).await?;
            let mut session = session;
            session.logout().await?;
            Ok(())
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            let mut session = sign_in(Client::new(tcp), cfg).await?;
            session.logout().await?;
            Ok(())
        }
    }
}

pub async fn probe(cfg: &ImapConfig, fetch_limit: u32) -> Result<ProbeReport> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            probe_session(client, cfg, fetch_limit).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            probe_session(Client::new(tcp), cfg, fetch_limit).await
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn labels_are_quoted_so_a_name_cannot_become_a_command() {
        // Labels are written by the user, so they arrive with spaces, quotes
        // and backslashes in them. Unquoted, "Work stuff" is two labels; with
        // an unescaped quote, the rest of the name is parsed as IMAP.
        assert_eq!(super::quote_imap("Urgent"), "\"Urgent\"");
        assert_eq!(super::quote_imap("Work stuff"), "\"Work stuff\"");
        assert_eq!(super::quote_imap(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(super::quote_imap(r"back\slash"), r#""back\\slash""#);
    }
    use super::{capability_token, parse_capabilities};

    #[test]
    fn capability_tokens_normalise() {
        assert_eq!(capability_token("Atom(\"IDLE\")"), "IDLE");
        assert_eq!(capability_token("Auth(\"XOAUTH2\")"), "AUTH=XOAUTH2");
        assert_eq!(capability_token("Imap4rev1"), "IMAP4REV1");
    }

    #[test]
    fn strategy_follows_capabilities() {
        use super::SyncStrategy::*;
        let of = |caps: &[&str]| parse_capabilities(caps.iter().map(|s| s.to_string())).strategy();
        assert_eq!(of(&["IDLE", "QRESYNC", "CONDSTORE"]), Qresync);
        assert_eq!(of(&["IDLE", "CONDSTORE"]), Condstore);
        // Gmail-over-IMAP and Microsoft 365 both land here — the common case.
        assert_eq!(of(&["IDLE", "UIDPLUS", "MOVE"]), FullReconcile);
    }
}

/// The role a server claims for a folder, from its LIST attributes (RFC 6154
/// SPECIAL-USE), falling back to the one name every server agrees on.
///
/// Attributes arrive as the Debug rendering of the IMAP crate's name-attribute
/// type, so `\\Sent` may appear as `Extension("\\Sent")` or bare. Matching is
/// therefore on a normalised token rather than an exact string: being strict
/// here fails silently and leaves every folder unmapped, which reads as "this
/// server has no Sent folder" rather than as a parsing bug.
///
/// Gmail is the reason `\All` maps to archive. It has no `\Archive` at all —
/// archiving there means removing the Inbox label, and everything lives in All
/// Mail. Treating All Mail as the archive destination is what makes
/// "archive" mean the same thing to a user on Gmail as on Fastmail.
pub fn special_use_role(folder: &FolderInfo) -> Option<&'static str> {
    // INBOX is the one folder name the standard fixes, and it carries no
    // SPECIAL-USE attribute of its own.
    if folder.name.eq_ignore_ascii_case("INBOX") {
        return Some("inbox");
    }
    let tokens: Vec<String> = folder
        .attributes
        .iter()
        .map(|a| normalise_attr(a))
        .collect();
    let has = |t: &str| tokens.iter().any(|x| x == t);

    if has("sent") {
        return Some("sent");
    }
    if has("drafts") {
        return Some("drafts");
    }
    if has("junk") || has("spam") {
        return Some("spam");
    }
    if has("trash") {
        return Some("trash");
    }
    if has("archive") || has("all") {
        return Some("archive");
    }
    // RFC 6154's \Flagged mailbox — Gmail's [Gmail]/Starred.
    //
    // Worth syncing as a folder even though starred is a flag we already read,
    // because we only read the flags of messages we fetch. A star on older mail,
    // or on anything archived, is invisible otherwise: the server knows, and the
    // Starred view sits empty. It is also small by nature — a list of things
    // someone picked out by hand.
    if has("flagged") || has("starred") {
        return Some("starred");
    }
    // Gmail's auto-classifier category (RFC 8457 \Important). A role so it is
    // *recognised*, not so it is shown: it is not a place the user filed
    // anything, its contents are mostly the inbox again, and Petrel's notion
    // of priority is deliberately not a classifier's. Mapping it keeps it out
    // of the user-folder list and out of sync.
    if has("important") {
        return Some("important");
    }
    None
}

/// Whether the folder can hold mail at all.
///
/// `\Noselect` marks pure hierarchy — Gmail's bare `[Gmail]` container is
/// the canonical case. It answers LIST but refuses SELECT, so treating it as
/// a folder produces a rail entry that cannot open and a sync line that
/// fails on every pass.
pub fn selectable(folder: &FolderInfo) -> bool {
    !folder
        .attributes
        .iter()
        .any(|a| normalise_attr(a) == "noselect")
}

/// Lowercase alphanumerics only, so `Extension("\\Sent")`, `\Sent` and `Sent`
/// all reduce to the same token.
fn normalise_attr(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
        // The Debug wrapper contributes its own letters; drop the known ones so
        // `extensionsent` still reads as `sent`.
        .replace("extension", "")
        .replace("custom", "")
}

#[cfg(test)]
mod special_use_tests {
    use super::*;

    fn f(name: &str, attrs: &[&str]) -> FolderInfo {
        FolderInfo {
            name: name.into(),
            delimiter: Some("/".into()),
            attributes: attrs.iter().map(|a| a.to_string()).collect(),
        }
    }

    #[test]
    fn inbox_is_matched_by_name_since_it_carries_no_attribute() {
        assert_eq!(special_use_role(&f("INBOX", &[])), Some("inbox"));
        assert_eq!(special_use_role(&f("inbox", &[])), Some("inbox"));
    }

    #[test]
    fn attributes_match_however_the_imap_crate_renders_them() {
        for rendering in ["\\Sent", "Sent", "Extension(\"\\\\Sent\")"] {
            assert_eq!(
                special_use_role(&f("Sent Items", &[rendering])),
                Some("sent"),
                "failed on {rendering}"
            );
        }
    }

    #[test]
    fn gmail_all_mail_is_the_archive() {
        // Gmail ships \All and no \Archive; archiving there is "remove the
        // Inbox label", and All Mail is where the message still lives.
        assert_eq!(
            special_use_role(&f("[Gmail]/All Mail", &["\\All", "\\HasNoChildren"])),
            Some("archive")
        );
        assert_eq!(
            special_use_role(&f("[Gmail]/Sent Mail", &["\\Sent"])),
            Some("sent")
        );
        assert_eq!(
            special_use_role(&f("[Gmail]/Spam", &["\\Junk"])),
            Some("spam")
        );
        assert_eq!(
            special_use_role(&f("[Gmail]/Trash", &["\\Trash"])),
            Some("trash")
        );
    }

    #[test]
    fn a_users_own_folder_has_no_role() {
        assert_eq!(
            special_use_role(&f("Contracts", &["\\HasNoChildren"])),
            None
        );
        assert_eq!(special_use_role(&f("Contracts/2026", &[])), None);
    }

    #[test]
    fn junk_and_spam_are_the_same_role() {
        assert_eq!(special_use_role(&f("Junk", &["\\Junk"])), Some("spam"));
        assert_eq!(special_use_role(&f("Spam", &["\\Spam"])), Some("spam"));
    }

    #[test]
    fn gmails_classifier_category_is_recognised_not_shown() {
        // \Important gets a role so it is never mistaken for a folder the
        // user made — which put it in the folder list and marched the sync
        // counter past the size of the mailbox itself.
        assert_eq!(
            special_use_role(&f("[Gmail]/Important", &["\\Important"])),
            Some("important")
        );
    }

    #[test]
    fn noselect_containers_are_not_mailboxes() {
        assert!(!selectable(&f("[Gmail]", &["\\Noselect", "\\HasChildren"])));
        assert!(selectable(&f("INBOX", &["\\HasNoChildren"])));
        assert!(selectable(&f("Contracts", &[])));
    }
}

/// Marks every message in a folder, in one command.
///
/// `UID STORE 1:*` rather than a loop. The alternative is one round trip per
/// message, and a real account has ten thousand in a single folder: at even a
/// tenth of a second each that is twenty minutes of hammering somebody's
/// server to set a flag the protocol will set in one line.
///
/// Returns how many the server said were there, from SELECT's EXISTS, so the
/// caller can say "4,187 marked" rather than "done".
pub async fn store_flag_all(cfg: &ImapConfig, folder: &str, flag: &str, add: bool) -> Result<u32> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            store_flag_all_session(client, cfg, folder, flag, add).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            store_flag_all_session(Client::new(tcp), cfg, folder, flag, add).await
        }
    }
}

async fn store_flag_all_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    flag: &str,
    add: bool,
) -> Result<u32>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(folder).await?;
    let n = mailbox.exists;
    // Nothing to do, and `1:*` on an empty mailbox is a command some servers
    // answer with an error rather than a shrug.
    if n == 0 {
        session.logout().await?;
        return Ok(0);
    }
    let op = if add {
        "+FLAGS.SILENT"
    } else {
        "-FLAGS.SILENT"
    };
    {
        let mut updates = session.uid_store("1:*", format!("{op} ({flag})")).await?;
        while updates.next().await.is_some() {}
    }
    session.logout().await?;
    Ok(n)
}

/// Moves every message in a folder into another, in one command.
///
/// The bulk twin of `move_uid`, and the same two paths: UID MOVE where the
/// server has it, COPY + \Deleted + EXPUNGE where it does not. The fallback
/// expunges the whole mailbox, which here is exactly what was asked for —
/// unlike the single-message case, where it is the reason MOVE is preferred.
pub async fn move_all(
    cfg: &ImapConfig,
    from: &str,
    to: &str,
    server_has_move: bool,
) -> Result<u32> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            move_all_session(client, cfg, from, to, server_has_move).await
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            move_all_session(Client::new(tcp), cfg, from, to, server_has_move).await
        }
    }
}

async fn move_all_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    from: &str,
    to: &str,
    server_has_move: bool,
) -> Result<u32>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(from).await?;
    let n = mailbox.exists;
    if n == 0 {
        session.logout().await?;
        return Ok(0);
    }
    if server_has_move {
        session.uid_mv("1:*", to).await?;
    } else {
        session.uid_copy("1:*", to).await?;
        {
            let mut updates = session
                .uid_store("1:*", "+FLAGS.SILENT (\\Deleted)")
                .await?;
            while updates.next().await.is_some() {}
        }
        {
            let expunged = session.expunge().await?;
            futures::pin_mut!(expunged);
            while expunged.next().await.is_some() {}
        }
    }
    session.logout().await?;
    Ok(n)
}
