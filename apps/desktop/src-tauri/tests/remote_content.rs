//! The remote-content setting has to reach the renderer.
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
        allow_remote,
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

#[test]
fn blocking_removes_the_image_and_refuses_it_in_the_policy() {
    let (html, csp) = render(false);
    assert!(
        !html.contains("tracker.example"),
        "remote src survived: {html}"
    );
    assert!(
        !csp.contains("https:"),
        "policy still permits remote images: {csp}"
    );
}

#[test]
fn allowing_lets_the_image_through_both_layers() {
    let (html, csp) = render(true);
    assert!(
        html.contains("tracker.example"),
        "the setting did not reach the sanitizer: {html}"
    );
    assert!(
        csp.contains("https:"),
        "the setting did not reach the policy: {csp}"
    );
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
