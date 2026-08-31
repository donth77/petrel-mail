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
        // One buffer, one write. `writeln!` formats straight into the file and
        // can reach the descriptor in several calls, so two threads logging at
        // once interleaved mid-line: a real log holds
        // "17878307040931787830704093  SLOW storage_report: 1087msSLOW status:
        // 348ms", which is two entries spliced together and neither of them
        // parseable. Append mode makes a single write atomic; it cannot make
        // three of them atomic.
        let line = format!("{} {msg}\n", now_ms());
        let _ = f.write_all(line.as_bytes());
    }
}

/// Which provider a host belongs to, for advice that fits it.
///
/// Only used to choose between hints that are already true of that provider —
/// never to decide whether something failed. An unrecognised host gets the
/// general answer, which is correct rather than merely vague.
fn provider_of(host: &str) -> Provider {
    let h = host.to_ascii_lowercase();
    if h.contains("gmail") || h.contains("googlemail") {
        Provider::Gmail
    } else if h.contains("outlook") || h.contains("office365") || h.contains("hotmail") {
        Provider::Microsoft
    } else {
        Provider::Other
    }
}

enum Provider {
    Gmail,
    Microsoft,
    Other,
}

/// Turns a protocol error into something a person can act on.
///
/// The raw text is Rust's Debug rendering of an IMAP response — `code: None,
/// info: Some("[AUTHENTICATIONFAILED] ...")` — which tells a user nothing and
/// tells them it unhelpfully. The detail still goes to sync.log; what reaches
/// the screen should say what to do about it.
///
/// `host` decides which advice fits. Every sign-in failure used to be answered
/// with Gmail's: somebody whose Fastmail password was mistyped was told to
/// switch on 2-Step Verification, and somebody on Outlook was sent to set up a
/// Google app password. Advice for the wrong provider is worse than none —
/// it sends a person to fix something that was never broken.
pub(crate) fn friendly_sync_error_for(host: &str, raw: &str) -> String {
    let r = raw.to_ascii_uppercase();
    if r.contains("AUTHENTICATIONFAILED") || r.contains("INVALID CREDENTIALS") {
        return match provider_of(host) {
            Provider::Gmail => "Sign-in was refused. Gmail needs 2-Step Verification \
                switched on and an app password — your ordinary account password will \
                not work for IMAP."
                .into(),
            // Not "make an app password": Microsoft has been retiring password
            // sign-in for mail, and Petrel cannot do the OAuth that replaces
            // it. Saying so is more use than sending somebody to a settings
            // page that will not help.
            Provider::Microsoft => "Sign-in was refused. Microsoft accounts increasingly \
                require OAuth sign-in for mail, which Petrel does not support yet, so a \
                password may not work here however it is set up."
                .into(),
            Provider::Other => "Sign-in was refused. Check the address and password. \
                Many providers will not accept your ordinary password for mail and want \
                an app password made in their security settings."
                .into(),
        };
    }
    if r.contains("AUTHORIZATIONFAILED") || r.contains("WEBALERT") {
        return match provider_of(host) {
            Provider::Gmail => "The server accepted the password but refused access. \
                For Gmail this usually means IMAP is switched off in settings."
                .into(),
            _ => "The server accepted the password but refused access. Check whether \
                IMAP is switched on for this account."
                .into(),
        };
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
    if is_imap_parse_error(raw) {
        // Parser failures embed the raw FETCH line — subjects, addresses, all
        // of it — in the error text. The detail stays in the engine; what
        // reaches the screen must not repeat any of that.
        return "The server sent a response Petrel could not parse. Your mail \
                is still on the server."
            .into();
    }
    // Anything not recognised above is shown only if it cannot be carrying
    // mail. The old rule was the other way round — print the raw text and
    // hope — and that is how a parse dump put somebody's subject line and
    // correspondents on screen. Matching two substrings caught the dump we
    // had seen; every other parser error still walked straight through.
    //
    // A verdict a server sends about a request is short and structural. A
    // dump is long, and carries the bytes it choked on. Length is a blunt
    // test and a sound one: there is no protocol answer that needs three
    // hundred characters, and no mail content that fits in them next to an
    // error code.
    if looks_like_a_dump(raw) {
        return "The server sent something Petrel could not read. Your mail is \
                still on the server."
            .into();
    }
    raw.to_string()
}

/// Whether an error string is too big, or too structured, to be a verdict.
///
/// Erring towards silence. Saying less than we could costs somebody a detail
/// they might have pasted into an issue; saying more than we should puts
/// their correspondents on screen, and there is no taking that back.
fn looks_like_a_dump(raw: &str) -> bool {
    const LONGEST_PLAUSIBLE_VERDICT: usize = 300;
    raw.len() > LONGEST_PLAUSIBLE_VERDICT
        // The shapes a parser reaches for when it gives up: a byte array, a
        // struct rendered by Debug, or a quoted copy of what it was reading.
        || raw.contains("input:")
        || raw.contains("Error {")
        || raw.contains("FETCH (")
}

/// True when an error string is an IMAP parser dump rather than a protocol
/// verdict. Those dumps carry FETCH lines with mail content and must not be
/// logged or shown verbatim.
pub(crate) fn is_imap_parse_error(raw: &str) -> bool {
    let r = raw.to_ascii_lowercase();
    r.contains("during parsing") || r.contains("takewhile1")
}

#[cfg(test)]
mod sync_error_tests {
    use super::{friendly_sync_error_for, is_imap_parse_error};

    /// Advice for the wrong provider is worse than none.
    ///
    /// Every refused sign-in used to be answered with Gmail's: a Fastmail user
    /// who mistyped a password was told to switch on 2-Step Verification, and
    /// an Outlook user was sent to make a Google app password. Both are being
    /// pointed at something that was never broken.
    const REFUSED: &str = "code: None, info: Some(\"[AUTHENTICATIONFAILED] Invalid credentials\")";

    #[test]
    fn gmail_still_gets_gmails_advice() {
        let msg = friendly_sync_error_for("imap.gmail.com", REFUSED);
        assert!(msg.contains("Gmail"), "{msg}");
        assert!(msg.contains("2-Step"), "{msg}");
    }

    #[test]
    fn microsoft_is_told_the_actual_reason() {
        // Not "make an app password": Microsoft is retiring password sign-in
        // for mail and Petrel has no OAuth, so that advice leads nowhere.
        let msg = friendly_sync_error_for("outlook.office365.com", REFUSED);
        assert!(msg.contains("OAuth"), "{msg}");
        assert!(
            !msg.contains("Gmail"),
            "sent an Outlook user to Google: {msg}"
        );
    }

    #[test]
    fn everybody_else_gets_advice_that_is_true_of_them() {
        for host in [
            "imap.fastmail.com",
            "mail.privateemail.com",
            "imap.mail.me.com",
        ] {
            let msg = friendly_sync_error_for(host, REFUSED);
            assert!(!msg.contains("Gmail"), "{host} was told about Gmail: {msg}");
            assert!(msg.contains("app password"), "{host}: {msg}");
        }
    }

    #[test]
    fn imap_parse_errors_never_echo_fetch_payload() {
        let raw = "imap: io: Error(Error { input: [42], code: TakeWhile1 }) during \
            parsing of \"* 358391 FETCH (UID 1 ENVELOPE (会議の件 user@example.com))\"";
        assert!(is_imap_parse_error(raw));
        let msg = friendly_sync_error_for("imap.example.com", raw);
        assert!(
            msg.contains("could not parse"),
            "expected generic parse message: {msg}"
        );
        assert!(
            msg.contains("still on the server"),
            "expected reassurance: {msg}"
        );
        for leak in ["会議", "example.com", "FETCH ("] {
            assert!(!msg.contains(leak), "leaked mail content in: {msg}");
        }
    }

    #[test]
    fn takewhile1_alone_is_treated_as_parse_failure() {
        let raw = "parse error TakeWhile1 at ENVELOPE 会議の件 user@example.com";
        assert!(is_imap_parse_error(raw));
        let msg = friendly_sync_error_for("imap.example.com", raw);
        assert!(msg.contains("could not parse"), "{msg}");
        for leak in ["会議", "example.com", "ENVELOPE"] {
            assert!(!msg.contains(leak), "leaked mail content in: {msg}");
        }
    }

    #[test]
    fn the_other_verdicts_are_unchanged_whoever_the_host_is() {
        let dns =
            friendly_sync_error_for("imap.example.com", "failed to lookup address: DNS error");
        assert!(dns.contains("could not be looked up"), "{dns}");
        // And something nobody classified still shows its own words rather
        // than a reassuring guess.
        let odd = friendly_sync_error_for("imap.example.com", "something nobody has seen before");
        assert_eq!(odd, "something nobody has seen before");
    }
}

#[cfg(test)]
mod parse_dump_tests {
    use super::friendly_sync_error_for;

    /// A parser dump carries the bytes it choked on, and those bytes are
    /// somebody's mail. The first guard matched two substrings, which caught
    /// the dump we had seen and let every other one through to the screen.
    #[test]
    fn a_dump_with_an_unfamiliar_error_code_still_does_not_reach_the_screen() {
        // Not TakeWhile1, and no "during parsing" — the shape the old guard
        // missed entirely.
        let raw = "imap: Error { input: [42, 32, 49], code: Tag } parsing \
                   \"* 1 FETCH (ENVELOPE (NIL \\\"Q3 invoice\\\" \
                   ((\\\"Dana Wu\\\" NIL \\\"dana\\\" \\\"vendorco.example\\\"))))\"";
        let msg = friendly_sync_error_for("imap.example.com", raw);
        for leak in ["Dana Wu", "vendorco.example", "Q3 invoice", "FETCH ("] {
            assert!(!msg.contains(leak), "leaked {leak:?} in: {msg}");
        }
    }

    #[test]
    fn a_very_long_error_is_summarised_rather_than_repeated() {
        let raw = format!("something unrecognised: {}", "x".repeat(400));
        let msg = friendly_sync_error_for("imap.example.com", &raw);
        assert!(msg.len() < 200, "repeated a 400-character error: {msg}");
    }

    /// The other direction matters too. A short protocol verdict is exactly
    /// what somebody needs to see, and hiding it would make every unusual
    /// failure indistinguishable from every other.
    #[test]
    fn a_short_server_verdict_is_still_shown_in_full() {
        for verdict in [
            "NO [OVERQUOTA] Mailbox is full",
            "BAD Invalid command",
            "NO [SERVERBUG] Internal error occurred",
        ] {
            let msg = friendly_sync_error_for("imap.example.com", verdict);
            assert_eq!(msg, verdict, "hid a verdict worth reading");
        }
    }
}
