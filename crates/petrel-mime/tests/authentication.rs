//! `Authentication-Results`, as servers actually write it.
//!
//! The header is loosely specified and every provider formats it differently,
//! so these are real shapes rather than the grammar's tidiest examples. The
//! failure that matters is not a parse error: it is answering "yes, this is
//! really from your bank" about a message that failed DMARC.

use petrel_mime::parse::{AuthVerdict, authentication, authserv_trusted};

fn msg(header: &str) -> Vec<u8> {
    from("a@example.com", header)
}

/// `msg` from someone else: DMARC's verdict is about the From domain, and one
/// naming another domain is not shown.
fn from(sender: &str, header: &str) -> Vec<u8> {
    format!(
        "From: {sender}\r\nTo: me@example.com\r\nSubject: hi\r\n{header}\
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
    let raw = from(
        "Bank <alerts@bank.example>",
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
    let raw = from(
        "alerts@bank.example",
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

/// The forgery the audit found. Gmail copies the envelope sender into the SPF
/// comment and into `smtp.mailfrom=`, both ahead of the DMARC result, and `+`
/// and `=` are legal in an address. Searching the header for the first
/// `dmarc=` read the attacker's `+dmarc=pass` as the verdict, and the first
/// `header.from=` as the real, failing domain: "Verified — bank.example".
#[test]
fn a_verdict_written_into_the_envelope_sender_is_not_the_verdict() {
    let raw = from(
        "Bank <alerts@bank.example>",
        "Authentication-Results: mx.google.com;\r\n\
         \x20      dkim=none;\r\n\
         \x20      spf=pass (google.com: domain of x+dmarc=pass@attacker.example designates 1.2.3.4 as permitted sender) smtp.mailfrom=x+dmarc=pass@attacker.example;\r\n\
         \x20      dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=bank.example\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.dmarc, Some(AuthVerdict::Fail));
    assert_eq!(a.domain.as_deref(), Some("bank.example"));
    assert_eq!(a.identity_verified(), Some(false), "a forgery, said as one");
}

/// `header.from` belongs to DMARC's result. Read from anywhere, a property
/// the sender could place in another result chose the domain shown.
#[test]
fn the_domain_is_read_from_the_dmarc_result_alone() {
    let raw = from(
        "alerts@bank.example",
        "Authentication-Results: mx.google.com; spf=pass smtp.mailfrom=b@attacker.example \
         header.from=bank.example; dmarc=pass header.from=attacker.example\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.domain.as_deref(), Some("attacker.example"));
    assert_eq!(
        a.identity_verified(),
        None,
        "a pass for attacker.example says nothing about mail from bank.example"
    );
}

/// DMARC is about the From domain. A verdict naming another one is about
/// other mail, and earns neither the tick nor the warning.
#[test]
fn a_verdict_for_another_domain_is_not_shown() {
    let pass = from(
        "alerts@bank.example",
        "Authentication-Results: mx.example.net; dmarc=pass header.from=attacker.example\r\n",
    );
    assert_eq!(authentication(&pass).unwrap().identity_verified(), None);
    let fail = from(
        "alerts@bank.example",
        "Authentication-Results: mx.example.net; dmarc=fail header.from=other.example\r\n",
    );
    assert_eq!(authentication(&fail).unwrap().identity_verified(), None);
}

/// Without `header.from`, DMARC's verdict is still about the From domain.
#[test]
fn a_dmarc_result_that_names_no_domain_is_about_the_from_domain() {
    let raw = from(
        "News <news@Example.COM>",
        "Authentication-Results: mx.example.net; dmarc=pass\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.domain.as_deref(), Some("example.com"));
    assert_eq!(a.identity_verified(), Some(true));
}

/// Microsoft writes no authserv-id: the header opens with a result.
#[test]
fn a_header_that_names_no_server_is_read_from_its_first_result() {
    let raw = msg(
        "Authentication-Results: spf=pass (sender IP is 1.2.3.4)\r\n smtp.mailfrom=example.com; \
         dkim=pass (signature was verified)\r\n header.d=example.com;dmarc=pass action=none\r\n \
         header.from=example.com;compauth=pass reason=100\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.authserv, None);
    assert_eq!(a.spf, Some(AuthVerdict::Pass));
    assert_eq!(a.dkim, Some(AuthVerdict::Pass));
    assert_eq!(a.dmarc, Some(AuthVerdict::Pass));
    assert_eq!(a.identity_verified(), Some(true));
}

/// The grammar's latitude: space around `=`, quoted values, nested comments
/// and a `;` inside one, none of which is a new result.
#[test]
fn spacing_quotes_and_nested_comments_are_read_as_the_grammar_says() {
    let raw = msg(
        "Authentication-Results: \"mx.example.net\" 1; spf = pass (a (nested; comment) here) \
         smtp.mailfrom=a@example.com; dmarc = pass header.from = \"example.com\"\r\n",
    );
    let a = authentication(&raw).expect("header");
    assert_eq!(a.authserv.as_deref(), Some("mx.example.net"));
    assert_eq!(a.spf, Some(AuthVerdict::Pass));
    assert_eq!(a.dmarc, Some(AuthVerdict::Pass));
    assert_eq!(a.identity_verified(), Some(true));
}

/// Whose verdict is believed: the account's own provider's, by the domain it
/// stamps under, and nobody else's.
#[test]
fn only_the_accounts_own_provider_is_believed() {
    for (authserv, host) in [
        (Some("mx.google.com"), "imap.gmail.com"),
        (Some("mx.smallhost.example"), "imap.smallhost.example"),
        (Some("smallhost.example"), "mail.smallhost.example"),
        (Some("MX.SmallHost.Example."), "imap.smallhost.example"),
        (Some("dmarc.icloud.com"), "imap.mail.me.com"),
        (Some("mx6.messagingengine.com"), "imap.fastmail.com"),
        (
            Some("atlas-production.v2.mail.yahoo.com"),
            "imap.mail.yahoo.com",
        ),
        (Some("atlas-production.v2.mail.yahoo.com"), "imap.aol.com"),
        (Some("mx.zohomail.com"), "imap.zoho.com"),
        (Some("mx.provider.co.uk"), "imap.provider.co.uk"),
        (None, "outlook.office365.com"),
        (None, "imap-mail.outlook.com"),
    ] {
        assert!(authserv_trusted(authserv, host), "{authserv:?} for {host}");
    }
    for (authserv, host) in [
        // A stranger's name on a provider that wrote nothing of its own.
        (Some("mx.google.com"), "imap.smallhost.example"),
        (Some("mx.example.net"), "imap.gmail.com"),
        (Some("mx.other.co.uk"), "imap.provider.co.uk"),
        // Only Microsoft names no server.
        (None, "imap.gmail.com"),
        (None, "imap.smallhost.example"),
        // Nothing to compare with.
        (Some("mx.google.com"), "127.0.0.1"),
        (Some("localhost"), "localhost"),
        (Some(""), "imap.smallhost.example"),
        (
            Some("smallhost.example; dmarc=pass"),
            "imap.smallhost.example",
        ),
    ] {
        assert!(!authserv_trusted(authserv, host), "{authserv:?} for {host}");
    }
}

/// A quoted envelope sender a server copies into its SPF comment unescaped:
/// its `)` must not end the comment, nor its `(` swallow the header, or the
/// sender's text is read as results.
#[test]
fn brackets_inside_a_quoted_sender_do_not_end_the_comment() {
    for sender in ["\"x)y; dmarc=pass header.from=bank.example; z\"", "\"x(y\""] {
        let raw = from(
            "alerts@bank.example",
            &format!(
                "Authentication-Results: mx.google.com; spf=pass (google.com: domain of \
                 {sender}@attacker.example designates 1.2.3.4 as permitted sender) \
                 smtp.mailfrom={sender}@attacker.example; dmarc=fail (p=NONE) \
                 header.from=bank.example\r\n"
            ),
        );
        let a = authentication(&raw).expect("header");
        assert_eq!(a.dmarc, Some(AuthVerdict::Fail), "{sender}");
        assert_eq!(a.identity_verified(), Some(false), "{sender}");
    }
}

/// A message has one DMARC evaluation. Two in one header means one of them
/// is text somebody put there, and nothing says which: neither is shown.
#[test]
fn two_dmarc_results_are_believed_by_neither() {
    let raw = from(
        "alerts@bank.example",
        "Authentication-Results: mx.example.net; dmarc=pass header.from=bank.example; \
         spf=pass; dmarc=fail header.from=bank.example\r\n",
    );
    let a = authentication(&raw).expect("spf is still reported");
    assert_eq!(a.dmarc, None);
    assert_eq!(a.domain, None);
    assert_eq!(a.identity_verified(), None);
}
