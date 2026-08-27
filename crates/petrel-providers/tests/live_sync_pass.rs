//! The relaunch tax, measured against a real server.
//!
//!     source .env.namecheap && \
//!     cargo test -p petrel-providers --test live_sync_pass -- --ignored --nocapture

use std::time::Instant;

use petrel_providers::imap::{FolderPass, ImapConfig, PassOutcome, Security, sync_pass};

#[tokio::test]
#[ignore = "reads the real test account"]
async fn a_second_cycle_over_a_warm_store_fetches_nothing() {
    let cfg = ImapConfig {
        host: std::env::var("PETREL_NC_IMAP_HOST").expect("source .env.namecheap"),
        port: 993,
        user: std::env::var("PETREL_NC_USER").unwrap(),
        pass: std::env::var("PETREL_NC_PASS").unwrap(),
        security: Security::Tls,
    };
    let names = [
        "INBOX",
        "Sent",
        "Archive/Yearly/2023",
        "Archive/Yearly/2023/Job Hunt 2023",
        "Archive/Newsletter/TLDR",
        "Archive/Outdated/Interviews",
    ];

    // Cold: no watermarks — every folder seeds a 50-message window.
    let cold: Vec<FolderPass> = names
        .iter()
        .map(|n| FolderPass {
            path: n.to_string(),
            since_uid: 0,
            expected_validity: None,
            since_uidnext: None,
            since_modseq: None,
            seed_window: 50,
        })
        .collect();
    let mut fetched_bytes = 0usize;
    let mut watermarks: Vec<u32> = vec![0; names.len()];
    let t0 = Instant::now();
    let out = sync_pass(&cfg, &cold, true, |i, uid, _f, raw: &[u8]| {
        fetched_bytes += raw.len();
        if uid > watermarks[i] {
            watermarks[i] = uid;
        }
    })
    .await
    .expect("cold cycle");
    let cold_time = t0.elapsed();
    let cold_count: usize = out
        .iter()
        .map(|o| match o {
            PassOutcome::Fetched { fetched, .. } => *fetched,
            _ => 0,
        })
        .sum();

    // Warm: watermarks, validity and modseq from the cold pass — the relaunch.
    let warm: Vec<FolderPass> = out
        .iter()
        .zip(names.iter())
        .zip(watermarks.iter())
        .map(|((o, n), w)| {
            let (v, m, nx) = match o {
                PassOutcome::Fetched {
                    uid_validity,
                    highest_modseq,
                    uid_next,
                    ..
                } => (*uid_validity, *highest_modseq, *uid_next),
                _ => (None, None, None),
            };
            FolderPass {
                path: n.to_string(),
                since_uid: *w,
                expected_validity: v,
                since_uidnext: nx,
                since_modseq: m,
                seed_window: 50,
            }
        })
        .collect();
    let t1 = Instant::now();
    let out2 = sync_pass(&cfg, &warm, true, |_, _, _, _: &[u8]| {
        panic!("a warm cycle over an unchanged mailbox must fetch nothing")
    })
    .await
    .expect("warm cycle");
    let warm_time = t1.elapsed();
    let quiet = out2
        .iter()
        .filter(|o| matches!(o, PassOutcome::Unchanged { .. }))
        .count();

    println!(
        "cold: {cold_count} messages, {}KB, {cold_time:?} · warm: {quiet}/{} unchanged, {warm_time:?}, 0 bytes of mail",
        fetched_bytes / 1024,
        names.len(),
    );
    assert_eq!(quiet, names.len(), "{out2:?}");
    assert!(warm_time < cold_time, "warm must be cheaper than cold");
}
