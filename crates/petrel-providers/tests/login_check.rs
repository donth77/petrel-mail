//! The onboarding connection test has to give up on a host that accepts the
//! connection and then says nothing. It used to wait for as long as the
//! socket lived, with the setup form spinning on it.
use petrel_providers::imap::{Credential, ImapConfig, Security};
use petrel_providers::smtp::SmtpConfig;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// Accepts, then holds the socket open and silent.
async fn mute_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(120)).await;
        drop(socket);
    });
    port
}

#[tokio::test]
async fn an_imap_host_that_says_nothing_is_reported_not_waited_on() {
    unsafe { std::env::set_var("PETREL_CHECK_SECONDS", "2") };
    let port = mute_server().await;
    let started = Instant::now();
    let err = petrel_providers::imap::login_check(&ImapConfig {
        host: "127.0.0.1".into(),
        port,
        user: "someone".into(),
        credential: Credential::password("secret".to_string()),
        security: Security::Tls,
    })
    .await
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(30), "gave up late");
    assert!(err.to_string().contains("no answer"), "said: {err}");
}

#[tokio::test]
async fn an_smtp_host_that_says_nothing_is_reported_not_waited_on() {
    unsafe { std::env::set_var("PETREL_CHECK_SECONDS", "2") };
    let port = mute_server().await;
    let started = Instant::now();
    let err = petrel_providers::smtp::login_check(&SmtpConfig {
        host: "127.0.0.1".into(),
        port,
        user: "someone".into(),
        credential: Credential::password("secret".to_string()),
    })
    .await
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(30), "gave up late");
    assert!(err.contains("no answer"), "said: {err}");
}
