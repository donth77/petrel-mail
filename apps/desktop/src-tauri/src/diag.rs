//! Logging and the diagnostic scripts the webview runs on Petrel's behalf.

use crate::state::now_ms;

/// Webview-side diagnostics: init scripts run before page scripts and are
/// exempt from page CSP, so this reports what the webview actually did (loaded
/// URL, script execution, errors, CSP violations) even when the page itself is
/// dead. Events land on stderr via `frontend_log`.
pub(crate) const DIAG: &str = r#"
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

  // What remains of the diagnostics is the part that still earns its place:
  // uncaught errors. The input and focus probes below this were scaffolding for
  // a window that would not respond, which was traced to the launch context
  // months of debugging ago; left in, they wrote a line every three seconds
  // forever and buried the one line that mattered.
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
pub(crate) const SELFTEST: &str = r#"
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
pub fn frontend_log(entry: String) {
    eprintln!("[frontend] {entry}");
    // Also to a file: when the app is launched through LaunchServices (an .app
    // bundle, which is the only way macOS gives it real focus) stderr goes
    // nowhere readable, and diagnostics that vanish are not diagnostics.
    let path = data_dir().join("frontend.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{entry}");
    }
}

/// Where mail lives on disk. Shown in the UI so "your mail is yours" is a
/// path the user can open, not a slogan.
pub(crate) fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PETREL_DATA_DIR") {
        return std::path::PathBuf::from(dir);
    }
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    // The era of a separate "live" store is over, but the store itself is
    // not: real accounts and their mail live in Petrel-live because the
    // launch script kept them apart from demo data. A plain Dock launch
    // used to open the demo directory instead — same window, different
    // world, nothing on screen saying so — and the first thing it offered
    // was onboarding into the wrong store. Prefer the live directory when
    // it exists, so every way of launching opens the same mail.
    let live = base.join("Petrel-live");
    if live.join("petrel.db").exists() {
        return live;
    }
    base.join("Petrel")
}

/// Appends a line to a log file in the data directory.
///
/// Under LaunchServices — which is the only way the app gets real keyboard
/// focus on macOS — stderr goes nowhere readable, so `eprintln!` diagnostics
/// vanish precisely when the app is being run the way a user runs it. Anything
/// worth printing during a sync is worth writing here.
pub(crate) fn log_sync(msg: &str) {
    eprintln!("[sync] {msg}");
    let path = data_dir().join("sync.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{} {msg}", now_ms());
    }
}

/// Turns a protocol error into something a person can act on.
///
/// The raw text is Rust's Debug rendering of an IMAP response — `code: None,
/// info: Some("[AUTHENTICATIONFAILED] ...")` — which tells a user nothing and
/// tells them it unhelpfully. The detail still goes to sync.log; what reaches
/// the screen should say what to do about it.
pub(crate) fn friendly_sync_error(raw: &str) -> String {
    let r = raw.to_ascii_uppercase();
    if r.contains("AUTHENTICATIONFAILED") || r.contains("INVALID CREDENTIALS") {
        return "Sign-in was refused. Gmail needs 2-Step Verification switched on \
                and an app password — your ordinary account password will not work \
                for IMAP."
            .into();
    }
    if r.contains("AUTHORIZATIONFAILED") || r.contains("WEBALERT") {
        return "The server accepted the password but refused access. For Gmail \
                this usually means IMAP is switched off in settings."
            .into();
    }
    if r.contains("DNS") || r.contains("NAME OR SERVICE") || r.contains("RESOLVE") {
        return "That server name could not be looked up. Check the host.".into();
    }
    if r.contains("CONNECTION REFUSED") || r.contains("TIMED OUT") || r.contains("TIMEOUT") {
        return "The server did not answer. Check the host and port, and whether \
                something on this network blocks IMAP."
            .into();
    }
    if r.contains("CERTIFICATE") || r.contains("TLS") || r.contains("HANDSHAKE") {
        return "The encrypted connection could not be established, so Petrel \
                stopped rather than continuing in the clear."
            .into();
    }
    // Unknown: show the raw text rather than a reassuring guess.
    raw.to_string()
}
