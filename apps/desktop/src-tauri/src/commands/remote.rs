//! A message's reach outward: remote content, sender trust, and unsubscribing.

use crate::state::{AppState, now_ms};
use std::sync::Arc;
use tauri::State;

/// Who sent this, and whether their remote content is already allowed.
///
/// The reader asks so its banner can offer the right thing: the sender's
/// address to name in "always show images from …", and whether to bother
/// offering at all.
#[derive(serde::Serialize)]
pub(crate) struct RemoteStatus {
    from_addr: String,
    allowed: bool,
    /// True when it is allowed because the user has written to them, rather
    /// than because they were trusted by hand. The two are worth telling apart:
    /// one is a decision to revisit in settings, the other is not a decision
    /// at all and there is nothing in the list to find.
    because_written_to: bool,
}

#[tauri::command(async)]
pub fn remote_status(message_id: i64, state: State<Arc<AppState>>) -> Result<RemoteStatus, String> {
    let store = state.store()?;
    let from = store
        .message_sender(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(RemoteStatus {
            from_addr: from,
            allowed: false,
            because_written_to: false,
        });
    };
    let trusted = store
        .sender_trusted(account, &from)
        .map_err(|e| e.to_string())?;
    let written = store
        .has_written_to(account, &from)
        .map_err(|e| e.to_string())?;
    Ok(RemoteStatus {
        from_addr: from,
        allowed: trusted || written,
        because_written_to: written && !trusted,
    })
}

/// Shows this one message's remote content, for as long as the app is running.
#[tauri::command(async)]
pub fn show_remote_once(message_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    state
        .shown_once
        .lock()
        .map_err(|_| "lock poisoned")?
        .insert(message_id);
    Ok(())
}

/// Trusts this message's sender from now on.
#[tauri::command(async)]
pub fn trust_sender(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Err("no account".into());
    };
    let from = store
        .message_sender(message_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    if from.is_empty() {
        return Err("this message has no sender to trust".into());
    }
    store
        .trust_sender(account, &from, now_ms())
        .map_err(|e| e.to_string())?;
    Ok(from)
}

#[tauri::command(async)]
pub fn trusted_senders(state: State<Arc<AppState>>) -> Result<Vec<String>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.trusted_senders(account).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn untrust_sender(addr: String, state: State<Arc<AppState>>) -> Result<(), String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(());
    };
    store
        .untrust_sender(account, &addr)
        .map_err(|e| e.to_string())
}

/// What the message offers for leaving its list, shaped for the UI.
#[derive(serde::Serialize)]
pub(crate) struct UnsubInfo {
    /// True when RFC 8058 one-click is available — leaving without opening
    /// anything, which is the safest of the three.
    one_click: bool,
    url: Option<String>,
    mailto: Option<String>,
}

/// What the receiving server concluded about who sent a message.
///
/// A verdict relayed, not reached. Petrel cannot check SPF or DKIM itself
/// after the fact: SPF needs the connecting IP, gone by the time the message
/// is stored, and DKIM needs a DNS lookup against a key that may have rotated
/// since. The server that accepted the mail did that work and wrote it down.
#[derive(serde::Serialize)]
pub(crate) struct AuthInfo {
    /// Some(true) only when DMARC passed, Some(false) only when it failed,
    /// None whenever there is nothing to claim. The UI must treat None as
    /// silence, never as suspicion: most legitimate mail carries no verdict.
    verified: Option<bool>,
    /// The domain the sender was checked against, for a sentence a person can
    /// read instead of a status code.
    domain: Option<String>,
    /// Who did the checking. A stamp is only worth what the stamper is worth.
    authserv: Option<String>,
    spf: Option<String>,
    dkim: Option<String>,
    dmarc: Option<String>,
}

fn word(v: Option<petrel_mime::AuthVerdict>) -> Option<String> {
    use petrel_mime::AuthVerdict::*;
    v.map(|v| {
        match v {
            Pass => "pass",
            Fail => "fail",
            Inconclusive => "unknown",
        }
        .to_string()
    })
}

/// Reads the sender authentication verdicts for one message.
#[tauri::command(async)]
pub fn authentication_info(
    message_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<AuthInfo>, String> {
    let raw = raw_message_of(&state, message_id)?;
    Ok(petrel_mime::authentication(&raw).map(|a| AuthInfo {
        verified: a.identity_verified(),
        domain: a.domain.clone(),
        authserv: a.authserv.clone(),
        spf: word(a.spf),
        dkim: word(a.dkim),
        dmarc: word(a.dmarc),
    }))
}

fn raw_message_of(state: &AppState, message_id: i64) -> Result<Vec<u8>, String> {
    let hash = {
        let store = state.store()?;
        store
            .blob_hash_for(message_id)
            .map_err(|e| e.to_string())?
            .ok_or("message body not stored")?
    };
    state
        .blobs
        .read(&hash)
        .map_err(|_| "message body unavailable (failed verification)".into())
}

/// Reads the List-Unsubscribe offer for one message, if it makes one.
#[tauri::command(async)]
pub fn unsubscribe_info(
    message_id: i64,
    state: State<Arc<AppState>>,
) -> Result<Option<UnsubInfo>, String> {
    let raw = raw_message_of(&state, message_id)?;
    Ok(petrel_mime::unsubscribe_info(&raw).map(|u| UnsubInfo {
        one_click: u.one_click.is_some(),
        url: u.url,
        mailto: u.mailto,
    }))
}

/// Sends the RFC 8058 one-click POST for this message.
///
/// The URL is re-derived from the message's own bytes rather than accepted
/// from the caller: the message is the authority on where its list lives,
/// and a bridge that POSTs to whatever URL it is handed is a resource any
/// page in the webview would love to have.
#[tauri::command]
pub async fn unsubscribe_one_click(
    message_id: i64,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let raw = raw_message_of(&state, message_id)?;
    let url = petrel_mime::unsubscribe_info(&raw)
        .and_then(|u| u.one_click)
        .ok_or("this message does not offer one-click unsubscribe")?;
    tauri::async_runtime::spawn_blocking(move || post_one_click(&url))
        .await
        .map_err(|e| e.to_string())?
}

/// Why a one-click URL will not be posted to, if it will not be.
///
/// The URL comes out of a message, which means a stranger chose it, and this
/// runs from inside the person's network. `http://169.254.169.254/…` is a
/// cloud metadata service; `http://10.0.0.1/admin/reset` is somebody's
/// router. Neither is a mailing list, and neither should be reachable by
/// opening mail. The name is resolved here rather than trusted, because
/// `unsubscribe.example.com` can point wherever its owner likes.
fn refuse_reason(url: &str) -> Option<&'static str> {
    let Ok(parsed) = url.parse::<tauri::Url>() else {
        return Some("that unsubscribe link could not be read");
    };
    if parsed.scheme() != "https" {
        return Some("one-click unsubscribe must be https");
    }
    let Some(host) = parsed.host_str() else {
        return Some("that unsubscribe link names no host");
    };
    let port = parsed.port().unwrap_or(443);
    if resolves_inside(host, port) {
        return Some("that unsubscribe link points inside this network");
    }
    None
}

