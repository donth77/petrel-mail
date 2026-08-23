//! The chain against the real network. Ignored by default so CI stays
//! hermetic; run with `cargo test -p petrel-autoconfig -- --ignored`.

use petrel_autoconfig::{Via, discover};

#[tokio::test]
#[ignore]
async fn a_known_provider_answers_without_the_network() {
    let d = discover("someone@gmail.com").await.unwrap().unwrap();
    assert_eq!(d.via, Via::KnownProvider);
    assert_eq!(d.imap.host, "imap.gmail.com");
}

#[tokio::test]
#[ignore]
async fn the_ispdb_answers_for_a_catalogued_provider() {
    // A provider not in the short table, so the ISPDB has to answer.
    let d = discover("someone@posteo.de").await.unwrap().unwrap();
    assert_eq!(d.via, Via::Ispdb, "got {d:?}");
    assert!(d.imap.host.contains("posteo"), "got {d:?}");
    assert!(d.imap.tls && d.smtp.tls);
}

#[tokio::test]
#[ignore]
async fn a_custom_domain_on_namecheap_is_found_by_its_mx() {
    // privateemail.com itself is a Namecheap-hosted domain and not in the
    // ISPDB, so this exercises the MX step end to end against real DNS.
    let d = discover("someone@privateemail.com").await.unwrap().unwrap();
    assert_eq!(d.via, Via::Mx, "got {d:?}");
    assert_eq!(d.provider, "Namecheap Private Email");
    assert_eq!(d.imap.host, "mail.privateemail.com");
}

#[tokio::test]
#[ignore]
async fn an_unknown_domain_means_the_manual_form() {
    let d = discover("someone@example.invalid").await.unwrap();
    assert!(d.is_none());
}

/// A real custom domain on Namecheap, from the environment so no address is
/// written into the repository. Set PETREL_NC_USER (see .env.namecheap);
/// skipped silently when it is not.
#[tokio::test]
#[ignore]
async fn a_real_namecheap_domain_is_found_from_the_address_alone() {
    let Ok(addr) = std::env::var("PETREL_NC_USER") else {
        eprintln!("PETREL_NC_USER not set; skipping");
        return;
    };
    let d = discover(&addr)
        .await
        .unwrap()
        .expect("should be discovered");
    assert_eq!(d.via, Via::Mx, "a custom domain can only be found by MX");
    assert_eq!(d.provider, "Namecheap Private Email");
    assert_eq!(d.imap.host, "mail.privateemail.com");
    assert_eq!(d.smtp.host, "mail.privateemail.com");
}

/// The extended MX table against real DNS: each provider's own domain
/// resolves, by MX, to the right servers. Their own domains are used
/// because a customer's custom domain is not ours to name.
#[tokio::test]
#[ignore]
async fn the_custom_domain_hosts_resolve_by_real_dns() {
    let cases = [
        ("someone@migadu.com", "Migadu"),
        ("someone@mailbox.org", "mailbox.org"),
        ("someone@runbox.com", "Runbox"),
        ("someone@purelymail.com", "Purelymail"),
        ("someone@mxroute.com", "MXroute"),
    ];
    for (addr, provider) in cases {
        let d = discover(addr)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{addr} not discovered"));
        assert_eq!(d.provider, provider, "{addr} via {:?}", d.via);
    }
}

#[tokio::test]
#[ignore]
async fn a_provider_with_no_imap_is_an_answer_not_a_form() {
    match discover("someone@hey.com").await {
        Err(petrel_autoconfig::Error::NoImap { provider }) => assert_eq!(provider, "HEY"),
        other => panic!("expected NoImap, got {other:?}"),
    }
}
