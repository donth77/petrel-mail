//! `Authentication-Results`, as servers actually write it.
//!
//! The header is loosely specified and every provider formats it differently,
//! so these are real shapes rather than the grammar's tidiest examples. The
//! failure that matters is not a parse error: it is answering "yes, this is
//! really from your bank" about a message that failed DMARC.

use petrel_mime::parse::{AuthVerdict, authentication};

fn msg(header: &str) -> Vec<u8> {
    format!(
        "From: a@example.com\r\nTo: me@example.com\r\nSubject: hi\r\n{header}\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <a1@x>\r\n\
         MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nbody\r\n"
    )
    .into_bytes()
}

#[test]
fn a_gmail_style_pass_is_read_whole() {
    let raw = msg("Authentication-Results: mx.google.com;\r\n\
         \x20      dkim=pass header.i=@example.com header.s=s1 header.b=abc123;\r\n\
         \x20      spf=pass (google.com: domain of a@example.com designates 1.2.3.4 as permitted sender) smtp.mailfrom=a@example.com;\r\n\
         \x20      dmarc=pass (p=REJECT sp=REJECT dis=NONE) header.from=example.com\r\n");
    let a = authentication(&raw).expect("header present");
    assert_eq!(a.spf, Some(AuthVerdict::Pass));
    assert_eq!(a.dkim, Some(AuthVerdict::Pass));
    assert_eq!(a.dmarc, Some(AuthVerdict::Pass));
    assert_eq!(a.domain.as_deref(), Some("example.com"));
    assert_eq!(a.authserv.as_deref(), Some("mx.google.com"));
    assert_eq!(a.identity_verified(), Some(true));
}

#[test]
fn a_dmarc_failure_is_reported_as_a_failure() {
    let raw = msg(
        "Authentication-Results: mx.example.net; spf=fail smtp.mailfrom=a@evil.example; \
         dkim=none; dmarc=fail header.from=bank.example\r\n",
    );
    let a = authentication(&raw).expect("header present");
    assert_eq!(a.spf, Some(AuthVerdict::Fail));
    assert_eq!(
        a.dkim,
        Some(AuthVerdict::Inconclusive),
        "none is not a fail"
    );
    assert_eq!(a.dmarc, Some(AuthVerdict::Fail));
    assert_eq!(a.identity_verified(), Some(false));
}

#[test]
fn softfail_counts_as_the_domain_disowning_it() {
    let raw = msg("Authentication-Results: mx.example.net; spf=softfail; dmarc=fail\r\n");
    let a = authentication(&raw).expect("header");
    assert_eq!(a.spf, Some(AuthVerdict::Fail));
}

#[test]
fn spf_and_dkim_alone_never_claim_the_sender_is_verified() {
    // The whole point of leaning on DMARC. Both of these pass for a domain
    // that need not be the one in the From line, so a message can be
    // dkim=pass and still be a forgery of somebody else.
    let raw = msg("Authentication-Results: mx.example.net; spf=pass; dkim=pass\r\n");
    let a = authentication(&raw).expect("header");
    assert_eq!(a.spf, Some(AuthVerdict::Pass));
    assert_eq!(a.dkim, Some(AuthVerdict::Pass));
    assert_eq!(a.dmarc, None);
    assert_eq!(
        a.identity_verified(),
        None,
        "without DMARC there is no claim to make about the From address"
    );
}

#[test]
fn no_header_says_nothing_rather_than_failing() {
    // Plenty of legitimate mail carries no Authentication-Results at all,
    // including anything delivered by a server that does not check. Absent
    // must never render as suspicious.
    let a = authentication(&msg(""));
    assert!(a.is_none());
}

#[test]
fn a_header_with_nothing_recognisable_is_the_same_as_none() {
    let a = authentication(&msg("Authentication-Results: mx.example.net; none\r\n"));
    assert!(a.is_none(), "no method reported is nothing to report");
}

#[test]
fn only_the_nearest_header_is_believed() {
    // Two hops. The first header is the one your own server wrote; the
    // second came with the message and can say whatever the sender liked.
    let raw = msg(
        "Authentication-Results: mx.mine.example; dmarc=fail header.from=bank.example\r\n\
         Authentication-Results: upstream.evil.example; dmarc=pass header.from=bank.example\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.authserv.as_deref(), Some("mx.mine.example"));
    assert_eq!(
        a.identity_verified(),
        Some(false),
        "an upstream pass must not overrule the local fail"
    );
}

#[test]
fn a_prefixed_token_is_not_mistaken_for_the_method() {
    // `header.dkim=` and `xdmarc=` both contain a method name. Matching them
    // would read a result out of the wrong field.
    let raw = msg(
        "Authentication-Results: mx.example.net; header.dkim=whatever; dmarc=pass header.from=example.com\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.dmarc, Some(AuthVerdict::Pass));
    assert_eq!(a.dkim, None, "header.dkim= is not a dkim result");
}
