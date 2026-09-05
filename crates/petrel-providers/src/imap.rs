//! IMAP backend — M0 slice.
//!
//! Establishes a session, reads the server's capabilities, and derives the sync
//! strategy from them: QRESYNC → CONDSTORE → full reconcile. The ladder is the
//! whole point; Gmail-over-IMAP has no QRESYNC and Microsoft 365 has no
//! CONDSTORE, so the bottom rung is the common case, not an edge case.

use std::sync::Arc;
use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use async_imap::imap_proto::{AttributeValue, MessageSection, Response, SectionPath, Status};
use async_imap::types::Flag;
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

/// The rustls setup every connection out of this crate uses.
fn tls_config() -> Arc<ClientConfig> {
    // rustls needs one crypto provider chosen for the process. The desktop
    // chooses at startup; anything else that opens a connection through this
    // crate (a test, a tool) gets the same choice here rather than a panic.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    // TODO(M4): swap for the OS trust store so corporate/self-signed CAs work,
    // alongside the explicit per-host pinning flow for self-hosters.
    Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

async fn tls_stream(host: &str, port: u16) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = connect_tcp(host, port).await?;
    tls_upgrade(host, tcp).await
}

/// Opens the socket a connection runs on.
///
/// Two things the bare connect did not do. A deadline, because a black-holed
/// address otherwise waits out the operating system's own two minutes with the
/// account showing nothing at all; and TCP keepalive, because a NAT mapping
/// that expires while the lid is shut leaves a socket that is never closed and
/// never answers — the failure with no symptom.
async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    let limit = connect_timeout();
    let tcp = match tokio::time::timeout(limit, TcpStream::connect((host, port))).await {
        Ok(result) => result?,
        Err(_) => {
            return Err(ImapError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer from {host}:{port} after {}s", limit.as_secs()),
            )));
        }
    };
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(20));
    // Best effort: a platform that refuses the option is not a reason to
    // refuse the connection.
    let _ = socket2::SockRef::from(&tcp).set_tcp_keepalive(&keepalive);
    Ok(tcp)
}

/// Wraps an already-connected socket in TLS.
///
/// The whole of an implicit-TLS connect, and the second half of STARTTLS on
/// the submission port. One function for both, because an upgrade that
/// validated certificates less strictly than the implicit path would be a
/// downgrade with extra steps.
pub(crate) async fn tls_upgrade(
    host: &str,
    tcp: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let connector = TlsConnector::from(tls_config());
    let name = ServerName::try_from(host.to_string()).map_err(|e| ImapError::Tls(e.to_string()))?;
    connector
        .connect(name, tcp)
        .await
        .map_err(|e| ImapError::Tls(e.to_string()))
}

/// How long a connect may take before the host is called unreachable.
fn connect_timeout() -> Duration {
    crate::smtp::phase_timeout("PETREL_IMAP_CONNECT_SECONDS", 30)
}

/// How long a session may sit with the server saying nothing at all.
///
/// Generous, because a large FETCH on a slow mailbox really does go quiet
/// between messages. What it measures is a socket that will never speak
/// again, not one that is merely slow.
fn read_timeout() -> Duration {
    crate::smtp::phase_timeout("PETREL_IMAP_READ_SECONDS", 120)
}

/// The same, for a connection parked in IDLE, where silence is the point.
///
/// The wall-clock ceiling in `idle_watch` is what normally ends an IDLE; this
/// is the backstop for a connection that is wedged rather than quiet, so it
/// sits above any ceiling a caller would sensibly ask for.
fn idle_read_timeout() -> Duration {
    crate::smtp::phase_timeout("PETREL_IMAP_IDLE_READ_SECONDS", 1800)
}

/// How long `done()` and `logout()` may take. Both used to be untimed, and
/// both wait for the server to answer: a dead socket parked the account in
/// either of them for as long as the process lived.
fn command_timeout() -> Duration {
    crate::smtp::phase_timeout("PETREL_IMAP_COMMAND_SECONDS", 60)
}

/// The socket a session runs on: TLS in every shipping build and — in test
/// builds only — a plaintext loopback socket. One type, so every session in
/// this file is built the same way and carries the same deadline.
#[derive(Debug)]
enum Socket {
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    #[cfg(feature = "insecure-plaintext")]
    Plain(TcpStream),
}

