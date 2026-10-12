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
  // Where a blocked or failed resource lives, never its whole address. The
  // composer's remote images are a message's trackers, and a tracker's path
  // is commonly the recipient's address; the scheme and host say what was
  // refused without writing that into frontend.log. CSP's keywords
  // ("inline", "eval") carry no colon and pass through as they are.
  function origin(u) {
    var s = String(u);
    if (s.indexOf(':') < 0) { return s; }
    try { var x = new URL(s); return x.protocol + (x.host ? '//' + x.host : ''); } catch (e) { return 'unparsable'; }
  }

  // What remains of the diagnostics is the part that still earns its place:
  // uncaught errors. The input and focus probes below this were scaffolding for
  // a window that would not respond, which was traced to the launch context
  // months of debugging ago; left in, they wrote a line every three seconds
  // forever and buried the one line that mattered.
  try { document.title = 'D:' + String(location.href).slice(0, 48); } catch (e) {}
  send({ kind: 'boot', href: String(location.href), readyState: document.readyState });
  window.addEventListener('error', function (e) {
    if (e && e.target && e.target !== window && (e.target.src || e.target.href)) {
      send({ kind: 'resource-error', url: origin(e.target.src || e.target.href) });
      return;
    }
    send({ kind: 'js-error', msg: String(e.message), src: String(e.filename) + ':' + e.lineno });
  }, true);
  window.addEventListener('unhandledrejection', function (e) { send({ kind: 'rejection', msg: String(e.reason) }); });
  document.addEventListener('securitypolicyviolation', function (e) {
    send({ kind: 'csp-violation', directive: String(e.violatedDirective), blocked: origin(e.blockedURI) });
  });
  window.addEventListener('DOMContentLoaded', function () {
    send({ kind: 'dom', scripts: document.scripts.length, root: !!document.getElementById('root') });
    setTimeout(function () {
      var r = document.getElementById('root');
      // How much text, not which: the first 80 characters of the page are
      // the rail, and the rail starts with the signed-in address.
      send({ kind: 'settled', rootChildren: r ? r.childElementCount : -1,
             bodyChars: ((document.body && document.body.innerText) || '').length });
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
      // How many rows, never what they say: the first row is somebody's
      // sender, subject and opening line, and this runs on real mailboxes.
      log({ kind: 'selftest', query: q, results: rows(), timing: timing() });
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
    rotate_if_large(&path, LOG_LIMIT_BYTES);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{entry}");
    }
}

/// How big a log file may grow before it is set aside. A few megabytes is
/// weeks of ordinary use and still opens in anything.
const LOG_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

/// Sets a full log aside as `<name>.1`, replacing the previous one.
///
/// Neither log ever rotated, so on a machine that had run Petrel for a year
/// `sync.log` was whatever a year of sync lines comes to, and it grew a line
/// per event for as long as the app was installed. One previous file is
/// kept, which is enough to read what happened just before a restart.
pub(crate) fn rotate_if_large(path: &std::path::Path, limit: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < limit {
        return;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let previous = path.with_file_name(format!("{name}.1"));
    // Windows will not rename over an existing file.
    let _ = std::fs::remove_file(&previous);
    let _ = std::fs::rename(path, &previous);
}

/// Creates a directory that only this user can read, for files that are
/// somebody's mail: dropped attachments waiting to be sent, and copies made
/// so the OS can open them.
pub(crate) fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
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

/// Every line logged while the tests run, for the tests that check what the
/// log may and may not say.
///
/// They read here rather than the file. A test run's data directory is
/// wherever `PETREL_DATA_DIR` points, or, unset, a directory CI never creates,
/// so every write failed and a test reading the file read nothing. On a
/// developer's machine it would have been their own sync.log.
#[cfg(test)]
pub(crate) static TEST_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// What has been logged so far in this test run, one line per entry. Only the
/// scripted-server tests read it.
#[cfg(all(test, feature = "dev-plaintext-imap"))]
pub(crate) fn test_log_text() -> String {
    TEST_LOG
        .lock()
        .map(|lines| lines.join("\n"))
        .unwrap_or_default()
}

/// Appends a line to a log file in the data directory.
///
/// Under LaunchServices — which is the only way the app gets real keyboard
/// focus on macOS — stderr goes nowhere readable, so `eprintln!` diagnostics
/// vanish precisely when the app is being run the way a user runs it. Anything
/// worth printing during a sync is worth writing here.
pub(crate) fn log_sync(msg: &str) {
    eprintln!("[sync] {msg}");
    #[cfg(test)]
    if let Ok(mut lines) = TEST_LOG.lock() {
        lines.push(msg.to_string());
    }
    let path = data_dir().join("sync.log");
    rotate_if_large(&path, LOG_LIMIT_BYTES);
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

/// Which of an account's two servers a failure came from, for advice that
/// names the right one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Server {
    /// IMAP, where mail is read.
    Incoming,
    /// SMTP, where it is sent from.
    Outgoing,
}

/// What `friendly_error_for` says, by the id the window translates it under.
///
/// The English is en.ftl's, word for word (`advice_is_the_strings_file`
/// holds the two equal), because the window knows a sentence by it and shows
/// its own language's instead (`adviceText`). It said these in English in
/// every one of the eight, on the screen a new account first fails on.
/// Anything else that reaches the screen is the server's own words, and is
/// shown as they came.
pub(crate) const ADVICE: &[(&str, &str)] = &[
    (
        "advice-access-refused-gmail",
        "The server accepted the password but refused access. For Gmail this usually means IMAP is switched off in settings.",
    ),
    (
        "advice-access-refused",
        "The server accepted the password but refused access. Check whether IMAP is switched on for this account.",
    ),
    (
        "advice-signin-refused-gmail",
        "Sign-in was refused. Gmail needs 2-Step Verification switched on and an app password — your ordinary account password will not work for IMAP.",
    ),
    (
        "advice-signin-refused-microsoft",
        "Sign-in was refused. Microsoft accounts increasingly require OAuth sign-in for mail, which Petrel does not support yet, so a password may not work here however it is set up.",
    ),
    (
        "advice-signin-refused",
        "Sign-in was refused. Check the address and password. Many providers will not accept your ordinary password for mail and want an app password made in their security settings.",
    ),
    (
        "advice-no-such-host",
        "That server name could not be looked up. Check the host.",
    ),
    (
        "advice-no-answer-imap",
        "The server did not answer. Check the host and port, and whether something on this network blocks IMAP.",
    ),
    (
        "advice-no-answer-smtp",
        "The server did not answer. Check the host and port, and whether something on this network blocks outgoing mail (SMTP).",
    ),
    (
        "advice-no-tls",
        "The encrypted connection could not be established, so Petrel stopped rather than continuing in the clear.",
    ),
    (
        "advice-unparsable",
        "The server sent a response Petrel could not parse. Your mail is still on the server.",
    ),
    (
        "advice-unreadable",
        "The server sent something Petrel could not read. Your mail is still on the server.",
    ),
    ("advice-no-folder-synced", "No folder could be synced."),
    // Which server a connection test failed on, ahead of the advice.
    ("advice-half-imap", "Incoming (IMAP)"),
    ("advice-half-smtp", "Outgoing (SMTP)"),
];

/// What a sync says when every folder failed and the server gave no reason
/// of its own. Petrel's words, so they are advice like the rest.
pub(crate) const NO_FOLDER_SYNCED: &str = "no folder could be synced";

/// One of `ADVICE`, by id.
pub(crate) fn advice(id: &str) -> &'static str {
    ADVICE
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, text)| *text)
        .unwrap_or_default()
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
    friendly_error_for(Server::Incoming, host, raw)
}

