//! UTF-8 in IMAP ENVELOPE parsing and probe header sampling.
//!
//! Two regression fences for the IMAP UTF-8 envelope bug class:
//!
//! 1. **Parser** — imap-proto must accept UTF-8 quoted-strings inside ENVELOPE
//!    (unpatched versions fail with a nom `TakeWhile1` on non-ASCII bytes).
//! 2. **Probe** — account setup lists folders without fetching when the limit is
//!    zero, and samples preview headers via `BODY.PEEK[HEADER.FIELDS …]`, never
//!    ENVELOPE. Japanese text must round-trip from raw header bytes.

use async_imap::imap_proto::parser::parse_response;
use async_imap::imap_proto::types::{AttributeValue, Response};

/// Synthetic FETCH line: UTF-8 in ENVELOPE quoted-strings. Fake addresses only.
const SYNTHETIC_ENVELOPE_FETCH: &str = concat!(
    r#"* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 100 ENVELOPE ("Mon, 31 Aug 2026 06:33:39 +0000" "#,
    r#""会議の件" (("事務局" NIL "info" "example.jp")) (("事務局" NIL "info" "example.jp")) "#,
    r#"((NIL NIL "info" "example.jp")) ((NIL NIL "user" "example.com")) NIL NIL NIL "#,
    r#""<synthetic@example.test>"))"#,
    "\r\n"
);

#[test]
fn utf8_envelope_quoted_strings_parse() {
    // Unpatched imap-proto dies here with TakeWhile1; Ok means the UTF-8 patch landed.
    let (rest, response) =
        parse_response(SYNTHETIC_ENVELOPE_FETCH.as_bytes()).expect("ENVELOPE FETCH must parse");
    assert!(
        rest.is_empty(),
        "parser should consume the whole line, leftover: {:?}",
        std::str::from_utf8(rest)
    );

    let Response::Fetch(seq, attrs) = response else {
        panic!("expected FETCH, got {response:?}");
    };
    assert_eq!(seq, 1);

    let envelope = attrs
        .iter()
        .find_map(|attr| match attr {
            AttributeValue::Envelope(e) => Some(e.as_ref()),
            _ => None,
        })
        .expect("FETCH should carry an ENVELOPE attribute");

    let subject = std::str::from_utf8(envelope.subject.as_ref().expect("subject"))
        .expect("subject must be valid UTF-8");
    assert!(subject.contains("会議"), "subject: {subject}");

    let from = envelope
        .from
        .as_ref()
        .expect("from")
        .first()
        .expect("one from");
    let mailbox = std::str::from_utf8(from.mailbox.as_ref().expect("mailbox")).unwrap();
    assert_eq!(mailbox, "info");
}

#[cfg(feature = "insecure-plaintext")]
mod scripted {
    use std::sync::{Arc, Mutex};

    use petrel_providers::imap::{Credential, ImapConfig, Security, probe};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    use super::SYNTHETIC_ENVELOPE_FETCH;

    fn synthetic_header_literal() -> String {
        "From: 事務局 <info@example.jp>\r\n\
         Subject: 会議の件\r\n\
         Date: Mon, 31 Aug 2026 06:33:39 +0000\r\n\r\n"
            .to_string()
    }

    fn header_fields_fetch_response() -> String {
        let hdr = synthetic_header_literal();
        format!(
            "* 1 FETCH (UID 1 FLAGS () RFC822.SIZE 100 \
             BODY[HEADER.FIELDS (DATE FROM SUBJECT)] {{{}}}\r\n{hdr})\r\n",
            hdr.len()
        )
    }