impl AsyncRead for Socket {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Socket::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
            #[cfg(feature = "insecure-plaintext")]
            Socket::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Socket {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Socket::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
            #[cfg(feature = "insecure-plaintext")]
            Socket::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Socket::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
            #[cfg(feature = "insecure-plaintext")]
            Socket::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Socket::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
            #[cfg(feature = "insecure-plaintext")]
            Socket::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// A transport that gives up on a server which stops speaking.
///
/// No IMAP conversation had a deadline of any kind: a socket killed by a
/// closed lid or an expired NAT mapping left a read pending for as long as the
/// process lived, and the account it belonged to simply stopped syncing — no
/// error, no retry, nothing to notice. Here a read that makes no progress for
/// `idle` fails as a timeout, which is a failure the sync loop already knows
/// what to do with.
#[derive(Debug)]
struct Deadline<S> {
    inner: S,
    idle: Duration,
    /// Armed the first time a read cannot finish, disarmed by any progress.
    timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}

impl<S> Deadline<S> {
    fn new(inner: S, idle: Duration) -> Self {
        Deadline {
            inner,
            idle,
            timer: None,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Deadline<S> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let this = self.get_mut();
        match std::pin::Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(r) => {
                this.timer = None;
                Poll::Ready(r)
            }
            Poll::Pending => {
                let idle = this.idle;
                let timer = this
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
                match std::future::Future::poll(timer.as_mut(), cx) {
                    Poll::Ready(()) => {
                        this.timer = None;
                        Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("the server said nothing for {}s", idle.as_secs()),
                        )))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Deadline<S> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// What every session in this file runs on.
type Transport = Deadline<Socket>;

/// Opens a connection for an ordinary session.
async fn connect(cfg: &ImapConfig) -> Result<Transport> {
    connect_within(cfg, read_timeout()).await
}

/// Opens a connection for a watch, where a long silence is expected.
async fn connect_idle(cfg: &ImapConfig) -> Result<Transport> {
    connect_within(cfg, idle_read_timeout()).await
}

async fn connect_within(cfg: &ImapConfig, idle: Duration) -> Result<Transport> {
    let socket = match cfg.security {
        Security::Tls => Socket::Tls(Box::new(tls_stream(&cfg.host, cfg.port).await?)),
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => Socket::Plain(connect_tcp(&cfg.host, cfg.port).await?),
    };
    Ok(Deadline::new(socket, idle))
}

/// A mailbox name as it goes onto the wire: modified UTF-7 (RFC 3501 §5.1.3).
///
/// IMAP mailbox names are ASCII. Anything else is carried in a shift sequence,
/// so a German Drafts folder is `Entw&APw-rfe` on the wire, and a CREATE
/// carrying raw UTF-8 is refused by every server that has not been told to
/// accept it. Sending the name as typed meant non-English folders could not be
/// made, renamed, or selected.
fn wire_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut shifted: Vec<u16> = Vec::new();
    for c in name.chars() {
        // Printable ASCII travels as itself; `&` is the shift character, so it
        // says so by doubling.
        if c == '&' {
            end_shift(&mut shifted, &mut out);
            out.push_str("&-");
        } else if matches!(c, ' '..='~') {
            end_shift(&mut shifted, &mut out);
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            shifted.extend_from_slice(c.encode_utf16(&mut units));
        }
    }
    end_shift(&mut shifted, &mut out);
    out
}

/// Closes a run of non-ASCII characters as one `&…-` sequence.
fn end_shift(shifted: &mut Vec<u16>, out: &mut String) {
    use base64::Engine as _;
    if shifted.is_empty() {
        return;
    }
    let mut bytes = Vec::with_capacity(shifted.len() * 2);
    for unit in shifted.drain(..) {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&bytes);
    out.push('&');
    // Modified base64: `,` stands in for `/`, since `/` is a hierarchy
    // delimiter on many servers.
    out.push_str(&encoded.replace('/', ","));
    out.push('-');
}

/// A mailbox name as a person should see it.
///
/// The reverse of `wire_name`, plus the quoted-string escapes the response
/// parser hands back as they were written: a folder called `a "b"` arrives as
/// `a \"b\"`, and asking for it under that name asks for a mailbox no server
/// has.
fn display_name(raw: &str) -> String {
    from_wire_name(&unescape_quoted(raw))
}

fn unescape_quoted(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        // Only the two sequences a quoted string may contain. A backslash in
        // front of anything else is part of the name — a literal carries them
        // unescaped, and there is nothing in the parsed response to say which
        // form the server used.
        if c == '\\'
            && let Some(escaped) = chars.next_if(|n| matches!(n, '"' | '\\'))
        {
            out.push(escaped);
            continue;
        }
        out.push(c);
    }
    out
}

fn from_wire_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after.find('-') {
            // `&-` is how the wire spells a literal ampersand.
            Some(0) => {
                out.push('&');
                rest = &after[1..];
            }
            Some(end) => {
                match decode_shift(&after[..end]) {
                    Some(text) => out.push_str(&text),
                    // Not a shift sequence after all. Passed through exactly
                    // as it came: a name we cannot read is still the name the
                    // server will answer to.
                    None => {
                        out.push('&');
                        out.push_str(&after[..end]);
                        out.push('-');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// One `&…-` payload: modified base64 of UTF-16BE.
fn decode_shift(payload: &str) -> Option<String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(payload.replace(',', "/"))
        .ok()?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    char::decode_utf16(units)
        .collect::<std::result::Result<String, _>>()
        .ok()
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
                name: display_name(name.name()),
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
        //
        // A failure here is reported, not propagated. These headers are a
        // sample the setup screen shows to say "this is your mail"; the probe
        // itself is about capabilities, and a server that answers LIST and
        // SELECT but not this is still an account worth adding. Silence was
        // the wrong half of that though — a probe that came back with nothing
        // read exactly like a mailbox that was simply empty.
        match session
            .fetch(
                range,
                "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (DATE FROM SUBJECT)])",
            )
            .await
        {
            Err(e) => eprintln!("[imap] probe headers unavailable: {e}"),
            Ok(mut fetches) => {
                while let Some(fetch) = fetches.next().await {
                    let fetch = match fetch {
                        Ok(f) => f,
                        Err(e) => {
                            eprintln!("[imap] probe header skipped: {e}");
                            continue;
                        }
                    };
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
    }

    sign_out(&mut session).await?;

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
    let client = Client::new(connect(cfg).await?);
    let mut session = sign_in(client, cfg).await?;
    session.append(wire_name(folder), flags, None, raw).await?;
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    fetch_each_session(client, cfg, folder, limit, on_message).await
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
    let mailbox = session.select(wire_name(folder)).await?;
    let mut n = 0usize;
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        // FLAGS, not just the body. Without them every message ingests with no
        // read state and shows as unread — a mailbox with nothing unread in it
        // arrives looking like hundreds of unread conversations.
        fetch_command(
            &mut session,
            format!("FETCH {first}:{last} (UID FLAGS RFC822)"),
            |attrs| {
                if let (Some(uid), Some(body)) = (attr_uid(attrs), attr_body(attrs)) {
                    on_message(uid, flags_to_bits(attr_flags(attrs)), body);
                    n += 1;
                }
            },
        )
        .await?;
    }
    let uid_validity = mailbox.uid_validity;
    sign_out(&mut session).await?;
    Ok((n, uid_validity))
}

pub async fn fetch_raw(cfg: &ImapConfig, folder: &str, limit: u32) -> Result<Vec<(u32, Vec<u8>)>> {
    let client = Client::new(connect(cfg).await?);
    fetch_raw_session(client, cfg, folder, limit).await
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
    let mailbox = session.select(wire_name(folder)).await?;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        fetch_command(
            &mut session,
            format!("FETCH {first}:{last} (UID RFC822)"),
            |attrs| {
                if let (Some(uid), Some(body)) = (attr_uid(attrs), attr_body(attrs)) {
                    out.push((uid, body.to_vec()));
                }
            },
        )
        .await?;
    }
    sign_out(&mut session).await?;
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
    let stream = connect(cfg).await?;
    raw_thrid_exchange(stream, cfg, folder, limit, since).await
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
        &format!("EXAMINE {} (CONDSTORE)", quote(&wire_name(folder))),
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
    let client = Client::new(connect(cfg).await?);
    sweep_labels_session(client, cfg, folder, limit, since).await
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
    let mailbox = session.examine(wire_name(folder)).await?;
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
        // A refused sweep must not report a modseq: the caller records it, and
        // the next sweep would then start above labels this one never saw.
        fetch_command(&mut session, format!("FETCH {range} {query}"), |attrs| {
            if let Some(id) = attr_header(attrs).and_then(message_id_of) {
                out.push((id, attr_labels(attrs)));
            }
        })
        .await?;
    }
    sign_out(&mut session).await?;
    Ok(LabelSweep {
        labels: out,
        modseq,
    })
}

/// Pulls the Message-ID value out of a one-field header block.
fn message_id_of(header: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(header);
    // Unfolded first. Exchange folds a long id onto the continuation line,
    // and read line by line the first line's value was empty: every such
    // message failed to re-match after a UIDVALIDITY reset and was fetched
    // again, and Gmail's labels for it were keyed to "".
    let unfolded = unfold_header(&text);
    let line = unfolded
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

/// Joins folded header lines back onto the line they continue (RFC 5322 §2.2.3).
fn unfold_header(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !out.is_empty() && (line.starts_with(' ') || line.starts_with('\t')) {
            out.push(' ');
            out.push_str(line.trim_start());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
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
    let client = Client::new(connect(cfg).await?);
    gmail_labels_session(client, cfg, folder, limit).await
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
    let mailbox = session.examine(wire_name(folder)).await?;
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
    sign_out(&mut session).await?;
    Ok(out)
}

/// Just the flags of the newest `limit` messages in a folder.
///
/// For answering "is this message starred on the server" without pulling any
/// bodies: a diagnostic, and cheap enough to run against a real mailbox.
pub async fn fetch_flags_only(cfg: &ImapConfig, folder: &str, limit: u32) -> Result<Vec<i64>> {
    let client = Client::new(connect(cfg).await?);
    flags_only_session(client, cfg, folder, limit).await
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
    let mailbox = session.examine(wire_name(folder)).await?;
    let mut out = Vec::new();
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(limit.saturating_sub(1)).max(1);
        let mut fetches = session.fetch(format!("{first}:{last}"), "(FLAGS)").await?;
        while let Some(fetch) = fetches.next().await {
            out.push(flags_to_bits(fetch?.flags()));
        }
    }
    sign_out(&mut session).await?;
    Ok(out)
}

/// How many messages each folder holds.
///
/// EXAMINE rather than SELECT: read-only, so counting cannot mark anything seen
/// or otherwise disturb a mailbox we are only measuring.
pub async fn folder_counts(cfg: &ImapConfig, folders: &[String]) -> Result<Vec<(String, u32)>> {
    let client = Client::new(connect(cfg).await?);
    folder_counts_session(client, cfg, folders).await
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
        match session.examine(wire_name(name)).await {
            Ok(mb) => out.push((name.clone(), mb.exists)),
            Err(_) => continue,
        }
    }
    sign_out(&mut session).await?;
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
    let client = Client::new(connect_idle(cfg).await?);
    idle_session(client, cfg, folder, timeout).await
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
    let client = Client::new(connect_idle(cfg).await?);
    idle_watch_session(client, cfg, folder, ceiling, on_wake).await
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

/// Runs a FETCH, hands each item's attributes on, and reports how the server
/// ended it.
///
/// The typed fetch stream stops at the tagged reply without ever looking at
/// its status, so a FETCH answered `OK` and one answered
/// `NO [SERVERBUG] internal error` arrive identically: as a stream that simply
/// ends. Reading a short answer as a complete one is how a watermark moves
/// past mail that was never fetched — and mail above a watermark is never
/// asked for again. Gmail's "Some messages could not be FETCHed", Dovecot's
/// SERVERBUG and Exchange's "BAD Command Argument Error" all arrive this way.
async fn fetch_command<S, F>(
    session: &mut Session<S>,
    command: String,
    mut on_item: F,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(&[AttributeValue<'_>]),
{
    let tag = session.run_command(&command).await?;
    let name = command.split(' ').next().unwrap_or("FETCH").to_string();
    loop {
        // EOF mid-fetch is a truncated answer too, and the loudest kind: the
        // slice is incomplete and there is not even a status to read.
        let Some(response) = session.read_response().await? else {
            return Err(ImapError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                format!("{name}: the connection closed before the server answered"),
            )));
        };
        match response.parsed() {
            Response::Fetch(_, attrs) => on_item(attrs),
            Response::Done {
                tag: answered,
                status,
                information,
                ..
            } if answered == &tag => {
                return match status {
                    Status::Ok => Ok(()),
                    _ => Err(ImapError::Protocol(format!(
                        "{name}: {status:?} {}",
                        information.as_deref().unwrap_or_default()
                    ))),
                };
            }
            // A server going down says so and then closes; whatever it had
            // sent by then is a fragment.
            Response::Data {
                status: Status::Bye,
                information,
                ..
            } => {
                return Err(ImapError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    format!(
                        "{name}: the server closed the connection ({})",
                        information.as_deref().unwrap_or_default()
                    ),
                )));
            }
            // EXISTS, EXPUNGE, an unsolicited FLAGS: not this command's
            // business, and not a reason to stop reading.
            _ => {}
        }
    }
}

/// Marks every folder the pass never reached as failed, so the caller sees
/// "the connection was lost" rather than a verdict about mail it never saw.
fn fail_the_rest(out: &mut Vec<PassOutcome>, total: usize) {
    while out.len() < total {
        out.push(PassOutcome::Failed {
            detail: "the connection was lost before this folder was reached".into(),
        });
    }
}

/// Whether an error means the session is gone, as opposed to the server
/// refusing one command on a session that is still there.
///
/// The difference decides what the rest of a pass does. After a refusal the
/// next folder can be asked as usual. After a dead socket it cannot: the
/// stream yields nothing forever, every STATUS comes back empty, and an
/// empty STATUS reads as a UIDVALIDITY reset — which used to send every
/// remaining folder into a re-mapping that stripped the server numbers from
/// all but its newest messages. A lid closed mid-pass is all it took.
fn session_is_dead(e: &ImapError) -> bool {
    matches!(
        e,
        ImapError::Io(_)
            | ImapError::Imap(async_imap::error::Error::Io(_))
            | ImapError::Imap(async_imap::error::Error::ConnectionLost)
    )
}

/// The UID a FETCH item named, if it named one.
fn attr_uid(attrs: &[AttributeValue<'_>]) -> Option<u32> {
    attrs.iter().find_map(|a| match a {
        AttributeValue::Uid(uid) => Some(*uid),
        _ => None,
    })
}

/// The whole message, from `BODY[]` or `RFC822`.
///
/// `NIL` and a zero-length literal both mean the server had nothing to give:
/// neither is a message, and storing either would be storing an empty one.
fn attr_body<'a>(attrs: &'a [AttributeValue<'a>]) -> Option<&'a [u8]> {
    attrs
        .iter()
        .find_map(|a| match a {
            AttributeValue::BodySection {
                section: None,
                data: Some(body),
                ..
            }
            | AttributeValue::Rfc822(Some(body)) => Some(body.as_ref()),
            _ => None,
        })
        .filter(|body| !body.is_empty())
}

/// The header block from `BODY[HEADER…]` or `RFC822.HEADER`.
fn attr_header<'a>(attrs: &'a [AttributeValue<'a>]) -> Option<&'a [u8]> {
    attrs.iter().find_map(|a| match a {
        AttributeValue::BodySection {
            section: Some(SectionPath::Full(MessageSection::Header)),
            data: Some(header),
            ..
        }
        | AttributeValue::Rfc822Header(Some(header)) => Some(header.as_ref()),
        _ => None,
    })
}

/// The flag names on a FETCH item.
fn attr_flags<'a>(attrs: &'a [AttributeValue<'a>]) -> impl Iterator<Item = Flag<'a>> {
    attrs
        .iter()
        .filter_map(|a| match a {
            AttributeValue::Flags(flags) => Some(flags),
            _ => None,
        })
        .flatten()
        .map(|f| Flag::from(f.as_ref()))
}

/// Gmail's labels on a FETCH item.
fn attr_labels(attrs: &[AttributeValue<'_>]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|a| match a {
            AttributeValue::GmailLabels(labels) => Some(labels),
            _ => None,
        })
        .flatten()
        .map(|l| l.to_string())
        .collect()
}

/// Ends a session, and gives up if the server will not answer.
///
/// Untimed, this held the whole cycle on a socket that was already dead: the
/// mail was in, and the pass could not finish saying so.
async fn sign_out<S>(session: &mut Session<S>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    match tokio::time::timeout(command_timeout(), session.logout()).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(ImapError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the server did not answer LOGOUT",
        ))),
    }
}

/// Leaves IDLE, and gives up if the server will not answer DONE.
///
/// The wake is already in hand by this point, so a server that stops answering
/// here parks a watcher that has news and nobody to give it to — for as long
/// as the process lives.
async fn end_idle<S>(handle: async_imap::extensions::idle::Handle<S>) -> Result<Session<S>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug + 'static,
{
    match tokio::time::timeout(command_timeout(), handle.done()).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(ImapError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the server did not answer DONE",
        ))),
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
    session.select(wire_name(folder)).await?;

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
        session = end_idle(handle).await?;
        if woke {
            on_wake();
        }
    }
    sign_out(&mut session).await?;
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
    session.select(wire_name(folder)).await?;

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
    let mut session = end_idle(handle).await?;
    sign_out(&mut session).await?;
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

/// One slice of a catch-up fetch after the first seed: a closed UID range,
/// never `{uid}:*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchupSlice {
    /// The UIDs to fetch, or empty when nothing sits above the watermark.
    pub range: String,
    /// The highest UID the slice reaches; the next slice starts above it.
    pub last: u32,
    /// Whether the slice reaches the server's UIDNEXT. Never true without one.
    pub covered: bool,
    /// The UIDNEXT to record once the slice is in: the server's own when
    /// covered, one past the slice otherwise, and nothing when the server gave
    /// none — a watermark invented above mail that was never fetched would
    /// skip that mail for good.
    pub uid_next: Option<u32>,
}

/// How many UIDs one catch-up FETCH asks for.
pub const CATCHUP_SLICE: u32 = 200;

pub fn catchup_slice(since_uid: u32, server_uid_next: Option<u32>, chunk: u32) -> CatchupSlice {
    let start = since_uid.saturating_add(1);
    let chunk = chunk.max(1);
    let cap_end = start.saturating_add(chunk - 1);
    let server_end = server_uid_next.map(|n| n.saturating_sub(1));
    if let Some(end) = server_end
        && start > end
    {
        return CatchupSlice {
            range: String::new(),
            last: since_uid,
            covered: true,
            uid_next: server_uid_next,
        };
    }
    let last = match server_end {
        Some(end) => cap_end.min(end),
        None => cap_end,
    };
    let covered = server_end.is_some_and(|end| last >= end);
    let uid_next = match server_end {
        None => None,
        Some(_) if covered => server_uid_next,
        Some(_) => Some(last.saturating_add(1)),
    };
    CatchupSlice {
        range: format!("{start}:{last}"),
        last,
        covered,
        uid_next,
    }
}

/// The UIDNEXT a pass may record, held back by whatever it could not take.
///
/// `refused` is the lowest UID the server listed and gave no body for —
/// recording anything above it would skip that message for good. `unplaceable`
/// is an item that named no UID at all, which cannot be pinned to a number, so
/// nothing is recorded and the slice is asked for again.
fn watermark(reported: Option<u32>, refused: Option<u32>, unplaceable: bool) -> Option<u32> {
    if unplaceable {
        return None;
    }
    match (reported, refused) {
        (Some(n), Some(uid)) => Some(n.min(uid)),
        (n, _) => n,
    }
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
    let client = Client::new(connect(cfg).await?);
    sync_pass_session(client, cfg, passes, want_keywords, on_message).await
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
            .status(
                wire_name(&pass.path),
                "(MESSAGES UIDNEXT UIDVALIDITY HIGHESTMODSEQ)",
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let e: ImapError = e.into();
                let dead = session_is_dead(&e);
                out.push(PassOutcome::Failed {
                    detail: e.to_string(),
                });
                if dead {
                    fail_the_rest(&mut out, passes.len());
                    break;
                }
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
        let mailbox = match session.examine(wire_name(&pass.path)).await {
            Ok(m) => m,
            Err(e) => {
                out.push(PassOutcome::Failed {
                    detail: e.to_string(),
                });
                continue;
            }
        };

        let mut fetched = 0usize;
        let mut catchup_uid_next: Option<u32> = None;
        // Keywords ride with the first fetch as well as the CONDSTORE diff.
        // Tags set elsewhere on mail that was already tagged when Petrel
        // first saw it used to wait for a later flag change to bump the
        // modseq, and on a server without CONDSTORE never arrived at all.
        let mut keyword_updates: Vec<(u32, Vec<String>)> = Vec::new();
        // The lowest UID the server listed and would not give a body for, and
        // a flag for an item that named no UID at all. Both hold the watermark
        // back: a message counted as fetched and never stored is a message
        // nothing will ever ask for again.
        let mut refused_uid: Option<u32> = None;
        let mut unplaceable_item = false;
        let mut failure: Option<String> = None;
        // Whether a failure took the session with it; see `session_is_dead`.
        let mut dead = false;
        if new_mail {
            let query = "(UID FLAGS BODY.PEEK[])";
            if pass.since_uid == 0 {
                if mailbox.exists > 0 {
                    let first = mailbox
                        .exists
                        .saturating_sub(pass.seed_window.saturating_sub(1))
                        .max(1);
                    let range = format!("{first}:{last}", last = mailbox.exists);
                    let result = fetch_command(
                        &mut session,
                        format!("FETCH {range} {query}"),
                        |attrs| match (attr_uid(attrs), attr_body(attrs)) {
                            (Some(uid), Some(body)) => {
                                on_message(index, uid, flags_to_bits(attr_flags(attrs)), body);
                                fetched += 1;
                                if want_keywords {
                                    let keywords = keywords_of(attr_flags(attrs));
                                    if !keywords.is_empty() {
                                        keyword_updates.push((uid, keywords));
                                    }
                                }
                            }
                            (Some(uid), None) => {
                                refused_uid =
                                    Some(refused_uid.map_or(uid, |lowest| lowest.min(uid)))
                            }
                            // An item with a body but no UID cannot be placed; a bare
                            // `* n FETCH (FLAGS ...)` the server volunteers mid-way
                            // is not this command's answer and holds nothing back.
                            (None, Some(_)) => unplaceable_item = true,
                            (None, None) => {}
                        },
                    )
                    .await;
                    if let Err(e) = result {
                        dead |= session_is_dead(&e);
                        failure = Some(e.to_string());
                    }
                }
            } else {
                // Not `{uid}:*`: a stale watermark plus that range is the
                // rest of the mailbox as full bodies in one FETCH, and macOS
                // keeps the RSS after the Vecs drop. Slices of CATCHUP_SLICE
                // instead, one FETCH each, and every slice in this pass until
                // the server's UIDNEXT is reached — not one slice per cycle,
                // or a week's mail would arrive two hundred at a time, oldest
                // first, five minutes apart. Each message is handed on as it
                // arrives, so a slice's memory is gone before the next.
                let server_next = mailbox.uid_next.or(status.uid_next);
                let mut since = pass.since_uid;
                loop {
                    let slice = catchup_slice(since, server_next, CATCHUP_SLICE);
                    catchup_uid_next = slice.uid_next;
                    if slice.range.is_empty() {
                        break;
                    }
                    let mut got = 0usize;
                    let result = fetch_command(
                        &mut session,
                        format!("UID FETCH {} {query}", slice.range),
                        |attrs| match (attr_uid(attrs), attr_body(attrs)) {
                            (Some(uid), Some(body)) => {
                                if uid > since {
                                    on_message(index, uid, flags_to_bits(attr_flags(attrs)), body);
                                    fetched += 1;
                                    got += 1;
                                    if want_keywords {
                                        let keywords = keywords_of(attr_flags(attrs));
                                        if !keywords.is_empty() {
                                            keyword_updates.push((uid, keywords));
                                        }
                                    }
                                }
                            }
                            (Some(uid), None) => {
                                refused_uid =
                                    Some(refused_uid.map_or(uid, |lowest| lowest.min(uid)))
                            }
                            // An item with a body but no UID cannot be placed; a bare
                            // `* n FETCH (FLAGS ...)` the server volunteers mid-way
                            // is not this command's answer and holds nothing back.
                            (None, Some(_)) => unplaceable_item = true,
                            (None, None) => {}
                        },
                    )
                    .await;
                    if let Err(e) = result {
                        // The slice is incomplete, so the watermark stays
                        // where it was and this slice is asked for again next
                        // cycle rather than skipped.
                        dead |= session_is_dead(&e);
                        failure = Some(e.to_string());
                        break;
                    }
                    if slice.covered {
                        break;
                    }
                    // With no UIDNEXT there is no end to aim at: the first
                    // slice that brings nothing is the end.
                    if server_next.is_none() && got == 0 {
                        break;
                    }
                    since = slice.last;
                }
            }
        }
        if let Some(detail) = failure {
            out.push(PassOutcome::Failed { detail });
            if dead {
                fail_the_rest(&mut out, passes.len());
                break;
            }
            continue;
        }

        let mut flag_updates = Vec::new();
        // A refused diff keeps the old baseline, so the changes it would
        // have reported are asked for again next cycle; what this pass
        // fetched still stands. Failing the folder instead re-fetched every
        // new message on the next cycle too, since the watermark was never
        // recorded — and on an emptied mailbox the diff is refused every
        // time, so it is not asked at all.
        let mut diff_refused = false;
        if flags_moved
            && status.exists > 0
            && let Some(seen) = pass.since_modseq
        {
            // A refused diff is not a folder with nothing to report. Recording
            // the new modseq after one loses those flag changes for good,
            // because the next diff starts above them.
            let result = fetch_command(
                &mut session,
                format!("UID FETCH 1:* (FLAGS) (CHANGEDSINCE {seen})"),
                |attrs| {
                    if let Some(uid) = attr_uid(attrs) {
                        flag_updates.push((uid, flags_to_bits(attr_flags(attrs))));
                        if want_keywords {
                            keyword_updates.push((uid, keywords_of(attr_flags(attrs))));
                        }
                    }
                },
            )
            .await;
            if let Err(e) = result {
                if session_is_dead(&e) {
                    out.push(PassOutcome::Failed {
                        detail: e.to_string(),
                    });
                    fail_the_rest(&mut out, passes.len());
                    break;
                }
                diff_refused = true;
            }
        }

        out.push(PassOutcome::Fetched {
            fetched,
            uid_validity: mailbox.uid_validity.or(status.uid_validity),
            highest_modseq: if diff_refused {
                pass.since_modseq
            } else {
                mailbox.highest_modseq.or(status.highest_modseq)
            },
            // A pass that fetched nothing must not move the watermark past
            // mail it never saw. STATUS said "no new mail" with one UIDNEXT;
            // EXAMINE, a moment later, can report a higher one because a
            // message landed in between — recording that skipped the message
            // for good, since the next pass starts above it. The same applies
            // to a message the server listed and would not hand over: the
            // watermark stops below the lowest of those, and an item that
            // named no UID stops it moving at all.
            uid_next: watermark(
                if new_mail {
                    catchup_uid_next.or(mailbox.uid_next).or(status.uid_next)
                } else {
                    status.uid_next
                },
                refused_uid,
                unplaceable_item,
            ),
            flag_updates,
            keyword_updates,
            total: mailbox.exists,
        });
    }
    // Best effort, unlike everywhere else: the mail is already ingested and
    // every folder has said what happened to it. Throwing all of that away
    // because a dead socket would not answer LOGOUT — which is exactly the
    // case a folder just reported — would lose the report of the failure
    // along with the cycle's work.
    let _ = sign_out(&mut session).await;
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
    let client = Client::new(connect(cfg).await?);
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
    let mailbox = session.select(wire_name(folder)).await?;
    let uid_validity = mailbox.uid_validity;
    // Checked before a single byte is fetched: after a reset the watermark
    // is meaningless, and `{since+1}:*` in the new numbering could be
    // anything — most of the folder, or none of it.
    if let Some(expected) = expected_validity
        && uid_validity != Some(expected)
    {
        sign_out(&mut session).await?;
        return Ok(FetchOutcome::ValidityChanged { now: uid_validity });
    }
    let mut n = 0usize;
    fetch_command(
        &mut session,
        format!(
            "UID FETCH {}:* (UID FLAGS RFC822)",
            since_uid.saturating_add(1)
        ),
        |attrs| {
            if let (Some(uid), Some(body)) = (attr_uid(attrs), attr_body(attrs))
                && uid > since_uid
            {
                on_message(uid, flags_to_bits(attr_flags(attrs)), body);
                n += 1;
            }
        },
    )
    .await?;
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    id_map_session(client, cfg, folder, depth).await
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
    let mailbox = session.select(wire_name(folder)).await?;
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
        // A truncated listing is worse here than anywhere: the caller reads it
        // as "the server no longer holds these", and evicts.
        fetch_command(
            &mut session,
            format!("FETCH {start}:* (UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"),
            |attrs| {
                if let Some(uid) = attr_uid(attrs) {
                    map.entries
                        .push((uid, attr_header(attrs).and_then(message_id_of)));
                }
            },
        )
        .await?;
    }
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    id_range_session(client, cfg, folder, &set).await
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
    session.select(wire_name(folder)).await?;
    let mut out = Vec::new();
    fetch_command(
        &mut session,
        format!("UID FETCH {set} (UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])"),
        |attrs| {
            if let Some(uid) = attr_uid(attrs) {
                out.push((uid, attr_header(attrs).and_then(message_id_of)));
            }
        },
    )
    .await?;
    sign_out(&mut session).await?;
    Ok(out)
}

/// One folder's UIDNEXT, by STATUS — where a fresh All Mail walk starts.
pub async fn folder_uidnext(cfg: &ImapConfig, folder: &str) -> Result<Option<u32>> {
    let client = Client::new(connect(cfg).await?);
    let mut session = sign_in(client, cfg).await?;
    let s = session.status(wire_name(folder), "(UIDNEXT)").await?;
    sign_out(&mut session).await?;
    Ok(s.uid_next)
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
    let client = Client::new(connect(cfg).await?);
    uid_set_session(client, cfg, folder, &set, &mut on_message).await
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
        let client = Client::new(connect(cfg).await?);
        n += uid_set_session(client, cfg, folder, &set, &mut on_message).await?;
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
    session.select(wire_name(folder)).await?;
    let mut n = 0usize;
    // PEEK, as everywhere: fetching mail must not mark it read. A refused
    // range is an error rather than an empty one, because the backfill reads
    // "nothing here" as "this stretch of numbers is spent" and moves on.
    fetch_command(
        &mut session,
        format!("UID FETCH {set} (UID FLAGS BODY.PEEK[])"),
        |attrs| {
            if let (Some(uid), Some(body)) = (attr_uid(attrs), attr_body(attrs)) {
                on_message(uid, flags_to_bits(attr_flags(attrs)), body);
                n += 1;
            }
        },
    )
    .await?;
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    store_flag_session(client, cfg, folder, uid, flag, add).await
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
    session.select(wire_name(folder)).await?;
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
    sign_out(&mut session).await?;
    Ok(())
}

/// How many UIDs one STORE carries. A mark-read on a twenty-thousand-message
/// thread used to open that many connections; a hundred on one session is
/// what the drain now asks for.
pub const STORE_FLAG_BATCH: usize = 100;

/// Same as [`store_flag`], for many UIDs on one connection.
pub async fn store_flags(
    cfg: &ImapConfig,
    folder: &str,
    uids: &[u32],
    flag: &str,
    add: bool,
) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    if uids.len() == 1 {
        return store_flag(cfg, folder, uids[0], flag, add).await;
    }
    let client = Client::new(connect(cfg).await?);
    store_flags_session(client, cfg, folder, uids, flag, add).await
}

