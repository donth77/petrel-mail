//! What one sync cycle costs on a real account, measured rather than guessed.
//!
//!     source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_cycle_cost -- --ignored --nocapture
//!
//! The question this started from: why does mail show up in Petrel later than
//! in other clients? Because a wake used to be answered with a sweep of every
//! folder — 101 of them on this account — with IDLE torn down for the whole of
//! it, so anything delivered meanwhile was announced to nobody.
//!
//! The numbers here are what the two paths cost, and they are why the loop now
//! has two clocks. A wake takes the inbox alone; the sweep, on its own slower
//! timer, takes the rest. `idle_watch` is measured too, because a held
//! connection that quietly dies would be worse than the reconnecting one it
//! replaces.
//!
//! Read-only throughout: STATUS, SEARCH and a LOGIN. It sends nothing and
//! changes nothing.

use std::time::{Duration, Instant};

use petrel_providers::imap::{FolderPass, ImapConfig, Security, folder_counts, probe, sync_pass};

fn cfg() -> ImapConfig {
    ImapConfig {
        host: std::env::var("PETREL_NC_IMAP_HOST").expect("source .env.namecheap"),
        port: 993,
        user: std::env::var("PETREL_NC_USER").unwrap(),
        pass: std::env::var("PETREL_NC_PASS").unwrap(),
        security: Security::Tls,
    }
}

#[tokio::test]
#[ignore = "reads the real account"]
async fn one_cycle_costs_this_much() {
    let cfg = cfg();

    // The folder list, as the survey sees it.
    let t = Instant::now();
    let report = probe(&cfg, 1).await.expect("probe");
    let probe_ms = t.elapsed().as_millis();
    let paths: Vec<String> = report
        .folders
        .iter()
        .filter(|f| petrel_providers::imap::selectable(f))
        .map(|f| f.name.clone())
        .collect();
    println!("probe: {probe_ms}ms, {} selectable folder(s)", paths.len());
    println!(
        "capabilities: IDLE={} CONDSTORE={} QRESYNC={} MOVE={} UIDPLUS={} -> strategy {:?}",
        report.greeting_capabilities.idle,
        report.greeting_capabilities.condstore,
        report.greeting_capabilities.qresync,
        report.greeting_capabilities.move_,
        report.greeting_capabilities.uidplus,
        report.strategy,
    );

    // What re-arming IDLE costs: a TLS handshake, a LOGIN and a SELECT, paid
    // once per cycle. This is the floor on how long the account is unwatched
    // even if every other step were free.
    let t = Instant::now();
    let _ = petrel_providers::imap::idle_once(&cfg, "INBOX", Duration::from_millis(1)).await;
    println!(
        "idle connect + login + select: {}ms",
        t.elapsed().as_millis()
    );

    // The watcher: one login, held, reporting as it goes. A short ceiling so
    // the test finishes; in the app it is twenty minutes. What this proves is
    // that the session survives being taken in and out of IDLE and closes
    // cleanly at the ceiling — the failure this replaces was a connection per
    // wake, and a held connection that quietly dies is worse than either.
    let t = Instant::now();
    let mut wakes = 0usize;
    let held = petrel_providers::imap::idle_watch(&cfg, "INBOX", Duration::from_secs(20), || {
        wakes += 1;
    })
    .await;
    println!(
        "idle_watch held {}ms, {wakes} wake(s), ended {}",
        t.elapsed().as_millis(),
        match &held {
            Ok(()) => "cleanly at the ceiling".to_string(),
            Err(e) => format!("with {e}"),
        }
    );
    assert!(held.is_ok(), "the held connection must survive its ceiling");

    // The reconcile sweep's first half: one STATUS per folder holding mail.
    let t = Instant::now();
    let counts = folder_counts(&cfg, &paths).await.expect("folder_counts");
    let counts_ms = t.elapsed().as_millis();
    println!(
        "folder_counts over {} folder(s): {counts_ms}ms ({:.0}ms each)",
        paths.len(),
        counts_ms as f64 / paths.len().max(1) as f64
    );

    // The sync pass itself, warm: every folder, watermarked so nothing is
    // fetched. This is the best case — a cycle where no mail has moved.
    let passes: Vec<FolderPass> = paths
        .iter()
        .map(|p| {
            let held = counts
                .iter()
                .find(|(name, _)| name == p)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            FolderPass {
                path: p.clone(),
                // High enough that the server has nothing above it, which is
                // what a warm cycle looks like.
                since_uid: held.saturating_add(1_000_000),
                expected_validity: None,
                since_uidnext: None,
                since_modseq: None,
                seed_window: 1,
            }
        })
        .collect();
    let t = Instant::now();
    let _ = sync_pass(&cfg, &passes, true, |_, _, _, _: &[u8]| {}).await;
    let pass_ms = t.elapsed().as_millis();
    println!("warm sync_pass over {} folder(s): {pass_ms}ms", paths.len());

    // And the same pass narrowed to the folder a wake is actually about. This
    // is what the wake path costs, and the gap between the two numbers is the
    // whole reason for splitting them.
    let inbox: Vec<FolderPass> = passes.into_iter().filter(|p| p.path == "INBOX").collect();
    assert_eq!(inbox.len(), 1, "the account has an INBOX");
    let t = Instant::now();
    let _ = sync_pass(&cfg, &inbox, true, |_, _, _, _: &[u8]| {}).await;
    println!(
        "warm sync_pass over INBOX alone: {}ms",
        t.elapsed().as_millis()
    );

    println!(
        "\n>>> quiet cycle ~= {}ms unwatched, before any mail has to be fetched",
        counts_ms + pass_ms
    );
}
