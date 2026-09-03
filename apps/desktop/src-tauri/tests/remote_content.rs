//! The remote-content decision has to reach the renderer.
//!
//! A privacy control that saves a preference and changes nothing is worse than
//! no control at all: it is a promise the app does not keep, made in the one
//! pane people open precisely because they care whether it is kept.

use std::sync::Arc;

use petrel_desktop::message_view::{ViewTokens, handle};
use petrel_engine::blob::BlobStore;

fn message_with_remote_image() -> Vec<u8> {
    b"From: sam@example.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n\
      <p>Hello</p><img src=\"https://tracker.example/pixel.gif\">"
        .to_vec()
}

fn render(allow_remote: bool) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let blobs = BlobStore::open(dir.path()).unwrap();
    let (hash, _) = blobs.write(&message_with_remote_image()).unwrap();

    let tokens = Arc::new(ViewTokens::new());
    let token = tokens.issue(1);
    let request = http::Request::builder()
        .uri(format!("petrel-msg://localhost/message/{token}"))
        .body(Vec::new())
        .unwrap();

    let response = handle(
        &request,
        &tokens,
        &blobs,
        |_| Some(hash.clone()),
        // The renderer now asks per message rather than being told once, so the
        // test supplies the same answer the policy would.
        |_| allow_remote,
    );
    let csp = response
        .headers()
        .get("Content-Security-Policy")
        .expect("every response carries a policy")
        .to_str()
        .unwrap()
        .to_string();
    (String::from_utf8_lossy(response.body()).into_owned(), csp)
}

/// The count of refused resources leaves the frame, because the offer to undo
/// the refusal cannot live inside it — a sandboxed frame with no IPC can state
/// what happened but can never act on it. If this stops being reported, the
/// banner silently never appears and blocking becomes indistinguishable from
/// a message that simply had no images.
#[test]
fn the_blocked_count_is_reported_out_of_the_frame() {
    let (html, _) = render(false);
    assert!(
        html.contains("var BLOCKED = 1") && html.contains("petrelBlocked"),
        "the frame must tell the app what it refused: {html}"
    );
    let (allowed, _) = render(true);
    assert!(
        allowed.contains("var BLOCKED = 0") && allowed.contains("petrelBlocked"),
        "nothing was refused, and the banner must not appear: {allowed}"
    );
}

/// One directive out of a policy.
///
/// Asked for by name rather than searched for as a substring: the policy
/// now names the app window's own `https://tauri.localhost` in
/// `frame-ancestors`, and a search over the whole string reads that as
/// permission to load pictures from the web.
fn directive(csp: &str, name: &str) -> String {
    csp.split(';')
        .map(str::trim)
        .find(|d| d.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("no {name} directive in {csp}"))
        .to_string()
}

#[test]
fn blocking_removes_the_image_and_refuses_it_in_the_policy() {
    let (html, csp) = render(false);
    assert!(
        !html.contains("tracker.example"),
        "remote src survived: {html}"
    );
    let img = directive(&csp, "img-src");
    assert!(
        !img.split_whitespace()
            .any(|s| matches!(s, "https:" | "http:" | "*")),
        "policy still permits remote images: {img}"
    );
}

#[test]
fn allowing_lets_the_image_through_both_layers() {
    let (html, csp) = render(true);
    assert!(
        html.contains("tracker.example"),
        "the setting did not reach the sanitizer: {html}"
    );
    let img = directive(&csp, "img-src");
    assert!(
        img.split_whitespace().any(|s| s == "https:"),
        "the setting did not reach the policy: {img}"
    );
}

/// Windows: the reading pane was blank, and one reason was this directive.
/// `frame-ancestors 'self'` is the *frame's* origin — no app window is ever
/// that — so Chromium refused to display the message at all. The embedder
/// is named outright now, in every spelling the platforms use.
#[test]
fn the_app_window_is_allowed_to_show_the_frame_on_every_platform() {
    for allow in [false, true] {
        let (_, csp) = render(allow);
        let ancestors = directive(&csp, "frame-ancestors");
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(
                ancestors.split_whitespace().any(|s| s == origin),
                "{origin} cannot embed the reading pane: {ancestors}"
            );
        }
        assert!(!ancestors.contains("'self'"), "{ancestors}");
    }
}

/// Whatever the setting, the directives that matter never move. Allowing
/// pictures must not quietly allow anything else.
#[test]
fn allowing_remote_content_widens_nothing_else() {
    for allow in [false, true] {
        let (_, csp) = render(allow);
        assert!(csp.contains("default-src 'none'"), "{csp}");
        assert!(csp.contains("form-action 'none'"), "{csp}");
        assert!(csp.contains("base-uri 'none'"), "{csp}");
        assert!(csp.contains("script-src 'nonce-"), "{csp}");
        assert!(!csp.contains("script-src 'unsafe-inline'"), "{csp}");
    }
}