async fn store_flags_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    folder: &str,
    uids: &[u32],
    flag: &str,
    add: bool,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    session.select(wire_name(folder)).await?;
    let op = if add {
        "+FLAGS.SILENT"
    } else {
        "-FLAGS.SILENT"
    };
    let set = uids
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");
    {
        let mut updates = session.uid_store(set, format!("{op} ({flag})")).await?;
        while updates.next().await.is_some() {}
    }
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    store_labels_session(client, cfg, folder, uid, label, add).await
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
    session.select(wire_name(folder)).await?;
    let op = if add { "+X-GM-LABELS" } else { "-X-GM-LABELS" };
    {
        let mut updates = session
            .uid_store(uid.to_string(), format!("{op} ({})", quote_imap(label)))
            .await?;
        while updates.next().await.is_some() {}
    }
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    expunge_uid_session(client, cfg, folder, uid, server_has_uidplus).await
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
    session.select(wire_name(folder)).await?;
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
    sign_out(&mut session).await?;
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

/// Whether a refused CREATE was refused because the folder is already there.
///
/// The response code where the server sends one (RFC 5530), and the words
/// otherwise. Deliberately not "contains `exist`": "mailbox does not exist"
/// and "parent does not exist" both contain it, and both mean the folder was
/// not created.
fn already_exists(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("[alreadyexists]")
        || lower.contains("already exists")
        || lower.contains("already exist")
}

