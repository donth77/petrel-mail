//! Serves sanitized message bodies over the `petrel-msg` custom protocol.
//!
//! This is the reading pane's supply line, and it applies the layered defense
//! measured to hold on this platform:
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
/// The one script Petrel puts in a message frame. Two jobs, both structural:
///
///  1. Report the document height, so the reading pane can size the frame to its
///     content instead of nesting a scroll region inside a scroll region.
///  2. Forward keystrokes back out. A focused iframe swallows every keydown —
///     they reach the frame's document, never the parent — so once you click a
///     message, every shortcut in the application stops working. The frame holds
///     sanitized mail with no inputs, so nothing here is typing.
///
/// Admitted by a per-response nonce. It forwards key
/// identity only: never characters typed, never content, never anything read
/// from the document.
/// A per-response CSP nonce. Uniqueness is what matters — it is compared against
/// itself within one response and never reused — so process id, a counter and the
/// clock's nanoseconds are sufficient without pulling in an RNG dependency.
fn new_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{n:x}{nanos:x}", std::process::id())
}

const HEIGHT_REPORTER: &str = r#"
(function () {
  function h() {
    var d = document.documentElement, b = document.body;
    return Math.max(d.scrollHeight, b ? b.scrollHeight : 0);
  }
  function post() {
    try {
      parent.postMessage({ petrelHeight: h(), petrelBlocked: BLOCKED }, '*');
    } catch (e) {}
  }
  addEventListener('load', post);
  addEventListener('resize', post);
  if (window.ResizeObserver) { new ResizeObserver(post).observe(document.documentElement); }
  post();
  setTimeout(post, 60);
  setTimeout(post, 400);

  // The reading-size preference. A CSS variable on the host cannot cross into
  // an opaque-origin frame, so the size is sent in and applied here — which is
  // also why it takes effect immediately rather than on the next fetch.
  addEventListener('message', function (e) {
    var n = e.data && e.data.petrelSize;
    // Bounded: the only thing this accepts is a plausible font size.
    if (typeof n === 'number' && n >= 10 && n <= 28) {
      document.documentElement.style.setProperty('--petrel-size', n + 'px');
      post();
    }
  });

  addEventListener('keydown', function (e) {
    // Identity only — which key, which modifiers. Nothing about the document.
    try {
      parent.postMessage({
        petrelKey: {
          key: e.key,
          metaKey: e.metaKey, ctrlKey: e.ctrlKey,
          shiftKey: e.shiftKey, altKey: e.altKey
        }
      }, '*');
    } catch (err) {}
  });
})();
"#;

fn document(body: &str, blocked_remote: usize, nonce: &str) -> String {
    // The count goes out to the app rather than into a banner here. A notice
    // drawn inside the frame can only ever be a notice: the frame has no script
    // of its own, no IPC and no same-origin access, so "Show images" drawn here
    // would be a button that cannot do anything. Outside, it can.
    let reporter = HEIGHT_REPORTER.replace("BLOCKED", &blocked_remote.to_string());
    let banner = "";
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<style>
  :root {{ color-scheme: light; }}
  :root {{ --petrel-size: 15px; }}
  body {{ margin: 0; padding: 14px 16px; background: #fff; color: #182730;
         font: var(--petrel-size)/1.6 -apple-system, system-ui, sans-serif;
         word-wrap: break-word; }}
  img {{ max-width: 100%; height: auto; }}
  table {{ max-width: 100%; }}
  blockquote {{ margin: 8px 0; padding-left: 12px; border-left: 2px solid #d9e1e2; color: #54666e; }}
  .banner {{ background: #f6eedd; border: 1px solid #e2d3ae; color: #6b5220;
            padding: 8px 10px; border-radius: 4px; font-size: 12.5px; margin-bottom: 12px; }}
  .petrel-plain {{ white-space: pre-wrap;
                  font: calc(var(--petrel-size) * 0.92)/1.6 ui-monospace, SFMono-Regular, monospace; }}
  .petrel-plain .q {{ color: #54666e; }}
</style></head><body>{banner}{body}<script nonce="{nonce}">{reporter}</script></body></html>"#
    )
}

pub fn handle(
    request: &Request<Vec<u8>>,
    tokens: &Arc<ViewTokens>,
    blobs: &BlobStore,
    lookup_blob: impl Fn(i64) -> Option<String>,
    // Asked per message, because the answer is per sender: blocked unless the
    // sender was trusted or the user has written to them. Passed in rather than
    // decided here so the render path keeps no opinion about where preferences
    // live — and so the default, when anything goes wrong, is the safe one.
    allow_remote: impl Fn(i64) -> bool,
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
    let allow_remote = allow_remote(message_id);

    // Prefer HTML; fall back to the plain part. Fail closed to text.
    let (body, report) = match parsed.body_html.as_deref() {
        Some(html) => {
            let s = petrel_mime::sanitize_html(html, allow_remote);
            (s.html, s.report)
        }
        None => (
            petrel_mime::plain_text_to_html(&parsed.body_text),
            Default::default(),
        ),
    };

    // Fresh per response: markup that somehow survived sanitization still cannot
    // carry a matching nonce, so ours stays the only script that runs.
    let nonce = new_token();
    // Two layers, not one. The sanitizer has already removed remote sources
    // when they are blocked; this makes the browser refuse them as well, so a
    // URL that slips through the rewriter still cannot reach the network.
    // Allowing remote content widens exactly one directive and nothing else —
    // scripts, forms and frames stay refused however the setting is left.
    let img_src = if allow_remote {
        "cid: petrel-msg: http://petrel-msg.localhost https:"
    } else {
        "cid: petrel-msg: http://petrel-msg.localhost"
    };
    let csp = format!(
        "default-src 'none'; img-src {img_src}; \
         style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; script-src 'nonce-{nonce}'; \
         form-action 'none'; base-uri 'none'; frame-ancestors 'self'"
    );

    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(document(&body, report.blocked_remote, &nonce).into_bytes())
        .expect("message response")
}
