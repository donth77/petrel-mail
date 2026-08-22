//! Measures what a first sync actually costs against a real server.
//!
//! Deliberately prints only shapes and timings — folder counts, byte totals,
//! elapsed seconds. Never a subject, sender, or body: a live-provider run has
//! to be safe to paste into an issue or a transcript (docs 09).
//!
//! Credentials come from the environment, the same ones the app reads:
//!   set -a && . ./.env.local && set +a
//!   cargo run -p petrel-providers --example sync_probe -- [limit]

use std::time::Instant;

use petrel_providers::imap::{ImapConfig, Security, special_use_role};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let limit: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);

    let cfg = ImapConfig {
        host: env("PETREL_IMAP_HOST"),
        port: std::env::var("PETREL_IMAP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(993),
        user: env("PETREL_IMAP_USER"),
        pass: env("PETREL_IMAP_PASS"),
        security: Security::Tls,
    };
    // The host is safe to name; the account is not, so only its shape is shown.
    println!("host      : {}:{}", cfg.host, cfg.port);
    println!("user      : {} chars", cfg.user.len());
    println!("pass      : {} chars", cfg.pass.len());
    println!("limit     : {limit}");
    println!();

    let t0 = Instant::now();
    let report = match petrel_providers::imap::probe(&cfg, 0).await {
        Ok(r) => r,
        Err(e) => {
            println!("probe FAILED after {:?}: {e}", t0.elapsed());
            return;
        }
    };
    println!("probe     : {:?}", t0.elapsed());
    println!("strategy  : {:?}", report.strategy);
    println!(
        "inbox     : {} message(s) on the server",
        report.inbox.exists
    );
    println!("folders   : {}", report.folders.len());
    for f in &report.folders {
        // The folder *name* can be a user's own label, so only roles and the
        // bracketed provider namespace are printed verbatim.
        let role = special_use_role(f).unwrap_or("-");
        let shown = if f.name.starts_with("[Gmail]") || f.name.eq_ignore_ascii_case("INBOX") {
            f.name.clone()
        } else {
            format!("<user folder, {} chars>", f.name.len())
        };
        println!("            {role:<8} {shown}");
    }
    println!();

    let t1 = Instant::now();
    match petrel_providers::imap::fetch_raw(&cfg, "INBOX", limit).await {
        Ok(msgs) => {
            let bytes: usize = msgs.iter().map(|(_, b)| b.len()).sum();
            let elapsed = t1.elapsed();
            println!("fetched   : {} message(s) in {:?}", msgs.len(), elapsed);
            println!("bytes     : {:.1} MB", bytes as f64 / 1_048_576.0);
            if !msgs.is_empty() {
                println!(
                    "mean size : {:.0} KB",
                    bytes as f64 / msgs.len() as f64 / 1024.0
                );
                let largest = msgs.iter().map(|(_, b)| b.len()).max().unwrap_or(0);
                println!("largest   : {:.1} MB", largest as f64 / 1_048_576.0);
                let per = elapsed.as_secs_f64() / msgs.len() as f64;
                println!("rate      : {:.2} s/message", per);
                println!(
                    "→ 200 would take about {:.0} s and {:.0} MB",
                    per * 200.0,
                    bytes as f64 / msgs.len() as f64 * 200.0 / 1_048_576.0
                );
            }
        }
        Err(e) => println!("fetch FAILED after {:?}: {e}", t1.elapsed()),
    }

    // IDLE is the part most likely to be wrong in a way nothing notices, so it
    // gets exercised here rather than discovered in production silence.
    let idle_secs: u64 = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(0);
    if idle_secs > 0 {
        probe_idle(&cfg, idle_secs).await;
    }
}

async fn probe_idle(cfg: &ImapConfig, secs: u64) {
    use std::time::Duration;
    println!();
    println!("idle      : holding the connection for up to {secs}s…");
    let t = Instant::now();
    match petrel_providers::imap::idle_once(cfg, "INBOX", Duration::from_secs(secs)).await {
        Ok(true) => println!(
            "idle      : server reported activity after {:?}",
            t.elapsed()
        ),
        Ok(false) => println!(
            "idle      : clean timeout after {:?} (connection held, no news)",
            t.elapsed()
        ),
        Err(e) => println!("idle FAILED after {:?}: {e}", t.elapsed()),
    }
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        eprintln!("{key} is not set — source .env.local first");
        std::process::exit(1);
    })
}