async fn folder_op(cfg: &ImapConfig, op: FolderOp, a: &str, b: &str) -> Result<()> {
    let client = Client::new(connect(cfg).await?);
    folder_op_session(client, cfg, op, a, b).await
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
        FolderOp::Create => session.create(wire_name(a)).await,
        FolderOp::Rename => session.rename(wire_name(a), wire_name(b)).await,
        FolderOp::Delete => session.delete(wire_name(a)).await,
    };
    match result {
        Ok(()) => {}
        // "Already exists" answers a CREATE the way success does: the folder
        // the caller wanted is there. Everything else is a real failure —
        // including "mailbox does not exist", which contains the word and
        // used to be read as success, so a CREATE that failed for want of a
        // parent folder was reported as a folder that had been made.
        Err(e) if matches!(op, FolderOp::Create) && already_exists(&e.to_string()) => {}
        Err(e) => {
            let _ = sign_out(&mut session).await;
            return Err(e.into());
        }
    }
    sign_out(&mut session).await?;
    Ok(())
}

/// Moves one message: by MOVE where the server has it, and by COPY, \Deleted
/// and an expunge where it does not.
///
/// The fallback is where the care is. A retry after a COPY that landed and a
/// STORE that did not used to COPY again, and the destination gained a second
/// copy; given the Message-ID, the destination is asked first and a copy
/// already there is not made twice. The expunge is by UID where UIDPLUS
/// allows it: a bare EXPUNGE commits every other pending deletion in the
/// mailbox, other clients' included. Without UIDPLUS the source copy is
/// marked \Deleted and left for the server's next compaction, as
/// `expunge_uid` does, and the caller is told so (`Ok(false)`).
pub async fn move_uid(
    cfg: &ImapConfig,
    from: &str,
    uid: u32,
    to: &str,
    server_has_move: bool,
    server_has_uidplus: bool,
    message_id: Option<&str>,
) -> Result<bool> {
    let client = Client::new(connect(cfg).await?);
    move_uid_session(
        client,
        cfg,
        from,
        uid,
        to,
        server_has_move,
        server_has_uidplus,
        message_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn move_uid_session<S>(
    client: Client<S>,
    cfg: &ImapConfig,
    from: &str,
    uid: u32,
    to: &str,
    server_has_move: bool,
    server_has_uidplus: bool,
    message_id: Option<&str>,
) -> Result<bool>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
{
    let mut session = sign_in(client, cfg).await?;
    if server_has_move {
        session.select(wire_name(from)).await?;
        session.uid_mv(uid.to_string(), wire_name(to)).await?;
        sign_out(&mut session).await?;
        return Ok(true);
    }
    // A copy that already landed is not made again.
    let already_there = match message_id {
        Some(id) => {
            session.select(wire_name(to)).await?;
            let query = format!("HEADER Message-ID {}", quote_imap(id));
            !session.uid_search(query).await?.is_empty()
        }
        None => false,
    };
    session.select(wire_name(from)).await?;
    if !already_there {
        session.uid_copy(uid.to_string(), wire_name(to)).await?;
    }
    {
        let mut updates = session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await?;
        while updates.next().await.is_some() {}
    }
    let expunged = if server_has_uidplus {
        // The expunge stream is not Unpin, so it has to be pinned before it
        // can be driven.
        let updates = session.uid_expunge(uid.to_string()).await?;
        futures::pin_mut!(updates);
        while updates.next().await.is_some() {}
        true
    } else {
        false
    };
    sign_out(&mut session).await?;
    Ok(expunged)
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
    // Quoted properly rather than by deleting quotes: an id is generated by
    // whoever sent the message, and a backslash in one used to end the string
    // early and leave the rest being read as IMAP.
    let query = format!("HEADER Message-ID {}", quote_imap(message_id));
    let client = Client::new(connect(cfg).await?);
    uid_search_session(client, cfg, folder, &query).await
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
    session.select(wire_name(folder)).await?;
    let hits = session.uid_search(query).await?;
    let mut found: Vec<u32> = hits.into_iter().collect();
    found.sort_unstable();
    sign_out(&mut session).await?;
    Ok(found)
}

/// How many UIDs one SEARCH asks about.
const SEARCH_RANGE: u32 = 50_000;

/// Every UID the folder currently holds — the ground truth a placement sweep
/// compares against.
///
/// In ranges rather than one `SEARCH ALL`. All Mail on a long-lived Gmail
/// account holds three hundred thousand messages, and asking about all of them
/// at once cost twenty-one seconds of server CPU, every twenty minutes, until
/// the backfill finished. A UID range is answered from the index instead.
pub async fn uids_in_folder(cfg: &ImapConfig, folder: &str) -> Result<Vec<u32>> {
    let client = Client::new(connect(cfg).await?);
    let mut session = sign_in(client, cfg).await?;
    let mailbox = session.select(wire_name(folder)).await?;
    let mut found = Vec::new();
    match mailbox.uid_next {
        // Without a UIDNEXT there is no last number to walk towards, so the
        // one broad question is the only one that can be asked.
        None => found.extend(session.uid_search("ALL").await?),
        Some(uid_next) => {
            let mut first = 1u32;
            while first < uid_next {
                let last = first.saturating_add(SEARCH_RANGE - 1).min(uid_next - 1);
                found.extend(session.uid_search(format!("UID {first}:{last}")).await?);
                if last == uid_next - 1 {
                    break;
                }
                first = last + 1;
            }
        }
    }
    found.sort_unstable();
    sign_out(&mut session).await?;
    Ok(found)
}

pub async fn find_message_id(cfg: &ImapConfig, folder: &str, message_id: &str) -> Result<Vec<u32>> {
    // Message-ID values are generated by us; quote defensively regardless.
    // Quoted properly rather than by deleting quotes: an id is generated by
    // whoever sent the message, and a backslash in one used to end the string
    // early and leave the rest being read as IMAP.
    let query = format!("HEADER Message-ID {}", quote_imap(message_id));
    let client = Client::new(connect(cfg).await?);
    search_session(client, cfg, folder, &query).await
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
    session.select(wire_name(folder)).await?;
    let hits = session.search(query).await?;
    let mut found: Vec<u32> = hits.into_iter().collect();
    found.sort_unstable();
    sign_out(&mut session).await?;
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
    // One ceiling over the whole check, as the SMTP half has. A host that
    // accepts the connection and never greets used to hold the setup form
    // for as long as the socket lived.
    let limit = crate::smtp::check_timeout();
    match tokio::time::timeout(limit, login_check_inner(cfg)).await {
        Ok(result) => result,
        Err(_) => Err(ImapError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "no answer from {}:{} after {}s",
                cfg.host,
                cfg.port,
                limit.as_secs()
            ),
        ))),
    }
}

