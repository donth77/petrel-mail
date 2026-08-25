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

#[tauri::command]
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
#[tauri::command]
pub fn show_remote_once(message_id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    state
        .shown_once
        .lock()
        .map_err(|_| "lock poisoned")?
        .insert(message_id);
    Ok(())
}

/// Trusts this message's sender from now on.
#[tauri::command]
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

#[tauri::command]
pub fn trusted_senders(state: State<Arc<AppState>>) -> Result<Vec<String>, String> {
    let store = state.store()?;
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.trusted_senders(account).map_err(|e| e.to_string())
}

#[tauri::command]
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
#[tauri::command]
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

/// The POST itself: the fixed form body RFC 8058 specifies, over https only.
fn post_one_click(url: &str) -> Result<(), String> {
    if !url.to_ascii_lowercase().starts_with("https://") && !cfg!(test) {
        return Err("one-click unsubscribe must be https".into());
    }
    ureq::post(url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send("List-Unsubscribe=One-Click")
        .map(|_| ())
        .map_err(|e| format!("the sender's unsubscribe endpoint refused: {e}"))
}

#[cfg(test)]
mod unsubscribe_tests {
    use super::post_one_click;
    use std::io::{Read, Write};

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
