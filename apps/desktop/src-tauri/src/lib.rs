//! Petrel's desktop shell: a thin window over the engine. All real work
//! happens in `petrel-engine`; this crate wires typed IPC and (soon) the
//! `petrel-msg://` custom protocol for sanitized message documents.
//!
//! Two source modes: with `PETREL_IMAP_*` set it syncs a real mailbox through
//! the engine's ingest path; without, it seeds synthetic mail so the UI is
//! exercisable with no account. Both run the same store, index, and queries.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{Listing, NewMessage, Store};
use petrel_providers::imap::{ImapConfig, Security};
use petrel_testkit::MailboxGen;
use tauri::{Manager, State};

mod message_view;
mod spike_s2;

use message_view::ViewTokens;

const DEMO_MESSAGES: usize = 10_000;

struct AppState {
    store: Mutex<Store>,
    blobs: BlobStore,
    seeding: AtomicBool,
    seeded: AtomicUsize,
    source: Mutex<String>,
    tokens: Arc<ViewTokens>,
    account_id: i64,
    data_dir: String,
}

#[derive(serde::Serialize)]
struct Status {
    seeding: bool,
    count: usize,
    source: String,
    /// The retention mode, in words. Q24's binding rule is that the active
    /// policy is always stated — never something the user has to infer.
    retention: String,
    data_dir: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
fn status(state: State<Arc<AppState>>) -> Status {
    Status {
        seeding: state.seeding.load(Ordering::Relaxed),
        count: state.seeded.load(Ordering::Relaxed),
        source: state
            .source
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "unknown".into()),
        retention: state
            .store
            .lock()
            .ok()
            .and_then(|s| s.retention_mode(state.account_id).ok())
            .map(|m| m.describe().to_string())
            .unwrap_or_default(),
        data_dir: state.data_dir.clone(),
    }
}

/// Reads account settings from the environment. Credentials never appear in
/// argv (visible to every process on the machine) or in a config file we wrote;
/// the keychain replaces this at M4 when account setup exists.
fn imap_config_from_env() -> Option<ImapConfig> {
    let host = std::env::var("PETREL_IMAP_HOST").ok()?;
    let user = std::env::var("PETREL_IMAP_USER").ok()?;
    let pass = std::env::var("PETREL_IMAP_PASS").ok()?;
    let plaintext = std::env::var("PETREL_IMAP_TLS")
        .map(|v| v == "0")
        .unwrap_or(false);

    #[cfg(feature = "dev-plaintext-imap")]
    let security = if plaintext {
        Security::InsecurePlaintext
    } else {
        Security::Tls
    };
    #[cfg(not(feature = "dev-plaintext-imap"))]
    let security = {
        if plaintext {
            eprintln!(
                "[sync] PETREL_IMAP_TLS=0 ignored: plaintext is not compiled into this build"
            );
        }
        Security::Tls
    };

    Some(ImapConfig {
        host,
        port: std::env::var("PETREL_IMAP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(if plaintext { 143 } else { 993 }),
        user,
        pass,
        security,
    })
}

/// One-shot sync: fetch recent mail and ingest it. Deliberately not a sync
/// engine — that arrives with the orchestrator; this proves the path end to end
/// inside the app.
fn spawn_real_sync(state: Arc<AppState>, account: i64, cfg: ImapConfig) {
    tauri::async_runtime::spawn(async move {
        *state.source.lock().unwrap() = format!("syncing {}…", cfg.host);
        match petrel_providers::imap::fetch_raw(&cfg, "INBOX", 200).await {
            Ok(messages) => {
                eprintln!("[sync] fetched {} message(s)", messages.len());
                // Fetch fully, *then* take the lock: holding a database lock
                // across network I/O would stall every UI query behind the
                // slowest server in the account list.
                let mut ok = 0usize;
                for (uid, raw) in &messages {
                    let mut store = match state.store.lock() {
                        Ok(s) => s,
                        Err(_) => break,
                    };
                    match store.ingest_raw(&state.blobs, account, None, Some(*uid), raw) {
                        Ok(_) => {
                            ok += 1;
                            state.seeded.store(ok, Ordering::Relaxed);
                        }
                        Err(e) => eprintln!("[sync] skipped one message: {e}"),
                    }
                }
                *state.source.lock().unwrap() = format!("{} · {ok} message(s) synced", cfg.user);
                eprintln!("[sync] ingested {ok}/{}", messages.len());
            }
            Err(e) => {
                eprintln!("[sync] failed: {e}");
                *state.source.lock().unwrap() = format!("sync failed: {e}");
            }
        }
        state.seeding.store(false, Ordering::Relaxed);
    });
}

#[tauri::command]
fn list_messages(
    offset: u32,
    limit: u32,
    state: State<Arc<AppState>>,
) -> Result<Vec<Listing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store
        .list_recent(offset, limit.min(100))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn search_messages(query: String, state: State<Arc<AppState>>) -> Result<Vec<Listing>, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    store.search_listing(&query, 50).map_err(|e| e.to_string())
}