/// `friendly_sync_error_for`, told which server failed. Onboarding and Sign
/// in again test both, and the outgoing one was told to check whether the
/// network blocks IMAP.
pub(crate) fn friendly_error_for(server: Server, host: &str, raw: &str) -> String {
    if raw == NO_FOLDER_SYNCED {
        return advice("advice-no-folder-synced").into();
    }
    let r = raw.to_ascii_uppercase();
    // Before the password's advice: refused at sign-in, these too read as a
    // refusal, but the password was right and the advice is not about it.
    if r.contains("AUTHORIZATIONFAILED") || r.contains("WEBALERT") {
        return match provider_of(host) {
            Provider::Gmail => advice("advice-access-refused-gmail").into(),
            _ => advice("advice-access-refused").into(),
        };
    }
    // Gmail's word for its ordinary password, with 2-Step Verification on,
    // comes as an ALERT. `imap::sign_in` counts that one as refused, so it
    // usually arrives marked; the words are matched as well for any path that
    // reports the server's text unmarked.
    if is_password_refusal(raw) || r.contains("APPLICATION-SPECIFIC PASSWORD REQUIRED") {
        return match provider_of(host) {
            Provider::Gmail => advice("advice-signin-refused-gmail").into(),
            // Not "make an app password": Microsoft has been retiring password
            // sign-in for mail, and Petrel cannot do the OAuth that replaces
            // it. Saying so is more use than sending somebody to a settings
            // page that will not help.
            Provider::Microsoft => advice("advice-signin-refused-microsoft").into(),
            Provider::Other => advice("advice-signin-refused").into(),
        };
    }
    if r.contains("DNS") || r.contains("NAME OR SERVICE") || r.contains("RESOLVE") {
        return advice("advice-no-such-host").into();
    }
    // Windows words both differently: a refused port is "the target machine
    // actively refused it" (os error 10061), and a silent one "the connected
    // host has failed to respond" (10060). Matched by those words, or the
    // advice was the system's sentence instead of what to check.
    if r.contains("CONNECTION REFUSED")
        || r.contains("ACTIVELY REFUSED")
        || r.contains("TIMED OUT")
        || r.contains("TIMEOUT")
        || r.contains("FAILED TO RESPOND")
    {
        return match server {
            Server::Incoming => advice("advice-no-answer-imap"),
            // Sending is what networks block most: port 25 often, and some
            // hotel and office networks every SMTP port.
            Server::Outgoing => advice("advice-no-answer-smtp"),
        }
        .into();
    }
    if r.contains("CERTIFICATE") || r.contains("TLS") || r.contains("HANDSHAKE") {
        return advice("advice-no-tls").into();
    }
    if is_imap_parse_error(raw) {
        // Parser failures embed the raw FETCH line — subjects, addresses, all
        // of it — in the error text. The detail stays in the engine; what
        // reaches the screen must not repeat any of that.
        return advice("advice-unparsable").into();
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
        return advice("advice-unreadable").into();
    }
    raw.to_string()
}