    /// True when the line is an IMAP FETCH command (tag FETCH …), not a substring
    /// match inside CAPABILITY or another verb.
    fn line_is_fetch(line: &str) -> bool {
        line.split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("FETCH"))
    }

    /// Minimal loopback server: enough for probe (login, list, select, optional fetch).
    async fn utf8_scripted_server(commands: Arc<Mutex<Vec<String>>>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                let commands = Arc::clone(&commands);
                tokio::spawn(async move {
                    let (rx, mut tx) = sock.into_split();
                    let mut reader = BufReader::new(rx);
                    let _ = tx.write_all(b"* OK petrel-utf8-scripted ready\r\n").await;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        commands.lock().unwrap().push(line.clone());

                        let tag = line.split_whitespace().next().unwrap_or("*").to_string();
                        let upper = line.to_ascii_uppercase();
                        let mut out: Vec<u8> = Vec::new();

                        if upper.contains(" CAPABILITY") {
                            out.extend_from_slice(b"* CAPABILITY IMAP4rev1\r\n");
                            out.extend_from_slice(format!("{tag} OK\r\n").as_bytes());
                        } else if upper.contains(" LOGIN ") {
                            out.extend_from_slice(format!("{tag} OK logged in\r\n").as_bytes());
                        } else if upper.contains(" LIST ") {
                            out.extend_from_slice(b"* LIST () \"/\" INBOX\r\n");
                            out.extend_from_slice(format!("{tag} OK\r\n").as_bytes());
                        } else if upper.contains(" SELECT ") || upper.contains(" EXAMINE ") {
                            out.extend_from_slice(
                                b"* 1 EXISTS\r\n* 0 RECENT\r\n\
                                 * OK [UIDVALIDITY 1] UIDs valid\r\n\
                                 * FLAGS (\\Seen \\Flagged)\r\n",
                            );
                            out.extend_from_slice(
                                format!("{tag} OK [READ-WRITE] selected\r\n").as_bytes(),
                            );
                        } else if upper.contains(" FETCH ") && upper.contains("ENVELOPE") {
                            // Would crash unpatched imap-proto if the client ever asked.
                            out.extend_from_slice(SYNTHETIC_ENVELOPE_FETCH.as_bytes());
                            out.extend_from_slice(format!("{tag} OK\r\n").as_bytes());
                        } else if upper.contains(" FETCH ")
                            && (upper.contains("HEADER.FIELDS") || upper.contains("BODY.PEEK"))
                        {
                            out.extend_from_slice(header_fields_fetch_response().as_bytes());
                            out.extend_from_slice(format!("{tag} OK\r\n").as_bytes());
                        } else if upper.contains(" LOGOUT") {
                            out.extend_from_slice(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes());
                            let _ = tx.write_all(&out).await;
                            return;
                        } else {
                            out.extend_from_slice(format!("{tag} OK noop\r\n").as_bytes());
                        }

                        if tx.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        port
    }

    fn probe_cfg(port: u16) -> ImapConfig {
        ImapConfig {
            host: "127.0.0.1".into(),
            port,
            user: "petrel".into(),
            credential: Credential::password("petrelpass"),
            security: Security::InsecurePlaintext,
        }
    }

    #[tokio::test]
    async fn probe_with_limit_zero_does_not_fetch() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let port = utf8_scripted_server(Arc::clone(&commands)).await;
        let cfg = probe_cfg(port);

        let report = probe(&cfg, 0).await.expect("probe with limit 0");

        assert!(
            !report.folders.is_empty(),
            "folder discovery should still run"
        );
        assert!(
            report.headers.is_empty(),
            "limit 0 must not sample message headers"
        );

        let recorded = commands.lock().unwrap().clone();
        assert!(
            !recorded.iter().any(|line| line_is_fetch(line)),
            "probe(limit=0) must not issue FETCH; saw: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn probe_samples_japanese_via_headers() {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let port = utf8_scripted_server(Arc::clone(&commands)).await;
        let cfg = probe_cfg(port);

        let report = probe(&cfg, 1).await.expect("probe with limit 1");

        let recorded = commands.lock().unwrap().clone();
        let Some(fetch_line) = recorded.iter().find(|line| line_is_fetch(line)) else {
            panic!("probe(1) must FETCH; commands: {recorded:?}");
        };
        let upper = fetch_line.to_ascii_uppercase();
        assert!(
            upper.contains("HEADER.FIELDS") && upper.contains("BODY.PEEK"),
            "sample must be BODY.PEEK[HEADER.FIELDS], not ENVELOPE/RFC822: {fetch_line}"
        );
        assert!(
            !upper.contains("ENVELOPE"),
            "probe must not ask for ENVELOPE; commands: {recorded:?}"
        );

        let Some(header) = report.headers.first() else {
            panic!("expected one sampled header, got {:?}", report.headers);
        };
        assert!(
            header.subject.contains("会議"),
            "subject should carry UTF-8 from raw headers: {:?}",
            header.subject
        );
        assert!(
            header.from.contains("info@example.jp"),
            "from should come from the From: header line: {:?}",
            header.from
        );
    }
}