/// Whether a host lands anywhere on this machine or this network. A name
/// that resolves to nothing counts: there is nothing to post to, and it is
/// not worth finding out later.
fn resolves_inside(host: &str, port: u16) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

    fn v4_inside(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        ip.is_private()
            || ip.is_loopback()
            || ip.is_link_local()
            || ip.is_broadcast()
            || ip.is_documentation()
            || ip.is_unspecified()
            // 0.0.0.0/8, and the carrier-grade NAT range.
            || a == 0
            || (a == 100 && (64..128).contains(&b))
    }
    fn v6_inside(ip: Ipv6Addr) -> bool {
        if let Some(mapped) = ip.to_ipv4_mapped() {
            return v4_inside(mapped);
        }
        let first = ip.segments()[0];
        ip.is_loopback()
            || ip.is_unspecified()
            // Unique local (fc00::/7) and link local (fe80::/10).
            || (first & 0xfe00) == 0xfc00
            || (first & 0xffc0) == 0xfe80
    }

    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            let mut seen = false;
            for addr in addrs {
                seen = true;
                let inside = match addr.ip() {
                    IpAddr::V4(v4) => v4_inside(v4),
                    IpAddr::V6(v6) => v6_inside(v6),
                };
                // Any address inside is enough: a name that answers with one
                // public and one private address is the classic way round a
                // check that only looks at the first.
                if inside {
                    return true;
                }
            }
            !seen
        }
        Err(_) => true,
    }
}

/// The POST itself: the fixed form body RFC 8058 specifies, over https, to
/// somewhere outside, and with redirects refused.
///
/// A redirect is the other way to reach the router: the list's own host
/// answers 302 to `http://192.168.1.1/…` and a client that follows it has
/// made the request anyway. There is nothing an unsubscribe needs a redirect
/// for, so the answer is the answer.
fn post_one_click(url: &str) -> Result<(), String> {
    // The loopback the test posts to is exactly what the guard refuses, so
    // the guard is exercised in `refuse_reason` and skipped here.
    if !cfg!(test)
        && let Some(why) = refuse_reason(url)
    {
        return Err(why.into());
    }
    ureq::post(url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .max_redirects(0)
        .build()
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send("List-Unsubscribe=One-Click")
        .map(|_| ())
        .map_err(|e| format!("the sender's unsubscribe endpoint refused: {e}"))
}

#[cfg(test)]
mod unsubscribe_tests {
    use super::{post_one_click, refuse_reason};
    use std::io::{Read, Write};

    /// The link is a stranger's, and this runs inside the person's network.
    #[test]
    fn a_link_pointing_inside_this_network_is_refused() {
        for inside in [
            "https://127.0.0.1/unsub",
            "https://localhost/unsub",
            "https://[::1]/unsub",
            "https://10.0.0.1/unsub",
            "https://192.168.1.1/admin",
            "https://172.16.4.4/unsub",
            // The cloud metadata service, which is the whole reason this
            // check exists.
            "https://169.254.169.254/latest/meta-data/",
            "https://0.0.0.0/unsub",
        ] {
            assert!(refuse_reason(inside).is_some(), "{inside} was allowed");
        }
        // And the other ways round it.
        assert!(refuse_reason("http://lists.example.com/unsub").is_some());
        assert!(refuse_reason("ftp://lists.example.com/unsub").is_some());
        assert!(refuse_reason("not a url at all").is_some());
        assert!(refuse_reason("https://").is_some());
        // A name nobody can resolve is nothing to post to either.
        assert!(refuse_reason("https://petrel-no-such-host.invalid/unsub").is_some());
    }

    /// The exact bytes RFC 8058 asks for, proven against a listening socket.
    #[test]
    fn the_one_click_post_has_the_shape_the_rfc_specifies() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Headers and body can arrive in separate reads; keep reading
            // until the body has, or the peer stops.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = sock.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains("One-Click") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            req
        });
        post_one_click(&format!("http://127.0.0.1:{port}/unsub?u=42")).expect("post");
        let req = served.join().unwrap();
        assert!(req.starts_with("POST /unsub?u=42 HTTP/1.1"), "{req}");
        assert!(
            req.to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded"),
            "{req}"
        );
        assert!(req.ends_with("List-Unsubscribe=One-Click"), "{req}");
    }
}