/// Issues a one-message URL for the reading pane. The UI never receives the
/// body over IPC — bulk bytes go over the custom protocol, and the frame that
/// renders them has no IPC access at all.
#[tauri::command]
fn message_url(message_id: i64, state: State<Arc<AppState>>) -> Result<String, String> {
    let store = state.store.lock().map_err(|_| "store lock poisoned")?;
    match store.blob_hash_for(message_id).map_err(|e| e.to_string())? {
        Some(_) => Ok(format!(
            "petrel-msg://localhost/message/{}",
            state.tokens.issue(message_id)
        )),
        None => Err("message has no stored body".into()),
    }
}

fn spawn_demo_seeding(state: Arc<AppState>, account: i64) {
    std::thread::spawn(move || {
        let mut generator = MailboxGen::new(7, DEMO_MESSAGES);
        loop {
            let batch: Vec<NewMessage> = generator
                .by_ref()
                .take(500)
                .map(|g| NewMessage {
                    account_id: account,
                    date_ms: g.date_ms,
                    from_addr: g.from_addr,
                    from_display: g.from_display,
                    to_addr: g.to_addr,
                    subject: g.subject,
                    body_text: g.body,
                })
                .collect();
            if batch.is_empty() {
                break;
            }
            let n = batch.len();
            match state.store.lock() {
                Ok(mut store) => {
                    if store.insert_messages(&batch).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
            state.seeded.fetch_add(n, Ordering::Relaxed);
        }
        state.seeding.store(false, Ordering::Relaxed);
    });
}

/// Webview-side diagnostics: init scripts run before page scripts and are
/// exempt from page CSP, so this reports what the webview actually did (loaded
/// URL, script execution, errors, CSP violations) even when the page itself is
/// dead. Events land on stderr via `frontend_log`.
const DIAG: &str = r#"
(function () {
  var buf = [];
  function flush() {
    if (!window.__TAURI_INTERNALS__ || !window.__TAURI_INTERNALS__.invoke) { setTimeout(flush, 50); return; }
    while (buf.length) {
      var e = buf.shift();
      try { window.__TAURI_INTERNALS__.invoke('frontend_log', { entry: e }); } catch (err) {}
    }
  }
  function send(obj) { try { buf.push(JSON.stringify(obj)); } catch (e) { buf.push('"unserializable"'); } flush(); }
  try { document.title = 'D:' + String(location.href).slice(0, 48); } catch (e) {}
  send({ kind: 'boot', href: String(location.href), readyState: document.readyState });
  window.addEventListener('error', function (e) {
    if (e && e.target && e.target !== window && (e.target.src || e.target.href)) {
      send({ kind: 'resource-error', url: String(e.target.src || e.target.href) });
      return;
    }
    send({ kind: 'js-error', msg: String(e.message), src: String(e.filename) + ':' + e.lineno });
  }, true);
  window.addEventListener('unhandledrejection', function (e) { send({ kind: 'rejection', msg: String(e.reason) }); });
  document.addEventListener('securitypolicyviolation', function (e) {
    send({ kind: 'csp-violation', directive: String(e.violatedDirective), blocked: String(e.blockedURI) });
  });
  window.addEventListener('DOMContentLoaded', function () {
    send({ kind: 'dom', scripts: document.scripts.length, root: !!document.getElementById('root') });
    setTimeout(function () {
      var r = document.getElementById('root');
      send({ kind: 'settled', rootChildren: r ? r.childElementCount : -1,
             bodyText: ((document.body && document.body.innerText) || '').slice(0, 80) });
    }, 2000);
  });
})();
"#;

/// Opt-in UI smoke test (`PETREL_SELFTEST=1`): drives the search box the way a
/// user would — real input events into React — and reports what came back.
/// Verifies UI → IPC → engine → FTS → UI end to end without needing OS
/// accessibility permissions. Precursor to the M5 E2E suite.
const SELFTEST: &str = r#"
(function () {
  function log(o) { try { window.__TAURI_INTERNALS__.invoke('frontend_log', { entry: JSON.stringify(o) }); } catch (e) {} }
  function type(el, text) {
    var setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(el, text);
    el.dispatchEvent(new Event('input', { bubbles: true }));
  }
  function rows() { return document.querySelectorAll('.row').length; }
  function timing() { var m = document.querySelectorAll('.meta span'); return m.length > 1 ? m[1].textContent : ''; }
  function firstRow() { var r = document.querySelector('.row'); return r ? r.innerText.replace(/\s+/g, ' ').slice(0, 90) : ''; }
  var queries = (window.__PETREL_SELFTEST_QUERIES__ || ['meeting', 'zephyrite5000', '東京計', 'quarterly report']);
  var i = 0;
  function step() {
    var input = document.querySelector('.search');
    if (!input) { setTimeout(step, 300); return; }
    if (i >= queries.length) {
      // Open the first result so the reading pane renders under observation.
      if (window.__PETREL_SELFTEST_OPEN__) {
        var row = document.querySelector('.row');
        if (row) { row.click(); }
        setTimeout(function () {
          var f = document.querySelector('.reader iframe');
          log({ kind: 'selftest-open', opened: !!f, src: f ? f.getAttribute('src') : null,
                sandbox: f ? f.getAttribute('sandbox') : null });
        }, 1500);
      }
      log({ kind: 'selftest-done' });
      return;
    }
    var q = queries[i++];
    type(input, q);
    setTimeout(function () {
      log({ kind: 'selftest', query: q, results: rows(), timing: timing(), first: firstRow() });
      step();
    }, 900);
  }
  setTimeout(step, 4000);
})();
"#;

#[tauri::command]
fn frontend_log(entry: String) {
    eprintln!("[frontend] {entry}");
}

/// Where mail lives on disk. Shown in the UI so "your mail is yours" is a
/// path the user can open, not a slogan.
fn data_dir() -> std::path::PathBuf {
    std::env::var("PETREL_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Petrel")
        })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).expect("create data directory");
    eprintln!("[store] data directory: {}", dir.display());

    let store = Store::open(&dir.join("petrel.db")).expect("open store");
    // One account row for now; the account model arrives with setup UI.
    let account = match store.first_account().expect("read accounts") {
        Some(id) => id,
        None => store.ensure_test_account().expect("create account row"),
    };
    let blobs = BlobStore::open(&dir.join("blobs")).expect("open blob store");

    // Startup housekeeping: clear temp files left by an interrupted write, then
    // destroy anything whose grace period expired while the app was closed.
    let _ = blobs.sweep_tmp();
    let state = Arc::new(AppState {
        store: Mutex::new(store),
        blobs,
        seeding: AtomicBool::new(true),
        seeded: AtomicUsize::new(0),
        source: Mutex::new("starting…".into()),
        tokens: Arc::new(ViewTokens::new()),
        account_id: account,
        data_dir: dir.display().to_string(),
    });

    {
        let now = now_ms();
        if let Ok(mut store) = state.store.lock() {
            match store.gc(
                &state.blobs,
                now,
                petrel_engine::retention::DEFAULT_GRACE_DAYS,
            ) {
                Ok(r) if r.messages_purged > 0 || r.blobs_removed > 0 => eprintln!(
                    "[store] gc purged {} message(s), reclaimed {} blob(s)",
                    r.messages_purged, r.blobs_removed
                ),
                Ok(_) => {}
                Err(e) => eprintln!("[store] gc failed: {e}"),
            }
        }
    }

    match imap_config_from_env() {
        Some(cfg) => {
            eprintln!("[sync] account configured: {} @ {}", cfg.user, cfg.host);
            spawn_real_sync(state.clone(), account, cfg);
        }
        None => {
            // Demo data is for an empty first run only. Seeding it into a store
            // that already holds real mail would mix fabricated messages into
            // someone's actual mailbox — found the hard way when a persistence
            // test relaunched without credentials and buried a real message
            // under 10,000 synthetic ones.
            let existing = state
                .store
                .lock()
                .ok()
                .and_then(|s| s.message_count().ok())
                .unwrap_or(0);
            if existing > 0 {
                state.seeded.store(existing as usize, Ordering::Relaxed);
                state.seeding.store(false, Ordering::Relaxed);
                *state.source.lock().unwrap() =
                    "no account configured · showing stored mail".into();
            } else {
                *state.source.lock().unwrap() = "synthetic demo data".into();
                spawn_demo_seeding(state.clone(), account);
            }
        }
    }

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            status,
            list_messages,
            search_messages,
            message_url,
            frontend_log
        ])
        .register_uri_scheme_protocol("petrel-msg", move |ctx, request| {
            if request.uri().path().starts_with("/doc/")
                || request.uri().path().starts_with("/beacon/")
            {
                return spike_s2::handle(&request);
            }
            let state = ctx.app_handle().state::<Arc<AppState>>();
            let lookup = |id: i64| {
                state
                    .store
                    .lock()
                    .ok()
                    .and_then(|s| s.blob_hash_for(id).ok().flatten())
            };
            message_view::handle(&request, &state.tokens, &state.blobs, lookup)
        })
        .setup(|app| {
            let mut init = DIAG.to_string();
            if let Ok(mode) = std::env::var("PETREL_SELFTEST") {
                if mode == "open" {
                    init.push_str(
                        "window.__PETREL_SELFTEST_QUERIES__=['hostile'];\
                         window.__PETREL_SELFTEST_OPEN__=true;",
                    );
                }
                init.push_str(SELFTEST);
            }
            if std::env::var("PETREL_SPIKE_S2").is_ok() {
                let port = spike_s2::start_leak_listener();
                eprintln!("[s2] leak listener on 127.0.0.1:{port}");
                init.push_str("window.__PETREL_SPIKE__='s2';");
            }
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
                .title("Petrel")
                .inner_size(1200.0, 800.0)
                .min_inner_size(720.0, 480.0)
                .initialization_script(&init)
                .on_navigation(|url| {
                    eprintln!("[nav] {url}");
                    true
                })
                .on_page_load(|_webview, payload| {
                    eprintln!("[pageload] {:?} {}", payload.event(), payload.url());
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running petrel");
}