async fn login_check_inner(cfg: &ImapConfig) -> Result<()> {
    let client = Client::new(connect(cfg).await?);
    let session = sign_in(client, cfg).await?;
    let mut session = session;
    sign_out(&mut session).await?;
    Ok(())
}

pub async fn probe(cfg: &ImapConfig, fetch_limit: u32) -> Result<ProbeReport> {
    let client = Client::new(connect(cfg).await?);
    probe_session(client, cfg, fetch_limit).await
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_folded_message_id_is_read_whole() {
        // Exchange folds long ids onto the continuation line; read line by
        // line, the first line's value is empty.
        let folded = b"Message-ID:\r\n <long.id.1234@mail.example.com>\r\n\r\n";
        assert_eq!(
            super::message_id_of(folded).as_deref(),
            Some("long.id.1234@mail.example.com")
        );
        let plain = b"Message-ID: <a@b>\r\n\r\n";
        assert_eq!(super::message_id_of(plain).as_deref(), Some("a@b"));
        let other_first = b"Subject: x\r\n folded subject\r\nMessage-ID: <c@d>\r\n\r\n";
        assert_eq!(super::message_id_of(other_first).as_deref(), Some("c@d"));
    }

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

    #[test]
    fn a_catchup_slice_is_closed_and_says_how_far_it_got() {
        let s = super::catchup_slice(100, Some(1000), 200);
        assert_eq!(
            (s.range.as_str(), s.last, s.covered, s.uid_next),
            ("101:300", 300, false, Some(301))
        );
        let s = super::catchup_slice(800, Some(1000), 200);
        assert_eq!(
            (s.range.as_str(), s.last, s.covered, s.uid_next),
            ("801:999", 999, true, Some(1000))
        );
        let s = super::catchup_slice(999, Some(1000), 200);
        assert_eq!(
            (s.range.as_str(), s.covered, s.uid_next),
            ("", true, Some(1000))
        );
    }

    /// A server that gives no UIDNEXT gets no invented watermark: one past an
    /// unfetched slice would have skipped everything under it for good.
    #[test]
    fn without_a_server_uidnext_no_watermark_is_claimed() {
        let s = super::catchup_slice(100, None, 200);
        assert_eq!(s.range, "101:300");
        assert_eq!(s.last, 300);
        assert!(!s.covered);
        assert_eq!(s.uid_next, None);
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
    let client = Client::new(connect(cfg).await?);
    store_flag_all_session(client, cfg, folder, flag, add).await
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
    let mailbox = session.select(wire_name(folder)).await?;
    let n = mailbox.exists;
    // Nothing to do, and `1:*` on an empty mailbox is a command some servers
    // answer with an error rather than a shrug.
    if n == 0 {
        sign_out(&mut session).await?;
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
    sign_out(&mut session).await?;
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
    let client = Client::new(connect(cfg).await?);
    move_all_session(client, cfg, from, to, server_has_move).await
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
    let mailbox = session.select(wire_name(from)).await?;
    let n = mailbox.exists;
    if n == 0 {
        sign_out(&mut session).await?;
        return Ok(0);
    }
    if server_has_move {
        session.uid_mv("1:*", wire_name(to)).await?;
    } else {
        session.uid_copy("1:*", wire_name(to)).await?;
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
    sign_out(&mut session).await?;
    Ok(n)
}
