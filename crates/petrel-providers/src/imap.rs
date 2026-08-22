//! IMAP backend — M0 slice.
//!
//! Establishes a session, reads the server's capabilities, and derives the sync
//! strategy from them: QRESYNC → CONDSTORE → full reconcile. The ladder is the
//! whole point; Gmail-over-IMAP has no QRESYNC and Microsoft 365 has no
//! CONDSTORE, so the bottom rung is the common case, not an edge case.

use std::sync::Arc;

use async_imap::Client;
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
    pub pass: String,
    pub security: Security,
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
    let mut session = client
        .login(&cfg.user, &cfg.pass)
        .await
        .map_err(|(e, _)| e)?;

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
    if mailbox.exists > 0 {
        let last = mailbox.exists;
        let first = last.saturating_sub(fetch_limit.saturating_sub(1)).max(1);
        let range = format!("{first}:{last}");
        let mut fetches = session
            .fetch(range, "(UID FLAGS RFC822.SIZE ENVELOPE)")
            .await?;
        while let Some(fetch) = fetches.next().await {
            let fetch = fetch?;
            let envelope = fetch.envelope();
            let subject = envelope
                .and_then(|e| e.subject.as_ref())
                .map(|s| String::from_utf8_lossy(s).to_string())
                .unwrap_or_default();
            let from = envelope
                .and_then(|e| e.from.as_ref())
                .and_then(|f| f.first())
                .map(|a| {
                    let mbox = a
                        .mailbox
                        .as_ref()
                        .map(|m| String::from_utf8_lossy(m).to_string());
                    let host = a
                        .host
                        .as_ref()
                        .map(|h| String::from_utf8_lossy(h).to_string());
                    match (mbox, host) {
                        (Some(m), Some(h)) => format!("{m}@{h}"),
                        (Some(m), None) => m,
                        _ => String::new(),
                    }
                })
                .unwrap_or_default();
            headers.push(FetchedHeader {
                uid: fetch.uid,
                subject,
                from,
                size: fetch.size,
                seen: fetch
                    .flags()
                    .any(|f| matches!(f, async_imap::types::Flag::Seen)),
            });
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
pub async fn append_message(cfg: &ImapConfig, folder: &str, raw: &[u8]) -> Result<()> {
    match cfg.security {
        Security::Tls => {
            let client = Client::new(tls_stream(&cfg.host, cfg.port).await?);
            let mut session = client
                .login(&cfg.user, &cfg.pass)
                .await
                .map_err(|(e, _)| e)?;
            session.append(folder, None, None, raw).await?;
            session.logout().await?;
        }
        #[cfg(feature = "insecure-plaintext")]
        Security::InsecurePlaintext => {
            let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
            let client = Client::new(tcp);
            let mut session = client
                .login(&cfg.user, &cfg.pass)
                .await
                .map_err(|(e, _)| e)?;
            session.append(folder, None, None, raw).await?;
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

/// Fetches recent messages, handing each one to `on_message` as it arrives.
///
/// The buffering version holds every message in memory and returns only once
/// the last one lands, so a caller can show no progress and hold no mail until
/// the whole batch is done — which reads as a hang, and on a large mailbox is
/// one in every sense that matters. Streaming lets the store take each message
/// as it comes and the UI count climb while it does.
pub async fn fetch_raw_each<F>(
    cfg: &ImapConfig,
    folder: &str,
    limit: u32,
    on_message: F,
) -> Result<usize>
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
) -> Result<usize>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + std::fmt::Debug,
    F: FnMut(u32, i64, &[u8]),
{
    let mut session = client
        .login(&cfg.user, &cfg.pass)
        .await
        .map_err(|(e, _)| e)?;
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
    session.logout().await?;
    Ok(n)
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
    let mut session = client
        .login(&cfg.user, &cfg.pass)
        .await
        .map_err(|(e, _)| e)?;
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

/// Searches a folder for a Message-ID. This is the evidence-gathering half of
/// the ambiguous-send rule: after a send whose outcome we could not read, we
/// ask the server whether it actually has the message rather than guessing.
/// Returns the matching sequence numbers (empty = provably absent).
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
    let mut session = client
        .login(&cfg.user, &cfg.pass)
        .await
        .map_err(|(e, _)| e)?;
    session.select(folder).await?;
    let hits = session.search(query).await?;
    let mut found: Vec<u32> = hits.into_iter().collect();
    found.sort_unstable();
    session.logout().await?;
    Ok(found)
}

/// Connects, authenticates, and reports what the server supports and holds.
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
    None
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
}
