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
