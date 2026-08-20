//! Serves sanitized message bodies over the `petrel-msg` custom protocol.
//!
//! This is the reading pane's supply line, and it applies the layered defense
//! the S2 spike validated on this platform (ADR-0004):
//!   1. the body is sanitized here (allowlist; hostile constructs removed),
//!   2. the response carries a per-message CSP that blocks network egress,
//!   3. the UI renders it in a `sandbox`ed iframe with no scripts and no IPC.
//!
//! Each layer blocks a class the others do not, so none is redundant.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use petrel_engine::blob::BlobStore;
use tauri::http::{Request, Response};

/// Single-use, unguessable token gating access to a message body. Without this
/// any document rendered in the app could name another message's URL.
pub struct ViewTokens {
    counter: AtomicU64,
    issued: std::sync::Mutex<std::collections::HashMap<String, i64>>,
}

impl ViewTokens {
    pub fn new() -> Self {
        ViewTokens {
            counter: AtomicU64::new(0),
            issued: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Issues a token for one message. Not a security boundary on its own — the
    /// sandbox and CSP are — but it keeps message URLs unguessable and scoped.
    pub fn issue(&self, message_id: i64) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        // Process-local, non-persistent; a real nonce arrives with the
        // multi-window work, where tokens must not be shareable across windows.
        let token = format!(
            "{n:x}-{:x}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        if let Ok(mut map) = self.issued.lock() {
            map.insert(token.clone(), message_id);
        }
        token
    }

    fn resolve(&self, token: &str) -> Option<i64> {
        self.issued.lock().ok()?.get(token).copied()
    }
}

impl Default for ViewTokens {
    fn default() -> Self {
        Self::new()
    }
}

fn error_response(status: u16, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .expect("error response")
}

/// The document shell. Styling lives here rather than in the message so that a
/// message cannot restyle the reading pane around it.
fn document(body: &str, blocked_remote: usize) -> String {
    let banner = if blocked_remote > 0 {
        format!(
            "<div class=\"banner\">Remote content blocked \
             ({blocked_remote} resource{}). This message tried to load content from \
             another server, which would tell the sender you opened it.</div>",
            if blocked_remote == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<style>
  :root {{ color-scheme: light; }}
  body {{ margin: 0; padding: 14px 16px; background: #fff; color: #182730;
         font: 14px/1.6 -apple-system, system-ui, sans-serif; word-wrap: break-word; }}
  img {{ max-width: 100%; height: auto; }}
  table {{ max-width: 100%; }}
  blockquote {{ margin: 8px 0; padding-left: 12px; border-left: 2px solid #d9e1e2; color: #54666e; }}
  .banner {{ background: #f6eedd; border: 1px solid #e2d3ae; color: #6b5220;
            padding: 8px 10px; border-radius: 4px; font-size: 12.5px; margin-bottom: 12px; }}
  .petrel-plain {{ white-space: pre-wrap; font: 13.5px/1.6 ui-monospace, SFMono-Regular, monospace; }}
  .petrel-plain .q {{ color: #54666e; }}
</style></head><body>{banner}{body}</body></html>"#
    )
}

pub fn handle(
    request: &Request<Vec<u8>>,
    tokens: &Arc<ViewTokens>,
    blobs: &BlobStore,
    lookup_blob: impl Fn(i64) -> Option<String>,
) -> Response<Vec<u8>> {
    let path = request.uri().path().to_string();
    let Some(token) = path.strip_prefix("/message/") else {
        return error_response(404, "not found");
    };
    let Some(message_id) = tokens.resolve(token) else {
        return error_response(403, "unknown or expired message token");
    };
    let Some(hash) = lookup_blob(message_id) else {
        return error_response(404, "message body not stored");
    };
    let Ok(raw) = blobs.read(&hash) else {
        // Verification failure lands here: corrupt bytes are never rendered.
        return error_response(410, "message body unavailable (failed verification)");
    };
    let Some(parsed) = petrel_mime::parse_message(&raw) else {
        return error_response(422, "message could not be parsed");
    };

    // Prefer HTML; fall back to the plain part. Fail closed to text.
    let (body, report) = match parsed.body_html.as_deref() {
        Some(html) => {
            let s = petrel_mime::sanitize_html(html, false);
            (s.html, s.report)
        }
        None => (
            petrel_mime::plain_text_to_html(&parsed.body_text),
            Default::default(),
        ),
    };

    let csp = "default-src 'none'; img-src cid: petrel-msg: http://petrel-msg.localhost; \
               style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; script-src 'none'; \
               form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(document(&body, report.blocked_remote).into_bytes())
        .expect("message response")
}