/// Whether an IMAP failure is the server refusing the password at sign-in.
///
/// The provider's sign-in decides that by the reply's code
/// (`ImapError::SignInRefused`, written "sign-in refused: …"), and this only
/// reads its verdict back from text that has lost its type, such as a
/// cycle's last failure. It reads no server words: a bare "535 " in the
/// words matched Dovecot's timing figure on a refused MOVE, "(0.535 + 0.000
/// secs)", and stood the whole account down for an hour. Where the error
/// still has its type, ask it (`ImapError::is_sign_in_refused`).
pub(crate) fn is_sign_in_refusal(raw: &str) -> bool {
    raw.contains("sign-in refused: ")
}

/// Whether a failure is a refused password, for the advice on screen: the
/// IMAP sign-in's verdict (`is_sign_in_refusal`), or SMTP's — the send
/// worker's record of its sign-in stage ("auth: …"), or a 535 reply code at
/// the start of the reply, followed by a space or a hyphen as SMTP writes
/// it ("535 5.7.8 …", "535-5.7.8 …"). A 535 anywhere else is a number: a
/// Dovecot timing figure, "(0.535 + 0.000 secs)", stood an account down.
pub(crate) fn is_password_refusal(raw: &str) -> bool {
    if is_sign_in_refusal(raw) || raw.starts_with("auth:") {
        return true;
    }
    let reply = raw.trim_start();
    reply.len() > 3 && reply.starts_with("535") && matches!(reply.as_bytes()[3], b' ' | b'-')
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

/// Blanks anything shaped like an address before a line reaches the log.
///
/// Server replies quote what they refused: a Postfix rejection reads
/// `550 5.1.1 <someone@example.com>: Recipient address rejected`, and the
/// log is the one place the project keeps addresses out of. The row keeps
/// the full reply; the person can read it there.
pub(crate) fn without_addresses(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.contains('@') {
                "<address>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod rotation_tests {
    use super::rotate_if_large;

    /// A log over the limit is set aside as `.1` and a fresh one starts;
    /// the one before that is gone, so two files is all a log ever costs.
    #[test]
    fn a_full_log_is_set_aside_and_the_one_before_it_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("sync.log");
        let previous = dir.path().join("sync.log.1");

        std::fs::write(&log, "small\n").unwrap();
        rotate_if_large(&log, 1024);
        assert!(log.exists(), "under the limit, nothing moves");
        assert!(!previous.exists());

        std::fs::write(&previous, "older\n").unwrap();
        std::fs::write(&log, "x".repeat(2048)).unwrap();
        rotate_if_large(&log, 1024);
        assert!(!log.exists(), "the full log was set aside");
        assert_eq!(
            std::fs::read(&previous).unwrap().len(),
            2048,
            "the previous file is the log that just filled"
        );
        // A missing file is not an error.
        rotate_if_large(&dir.path().join("absent.log"), 1024);
    }
}

#[cfg(test)]
mod address_tests {
    use super::without_addresses;

    #[test]
    fn a_rejection_loses_the_address_and_keeps_the_verdict() {
        let line = "550 5.1.1 <someone@example.com>: Recipient address rejected: User unknown";
        let out = without_addresses(line);
        assert!(!out.contains("someone"), "{out}");
        assert!(out.contains("550 5.1.1"), "{out}");
        assert!(out.contains("Recipient address rejected"), "{out}");
        assert_eq!(
            without_addresses("250 2.0.0 Ok: queued"),
            "250 2.0.0 Ok: queued"
        );
    }
}

#[cfg(test)]
mod sync_error_tests {
    use super::{
        ADVICE, Server, advice, friendly_error_for, friendly_sync_error_for, is_imap_parse_error,
        is_password_refusal, is_sign_in_refusal,
    };

    /// The window knows each sentence by its English, so the English here
    /// and in en.ftl must be the same, character for character. Changed in
    /// one place only, the sentence would be shown untranslated again.
    #[test]
    fn advice_is_the_strings_file() {
        let ftl = include_str!("../../ui/src/locales/en.ftl");
        for (id, english) in ADVICE {
            let line = format!("{id} = {english}");
            assert!(
                ftl.lines().any(|l| l == line),
                "en.ftl lacks, or words differently: {line}"
            );
            assert_eq!(advice(id), *english);
        }
        assert_eq!(advice("advice-nothing-of-the-sort"), "");
        assert_eq!(
            friendly_sync_error_for("imap.example.com", super::NO_FOLDER_SYNCED),
            advice("advice-no-folder-synced"),
            "Petrel's own words are advice like the rest"
        );
    }

    /// Advice for the wrong provider is worse than none.
    ///
    /// Every refused sign-in used to be answered with Gmail's: a Fastmail user
    /// who mistyped a password was told to switch on 2-Step Verification, and
    /// an Outlook user was sent to make a Google app password. Both are being
    /// pointed at something that was never broken.
    const REFUSED: &str =
        "sign-in refused: code: None, info: Some(\"[AUTHENTICATIONFAILED] Invalid credentials\")";

    /// What stands an account down (signin.rs): the server refusing the
    /// password at sign-in, as the provider's sign-in classifies it by the
    /// reply's code (`ImapError::SignInRefused`, shown as "sign-in refused:"),
    /// and nothing else. Words are not read: a bare "535 " matched Dovecot's
    /// timing figure "(0.535 + 0.000 secs)" on a refused MOVE, and stood the
    /// whole account down for an hour.
    #[test]
    fn only_a_refused_sign_in_reads_as_one() {
        for raw in [
            r#"sign-in refused: code: None, info: Some("LOGIN failed.")"#,
            r#"sync cycle failed before any folder: sign-in refused: code: None, info: Some("[AUTHENTICATIONFAILED] Invalid credentials (Failure)")"#,
        ] {
            assert!(is_sign_in_refusal(raw), "{raw}");
        }
        for raw in [
            r#"imap: no response: code: Some(TryCreate), info: Some("Mailbox doesn't exist: Archive (0.535 + 0.000 secs).")"#,
            r#"imap: no response: code: Some(Alert), info: Some("Too many simultaneous connections. (Failure)")"#,
            r#"imap: no response: code: None, info: Some("[AUTHENTICATIONFAILED] Invalid credentials")"#,
            "Invalid credentials",
            "smtp: 535 5.7.8 Username and Password not accepted",
            "auth: rejected",
            "network: Connection refused (os error 61)",
            "tls: invalid peer certificate: UnknownIssuer",
            "NO [UNAVAILABLE] Server busy, try later",
            "no folder could be synced",
        ] {
            assert!(!is_sign_in_refusal(raw), "{raw}");
        }
    }

    /// The advice on screen knows a refused password from either protocol:
    /// the IMAP sign-in's classification, or SMTP's 535 as a reply code, at
    /// the start of the reply and followed by a space or a hyphen, or the
    /// send worker's record of its sign-in stage. A 535 anywhere else is a
    /// number.
    #[test]
    fn the_advice_knows_an_smtp_refusal_by_its_reply_code() {
        for raw in [
            "auth: 535 5.7.8 Username and Password not accepted",
            "535 5.7.8 Username and Password not accepted",
            "535-5.7.8 Username and Password not accepted.",
            "Outgoing (SMTP) — sign-in refused: 535 5.7.8 Authentication failed",
            r#"sign-in refused: code: None, info: Some("LOGIN failed.")"#,
        ] {
            assert!(is_password_refusal(raw), "{raw}");
        }
        for raw in [
            r#"imap: no response: code: Some(TryCreate), info: Some("Mailbox doesn't exist: Archive (0.535 + 0.000 secs).")"#,
            "rcpt: 550 5.1.1 no such user",
            "data: 5350 something",
            "connect: 1535 refused",
        ] {
            assert!(!is_password_refusal(raw), "{raw}");
        }
    }

    /// Gmail turns down an ordinary password, once 2-Step Verification is on,
    /// with an ALERT. Marked by the sign-in (`imap::sign_in` counts it as
    /// refused) or not, it gets the app password advice rather than the
    /// server's words in a debug dump.
    #[test]
    fn gmails_app_password_alert_gets_the_app_password_advice() {
        let alert = r#"code: Some(Alert), info: Some("Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)")"#;
        let marked = format!("sign-in refused: {alert}");
        assert!(is_password_refusal(&marked));
        let unmarked = format!("imap: no response: {alert}");
        assert!(!is_password_refusal(&unmarked));
        for raw in [marked.as_str(), unmarked.as_str()] {
            let said = friendly_sync_error_for("imap.gmail.com", raw);
            assert!(said.contains("app password"), "{said}");
        }
    }

    /// Refused at sign-in, an account the server will not let in for another
    /// reason still gets that reason's advice, not the password's.
    #[test]
    fn a_sign_in_refused_for_another_reason_keeps_its_own_advice() {
        let raw =
            r#"sign-in refused: code: None, info: Some("[AUTHORIZATIONFAILED] IMAP disabled")"#;
        let said = friendly_sync_error_for("imap.gmail.com", raw);
        assert!(said.contains("IMAP is switched off"), "{said}");
        let plain = r#"sign-in refused: code: None, info: Some("LOGIN failed.")"#;
        assert!(
            friendly_sync_error_for("mail.example.com", plain)
                .contains("Check the address and password")
        );
    }

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

    /// A port nothing listens on, or a server that never answers, gets the
    /// same advice on every system, in the system's own words for it. Windows
    /// says neither "connection refused" nor "timed out", and its users were
    /// shown the system's sentence instead.
    #[test]
    fn a_refused_or_silent_server_gets_advice_in_every_systems_words() {
        for raw in [
            "connect: Connection refused (os error 61)",
            "connect: Connection refused (os error 111)",
            "connect: No connection could be made because the target machine actively refused it. (os error 10061)",
            "connect: Operation timed out (os error 60)",
            "connect: A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond. (os error 10060)",
        ] {
            let incoming = friendly_error_for(Server::Incoming, "mail.example.com", raw);
            assert!(incoming.contains("blocks IMAP"), "{raw} → {incoming}");
            let outgoing = friendly_error_for(Server::Outgoing, "mail.example.com", raw);
            assert!(
                outgoing.contains("outgoing mail (SMTP)"),
                "{raw} → {outgoing}"
            );
        }
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
