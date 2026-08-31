//! The sign-in flow's arithmetic, without a network.
//!
//! Every failure here is one that would otherwise surface as "sign-in was
//! refused" against a real provider, with nothing to distinguish a wrong hash
//! from a wrong password. That is why so much of this is exactness: the
//! challenge is a specific transform of the verifier, the state check is the
//! whole of the flow's CSRF defence, and a token response can carry a refusal
//! in a body that arrives looking like success.

use base64::Engine as _;
use petrel_providers::oauth::{
    Pkce, Provider, authorize_url, code_exchange_form, code_from_redirect, parse_tokens,
    refresh_form,
};
use sha2::{Digest, Sha256};

#[test]
fn the_challenge_matches_rfc_7636s_own_worked_example() {
    // Appendix B of RFC 7636 publishes a verifier and the challenge it must
    // produce. Anything that fails this is wrong however plausible it looks,
    // and it is the one part of the flow with a published right answer.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
fn a_generated_challenge_is_the_hash_of_its_own_verifier() {
    let p = Pkce::new();
    let digest = Sha256::digest(p.verifier.as_bytes());
    assert_eq!(
        p.challenge,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    );
}

#[test]
fn verifiers_are_long_enough_unpadded_and_never_repeat() {
    let a = Pkce::new();
    let b = Pkce::new();
    // RFC 7636 wants 43 to 128 characters from the unreserved set.
    assert!(
        (43..=128).contains(&a.verifier.len()),
        "{}",
        a.verifier.len()
    );
    assert!(
        a.verifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~')),
        "a verifier carried a character that has to be escaped: {}",
        a.verifier
    );
    assert!(
        !a.verifier.contains('='),
        "padding would have to be escaped"
    );
    assert_ne!(a.verifier, b.verifier, "two sign-ins shared a secret");
    assert_ne!(a.state, b.state, "two sign-ins shared a state");
    assert_ne!(a.verifier, a.state, "the state was the verifier");
}

#[test]
fn the_authorize_url_asks_for_mail_and_a_refresh_token() {
    let pkce = Pkce::new();
    let url = authorize_url(
        Provider::Microsoft,
        "abc-123",
        "http://localhost:5599",
        &pkce,
    );

    assert!(url.starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
    // Without offline_access there is no refresh token, and the account stops
    // working an hour after it was set up.
    assert!(url.contains("offline_access"), "{url}");
    assert!(url.contains("IMAP.AccessAsUser.All"), "{url}");
    assert!(url.contains("SMTP.Send"), "{url}");
    // The verifier is the secret. It must never be in a URL.
    assert!(
        !url.contains(&pkce.verifier),
        "the verifier leaked into the URL"
    );
}

#[test]
fn the_redirect_and_scopes_are_escaped() {
    let pkce = Pkce::new();
    let url = authorize_url(Provider::Microsoft, "abc", "http://localhost:5599", &pkce);
    assert!(
        url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5599"),
        "{url}"
    );
    // A raw space in the scope list would end the URL at the first one.
    let scopes = url.split("scope=").nth(1).unwrap_or("");
    assert!(
        !scopes.split('&').next().unwrap_or("").contains(' '),
        "{url}"
    );
}

#[test]
fn google_is_asked_for_offline_access_its_own_way() {
    let url = authorize_url(Provider::Google, "abc", "http://localhost:1", &Pkce::new());
    // Google returns a refresh token only when asked, and only once unless
    // consent is forced.
    assert!(url.contains("access_type=offline"), "{url}");
    assert!(url.contains("prompt=consent"), "{url}");
}

#[test]
fn the_exchange_sends_the_verifier_and_the_refresh_does_not() {
    let exchange = code_exchange_form("cid", "http://localhost:1", "the-code", "the-verifier");
    let field = |k: &str| {
        exchange
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(field("grant_type").as_deref(), Some("authorization_code"));
    assert_eq!(field("code_verifier").as_deref(), Some("the-verifier"));
    // No client secret: a desktop app cannot keep one, and PKCE is what
    // stands in its place. Sending an empty one is worse than sending none.
    assert!(field("client_secret").is_none());

    let refresh = refresh_form("cid", "the-refresh");
    let rfield = |k: &str| {
        refresh
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(rfield("grant_type").as_deref(), Some("refresh_token"));
    assert_eq!(rfield("refresh_token").as_deref(), Some("the-refresh"));
    assert!(rfield("code_verifier").is_none(), "the verifier is spent");
}

#[test]
fn a_token_response_is_read_whole() {
    let t = parse_tokens(
        r#"{"access_token":"AT","refresh_token":"RT","expires_in":3599,"token_type":"Bearer"}"#,
    )
    .unwrap();
    assert_eq!(t.access_token, "AT");
    assert_eq!(t.refresh_token.as_deref(), Some("RT"));
    assert_eq!(t.expires_in, 3599);
}

#[test]
fn a_refresh_that_returns_no_new_refresh_token_is_not_a_failure() {
    // Providers that are happy with the one you have simply do not send
    // another. Treating that as an error would sign somebody out every time.
    let t = parse_tokens(r#"{"access_token":"AT2","expires_in":3599}"#).unwrap();
    assert_eq!(t.refresh_token, None);
    assert_eq!(t.access_token, "AT2");
}

#[test]
fn a_refusal_arrives_as_a_body_and_says_which_refusal() {
    // These are different problems with different answers, and collapsing them
    // into "sign-in failed" would send somebody to check a password when the
    // real answer is that an administrator has to approve the app.
    let e = parse_tokens(r#"{"error":"invalid_grant","error_description":"AADSTS70000: expired"}"#)
        .unwrap_err();
    assert!(e.contains("invalid_grant"), "{e}");
    assert!(e.contains("expired"), "{e}");

    let e = parse_tokens(r#"{"error":"consent_required"}"#).unwrap_err();
    assert_eq!(e, "consent_required");

    // And a body with neither a token nor an error is refused rather than
    // silently producing an empty credential.
    assert!(parse_tokens(r#"{"token_type":"Bearer"}"#).is_err());
    assert!(parse_tokens("not json at all").is_err());
}

#[test]
fn the_redirect_is_only_accepted_with_the_state_it_went_out_with() {
    assert_eq!(
        code_from_redirect("?code=THE-CODE&state=abc", "abc").unwrap(),
        "THE-CODE"
    );
    // The whole of this flow's CSRF defence: a browser arriving with somebody
    // else's authorization code must not be believed.
    let e = code_from_redirect("?code=THE-CODE&state=somebody-else", "abc").unwrap_err();
    assert!(e.contains("state"), "{e}");
    assert!(
        code_from_redirect("?code=THE-CODE", "abc").is_err(),
        "no state at all"
    );
    assert!(
        code_from_redirect("?state=abc", "abc").is_err(),
        "no code at all"
    );
}

#[test]
fn a_refused_sign_in_comes_back_through_the_redirect_too() {
    // Pressing Cancel on the consent screen is a redirect, not a timeout.
    let e = code_from_redirect(
        "?error=access_denied&error_description=The+user+cancelled&state=abc",
        "abc",
    )
    .unwrap_err();
    assert!(e.contains("access_denied"), "{e}");
    assert!(
        e.contains("The user cancelled"),
        "the + was not decoded: {e}"
    );
}

#[test]
fn a_percent_encoded_code_survives_the_trip_back() {
    // Authorization codes routinely carry characters that get escaped.
    let code = code_from_redirect("?code=a%2Fb%2Bc%3Dd&state=s", "s").unwrap();
    assert_eq!(code, "a/b+c=d");
}
