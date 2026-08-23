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
