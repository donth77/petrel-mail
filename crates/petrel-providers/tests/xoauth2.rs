//! The credential string OAuth sign-in puts on the wire.
//!
//! Exact, and easy to get subtly wrong: the separators are single 0x01 bytes,
//! there are two of them at the end rather than one, and the whole thing is
//! base64 with no line breaks. Every server that implements XOAUTH2 refuses a
//! near miss, and the refusal it sends back reads like a bad password — so a
//! typo here would look like an account problem for as long as it took
//! somebody to packet-capture the exchange.
//!
//! Checked against the shape Google and Microsoft both document.

use base64::Engine as _;
use petrel_providers::imap::xoauth2_payload;

#[test]
fn the_payload_is_exactly_what_the_mechanism_specifies() {
    let p = xoauth2_payload("tom@outlook.com", "ya29.TOKEN");
    assert_eq!(
        p,
        "user=tom@outlook.com\u{1}auth=Bearer ya29.TOKEN\u{1}\u{1}"
    );
}

#[test]
fn the_separators_are_single_bytes_not_the_two_characters_that_look_like_them() {
    let p = xoauth2_payload("a@b.com", "T");
    let bytes = p.as_bytes();
    assert_eq!(bytes.iter().filter(|b| **b == 0x01).count(), 3);
    // No literal backslash-x-zero-one anywhere, which is what an escaped
    // string in the wrong quoting would leave behind.
    assert!(
        !p.contains("\\x01"),
        "escapes survived into the payload: {p}"
    );
}

#[test]
fn it_ends_with_two_separators_and_nothing_else() {
    let p = xoauth2_payload("a@b.com", "T");
    assert!(p.ends_with("\u{1}\u{1}"), "{p:?}");
    assert!(!p.ends_with("\u{1}\u{1}\u{1}"), "one too many: {p:?}");
}

#[test]
fn base64_of_it_round_trips() {
    // What actually goes after "AUTH XOAUTH2 " / "AUTHENTICATE XOAUTH2 ".
    let p = xoauth2_payload("tom@outlook.com", "abc123");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&p);
    assert!(
        !encoded.contains('\n'),
        "line breaks would end the command early"
    );
    let back = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap();
    assert_eq!(String::from_utf8(back).unwrap(), p);
}

#[test]
fn a_token_with_awkward_characters_survives() {
    // Real tokens are long and carry dots, dashes and underscores. None of
    // them need escaping, and treating them as if they did would corrupt one.
    let token = "eyJ0eX-A.iOiJKV1_QiLCJhbGciOiJSUzI1NiJ9";
    let p = xoauth2_payload("a@b.com", token);
    assert!(p.contains(token), "the token was altered: {p}");
}
