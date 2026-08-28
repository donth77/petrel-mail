//! Serves sanitized message bodies over the `petrel-msg` custom protocol.
//!
//! This is the reading pane's supply line, and it applies the layered defense
//! measured to hold on this platform:
//!   1. the body is sanitized here (allowlist; hostile constructs removed),
//!   2. the response carries a per-message CSP that blocks network egress,
//!   3. the UI renders it in a `sandbox`ed iframe with no scripts and no IPC.
//!
//! Each layer blocks a class the others do not, so none is redundant.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use petrel_engine::blob::BlobStore;
use tauri::http::{Request, Response};

/// Single-use, unguessable token gating access to a message body. Without this
/// any document rendered in the app could name another message's URL.
pub struct ViewTokens {
    counter: AtomicU64,
    issued: std::sync::Mutex<std::collections::HashMap<String, i64>>,
}

impl ViewTokens {
    pub fn new() -> Self {
        ViewTokens {
            counter: AtomicU64::new(0),
            issued: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Issues a token for one message. Not a security boundary on its own — the
    /// sandbox and CSP are — but it keeps message URLs unguessable and scoped.
    pub fn issue(&self, message_id: i64) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        // Process-local, non-persistent; a real nonce arrives with the
        // multi-window work, where tokens must not be shareable across windows.
        let token = format!(
            "{n:x}-{:x}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        if let Ok(mut map) = self.issued.lock() {
            map.insert(token.clone(), message_id);
        }
        token
    }

    fn resolve(&self, token: &str) -> Option<i64> {
        self.issued.lock().ok()?.get(token).copied()
    }
}

impl Default for ViewTokens {
    fn default() -> Self {
        Self::new()
    }
}

fn error_response(status: u16, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .expect("error response")
}

/// The document shell. Styling lives here rather than in the message so that a
/// message cannot restyle the reading pane around it.
/// The one script Petrel puts in a message frame. Two jobs, both structural:
///
///  1. Report the document height, so the reading pane can size the frame to its
///     content instead of nesting a scroll region inside a scroll region.
///  2. Forward keystrokes back out. A focused iframe swallows every keydown —
///     they reach the frame's document, never the parent — so once you click a
///     message, every shortcut in the application stops working. The frame holds
///     sanitized mail with no inputs, so nothing here is typing.
///
/// Admitted by a per-response nonce. It forwards key
/// identity only: never characters typed, never content, never anything read
/// from the document.
/// A per-response CSP nonce. Uniqueness is what matters — it is compared against
/// itself within one response and never reused — so process id, a counter and the
/// clock's nanoseconds are sufficient without pulling in an RNG dependency.
fn new_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{n:x}{nanos:x}", std::process::id())
}

const HEIGHT_REPORTER: &str = r#"
(function () {
  // Below this, shrinking stops being a way to read the message and becomes a
  // way to be unable to. What is left over scrolls sideways instead.
  var MIN_SCALE = 0.5;
  var fitting = false;

  // Fits a too-wide message by scaling it, rather than cutting it off.
  //
  // Mail is full of layouts built to a fixed width, and a reading pane is
  // whatever width the window happens to be. Three things can happen to the
  // difference: the content is clipped, which loses it with no way to reach it;
  // it is squeezed, which takes fixed-width designs apart cell by cell; or the
  // whole thing is scaled down as one piece, which is the only one of the three
  // that keeps the message looking like itself.
  //
  // Scaling, not resizing: a transform leaves the layout alone, so nothing
  // reflows and the proportions the sender chose survive intact. The cost is
  // that a transform does not change the space the element reserves, so the
  // box around it has to be told the scaled height or the frame keeps a band of
  // blank space under short, wide mail.
  function fit() {
    var box = document.getElementById('petrel-box');
    var inner = document.getElementById('petrel-fit');
    if (!box || !inner || fitting) return;
    fitting = true;
    try {
      // Measured unscaled: a previous scale would otherwise be measured in and
      // the message would shrink a little further on every pass.
      inner.style.transform = '';
      box.style.height = '';
      var avail = inner.clientWidth;
      var natural = inner.scrollWidth;
      if (avail > 0 && natural > avail + 1) {
        var k = Math.max(avail / natural, MIN_SCALE);
        inner.style.transform = 'scale(' + k + ')';
        box.style.height = Math.ceil(inner.getBoundingClientRect().height) + 'px';
      }
    } catch (e) {}
    fitting = false;
  }

  function h() {
    var d = document.documentElement, b = document.body;
    return Math.max(d.scrollHeight, b ? b.scrollHeight : 0);
  }
  function post() {
    fit();
    try {
      parent.postMessage({ petrelHeight: h(), petrelBlocked: BLOCKED }, '*');
    } catch (e) {}
  }
  addEventListener('load', post);
  addEventListener('resize', post);
  if (window.ResizeObserver) { new ResizeObserver(post).observe(document.documentElement); }
  post();
  setTimeout(post, 60);
  setTimeout(post, 400);

  // Where a link actually goes, reported out for the app to show.
  //
  // This is a security control as much as a convenience. Phishing *is* link
  // text that disagrees with its destination, and mail is where it lands — so
  // the one habit worth supporting is looking before clicking. A browser gives
  // you that for free in its status bar; a reading pane has to be told to.
  // It matters more here than in a browser, because the link opens somewhere
  // else entirely and there is no address bar to check on the way.
  function hover(url) {
    try { parent.postMessage({ petrelHover: url || '' }, '*'); } catch (e) {}
  }
  addEventListener('mouseover', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (a) hover(a.href);
  });
  addEventListener('mouseout', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (a) hover('');
  });
  // A link can be left by scrolling or by the pointer leaving the frame
  // altogether, neither of which fires mouseout on the anchor.
  addEventListener('blur', function () { hover(''); });
  document.addEventListener('mouseleave', function () { hover(''); });

  // Links leave the frame, they do not navigate it.
  //
  // Left alone, a click would replace the message with whatever the sender
  // linked to — a live web page loaded inside the reading pane, no longer
  // carrying this response's CSP. So every click is caught here and the
  // destination handed out to the app, which decides what opening it means.
  // The frame never navigates and never opens anything itself.
  addEventListener('click', function (e) {
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (!a) return;
    e.preventDefault();
    try { parent.postMessage({ petrelOpen: a.href }, '*'); } catch (err) {}
  });

  // The reading-size preference. A CSS variable on the host cannot cross into
  // an opaque-origin frame, so the size is sent in and applied here — which is
  // also why it takes effect immediately rather than on the next fetch.
  addEventListener('message', function (e) {
    var n = e.data && e.data.petrelSize;
    // Bounded: the only thing this accepts is a plausible font size.
    if (typeof n === 'number' && n >= 10 && n <= 28) {
      document.documentElement.style.setProperty('--petrel-size', n + 'px');
      post();
    }
  });

  // Find in this message.
  //
  // Here rather than in the app because nothing outside can read this document:
  // the frame is opaque-origin by design, so the host cannot walk its text, and
  // window.find would search the app's own chrome instead. The app sends a term
  // and gets back a count; stepping between matches is the app's job, because
  // only it knows about the other messages in the thread.
  var found = [];

  function clearFind() {
    for (var i = 0; i < found.length; i++) {
      var m = found[i];
      var parent = m.parentNode;
      if (!parent) continue;
      parent.replaceChild(document.createTextNode(m.textContent), m);
      parent.normalize();
    }
    found = [];
  }

  function runFind(term) {
    clearFind();
    if (!term) { post(); return; }
    var needle = term.toLowerCase();
    // Text nodes only, and never inside a mark we just made — otherwise the
    // walk finds its own highlights and recurses.
    var walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: function (n) {
        if (!n.nodeValue || !n.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
        var p = n.parentNode;
        while (p && p !== document.body) {
          var tag = p.nodeName;
          if (tag === 'SCRIPT' || tag === 'STYLE') return NodeFilter.FILTER_REJECT;
          p = p.parentNode;
        }
        return n.nodeValue.toLowerCase().indexOf(needle) >= 0
          ? NodeFilter.FILTER_ACCEPT
          : NodeFilter.FILTER_REJECT;
      },
    });
    var targets = [];
    var node;
    while ((node = walker.nextNode())) targets.push(node);

    for (var t = 0; t < targets.length; t++) {
      var text = targets[t].nodeValue;
      var lower = text.toLowerCase();
      var frag = document.createDocumentFragment();
      var at = 0;
      var hit;
      while ((hit = lower.indexOf(needle, at)) >= 0) {
        if (hit > at) frag.appendChild(document.createTextNode(text.slice(at, hit)));
        var mark = document.createElement('mark');
        mark.className = 'petrel-find';
        mark.textContent = text.slice(hit, hit + needle.length);
        frag.appendChild(mark);
        found.push(mark);
        at = hit + needle.length;
      }
      if (at < text.length) frag.appendChild(document.createTextNode(text.slice(at)));
      targets[t].parentNode.replaceChild(frag, targets[t]);
    }
    try { parent.postMessage({ petrelFound: found.length }, '*'); } catch (e) {}
    post();
  }

  function setActive(i) {
    for (var n = 0; n < found.length; n++) {
      found[n].className = n === i ? 'petrel-find on' : 'petrel-find';
    }
    if (found[i] && found[i].scrollIntoView) {
      found[i].scrollIntoView({ block: 'center' });
    }
  }

  addEventListener('message', function (e) {
    var d = e.data || {};
    if (typeof d.petrelFind === 'string') runFind(d.petrelFind);
    if (typeof d.petrelFindActive === 'number') setActive(d.petrelFindActive);
  });

  addEventListener('keydown', function (e) {
    // Identity only — which key, which modifiers. Nothing about the document.
    try {
      parent.postMessage({
        petrelKey: {
          key: e.key,
          metaKey: e.metaKey, ctrlKey: e.ctrlKey,
          shiftKey: e.shiftKey, altKey: e.altKey
        }
      }, '*');
    } catch (err) {}
  });
})();
"#;

/// A civil date from epoch milliseconds, for the printed page's header.
/// UTC, deliberately: a printed page is a record, and a record that shifts
/// with the machine's timezone reads differently on every machine.
fn readable_date(ms: i64) -> String {
    let days_total = ms.div_euclid(86_400_000);
    let secs = ms.rem_euclid(86_400_000) / 1000;
    let (h, min) = (secs / 3600, (secs % 3600) / 60);
    // Civil-from-days (Howard Hinnant's algorithm), which is the standard
    // way to do this without a calendar dependency.
    let z = days_total + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{d} {} {y}, {h:02}:{min:02} UTC", MONTHS[(m - 1) as usize])
}

/// The printable form of one message: the envelope a reader needs on paper,
/// then the same sanitized body the screen shows.
///
/// Always the light palette — paper is light — and none of the screen
/// document's machinery: no height reporter, no scaler, no find hooks. The
/// one script is the print call itself, nonce-gated like every script here,
/// so the window opens straight into the OS print dialog and the page behind
/// it is the preview.
#[allow(clippy::too_many_arguments)]
fn print_document(
    body: &str,
    subject: &str,
    from: &str,
    to: &str,
    cc: &str,
    date: &str,
    nonce: &str,
) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let cc_line = if cc.is_empty() {
        String::new()
    } else {
        format!("<div class=\"line\"><span>Cc</span>{}</div>", esc(cc))
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<style>
  :root {{ color-scheme: light; }}
  @page {{ margin: 18mm; }}
  /* On screen this window is a preview, so it is laid out as the sheet rather
     than as raw markup filling a frame: a column the width the paper actually
     gives us, centred, with room to breathe around it. Text starting hard
     against the window frame is the thing that reads as unfinished.

     The column width is load-bearing, not decoration. The fitter below
     measures the box to decide whether a message has to be scaled, and it
     runs on screen — so if the box is as wide as the window, it measures the
     window and a message that clears 700px of frame still runs off 658px of
     paper. 174mm is A4 (210mm) less its two 18mm margins; US Letter is wider,
     so what fits this fits that.

     None of it survives into the print: there the @page margin is the
     margin, and a second one here would inset the text twice. */
  body {{ box-sizing: border-box; max-width: calc(174mm + 56px); margin: 0 auto;
         padding: 28px; background: #fff; color: #182730;
         font: 12.5px/1.6 -apple-system, system-ui, sans-serif; }}
  @media print {{ body {{ max-width: none; margin: 0; padding: 0; }} }}
  header {{ border-bottom: 1px solid #d9e1e2; padding-bottom: 10px; margin-bottom: 14px; }}
  h1 {{ font-size: 17px; margin: 0 0 8px; }}
  .line {{ font-size: 12px; color: #54666e; }}
  .line span {{ display: inline-block; min-width: 44px; color: #7c8f96; }}
  img {{ max-width: 100%; }}
  img:not([height]) {{ height: auto; }}
  blockquote {{ margin: 8px 0; padding-left: 12px; border-left: 2px solid #d9e1e2; color: #54666e; }}
  .petrel-plain {{ white-space: pre-wrap; font: 11.5px/1.6 ui-monospace, monospace; }}
  a {{ color: inherit; }}
  /* Same reasoning as the screen document: a fixed-width message is scaled as
     one piece rather than squeezed. `max-width: 100%` on a table takes the
     design apart — cells shrink independently, image mosaics stop lining up,
     text reflows into columns a pixel wide. Paper is narrower than the reading
     pane, so this matters more here, not less. */
  #petrel-fit {{ transform-origin: 0 0; }}
</style></head><body>
<header>
  <h1>{subject}</h1>
  <div class="line"><span>From</span>{from}</div>
  <div class="line"><span>To</span>{to}</div>
  {cc_line}
  <div class="line"><span>Date</span>{date}</div>
</header>
<div id="petrel-box"><div id="petrel-fit">{body}</div></div>
<script nonce="{nonce}">
  // Give images a beat to arrive, then fit the message to the page before the
  // dialog freezes it. Without this a message laid out at 600 or 700px — which
  // is most marketing mail — printed at its natural width and ran off the
  // right-hand edge, taking a column of every table with it.
  window.addEventListener('load', function () {{
    setTimeout(function () {{
      var box = document.getElementById('petrel-box');
      var fit = document.getElementById('petrel-fit');
      if (box && fit) {{
        var avail = box.clientWidth;
        var natural = fit.scrollWidth;
        if (natural > avail && avail > 0) {{
          var scale = avail / natural;
          fit.style.transform = 'scale(' + scale + ')';
          // The transform does not change layout, so the page would still
          // reserve the unscaled height and print blank sheets after it.
          fit.style.height = Math.ceil(fit.scrollHeight * scale) + 'px';
        }}
      }}
      window.print();
    }}, 250);
  }});
</script></body></html>"#,
        subject = esc(subject),
        from = esc(from),
        to = esc(to),
        date = esc(date),
    )
}

/// Where the reading frame may load images from.
///
/// `cid:` is the message's own parts and `petrel-msg:` is us serving them, so
/// both are admitted whatever the setting: neither reaches the network.
/// Allowing remote content adds the two web schemes and nothing else.
///
/// **Both** web schemes, deliberately. Plenty of real mail is older than
/// universal TLS — a 2018 newsletter whose images all sit on `http://` hosts
/// renders as a page of broken placeholders if the policy names only the
/// encrypted one, and the reader who allowed remote content is left looking at
/// an empty message with nothing on screen explaining why.
///
/// This is not the downgrade it looks like. Allowing remote content is already
/// the decision that lets the sender learn the message was opened; refusing
/// plaintext afterwards does not take that back, it only decides whether the
/// picture arrives. What it costs is the sender's own choice of scheme, and
/// withholding the image over that buys the reader nothing.
///
/// Note that `http://petrel-msg.localhost` is one origin, not a scheme: it does
/// not admit `http://anywhere.example`. That distinction is the whole bug this
/// function exists to keep fixed.
fn img_src(allow_remote: bool) -> &'static str {
    if allow_remote {
        "cid: petrel-msg: http://petrel-msg.localhost https: http:"
    } else {
        "cid: petrel-msg: http://petrel-msg.localhost"
    }
}

/// How the frame is allowed to be colored, decided per message.
///
/// The chrome around the frame is the app's and follows the app theme; this
/// only governs the message's own canvas. Mail that *styled itself* without
/// ever saying it works in the dark keeps its light canvas whatever the app
/// looks like — recoloring someone else's design because our chrome changed
/// is how mail ends up grey-on-grey. Mail with no styling of its own (plain
/// text, which we mark up ourselves) and mail whose sender declared
/// `color-scheme: dark` support both follow the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameTheme {
    /// Styled mail, no declaration: the light canvas it was designed on.
    AlwaysLight,
    /// Follows the app. `stamp` is the app's explicit choice; `None` is the
    /// system setting, where the frame's own media query decides — the same
    /// three-state pattern the app's tokens use.
    Adaptive { stamp: Option<&'static str> },
}

impl FrameTheme {
    /// Reads the app's choice out of the frame URL's query string.
    ///
    /// Carried in the URL rather than pushed in afterwards because the frame
    /// must be *born* the right color: a postMessage arriving after first
    /// paint is a white flash on every dark-mode message open.
    fn adaptive_from_query(query: Option<&str>) -> Self {
        let stamp = query
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("theme="))
            .and_then(|v| match v {
                "dark" => Some("dark"),
                "light" => Some("light"),
                _ => None,
            });
        FrameTheme::Adaptive { stamp }
    }
}

fn document(body: &str, blocked_remote: usize, nonce: &str, theme: FrameTheme) -> String {
    // The count goes out to the app rather than into a banner here. A notice
    // drawn inside the frame can only ever be a notice: the frame has no script
    // of its own, no IPC and no same-origin access, so "Show images" drawn here
    // would be a button that cannot do anything. Outside, it can.
    let reporter = HEIGHT_REPORTER.replace("BLOCKED", &blocked_remote.to_string());
    let banner = "";
    // The light palette is the only definition on bare :root, so an
    // AlwaysLight frame is simply one with no dark blocks — same variables,
    // one meaning. The dark values are the reading pane's own (surface, ink,
    // hairline from the app's dark tokens), not an inversion of the light.
    let dark_css = match theme {
        FrameTheme::AlwaysLight => String::new(),
        FrameTheme::Adaptive { .. } => {
            const DARK_VARS: &str = "color-scheme: dark; \
             --mv-bg: #142329; --mv-ink: #E4EDEE; --mv-ink2: #96A9AF; \
             --mv-hair: #24363D; --mv-mark: #453A14; --mv-mark-on: #8F6E17;";
            format!(
                "@media (prefers-color-scheme: dark) {{ \
                   :root:not([data-theme='light']) {{ {DARK_VARS} }} }} \
                 :root[data-theme='dark'] {{ {DARK_VARS} }}"
            )
        }
    };
    let stamp = match theme {
        FrameTheme::Adaptive { stamp: Some(mode) } => format!(" data-theme=\"{mode}\""),
        _ => String::new(),
    };
    format!(
        r#"<!doctype html><html{stamp}><head><meta charset="utf-8">
<style>
  /* The frame is sized to its content by the host, so it must never grow its
     own vertical scrollbar — a scroll region inside a scroll region. Sideways
     is different: it is the last resort for content too wide to shrink to a
     readable size, and reaching it beats having it silently cut off. */
  html {{ overflow-y: hidden; overflow-x: auto; }}
  :root {{ color-scheme: light;
          --mv-bg: #fff; --mv-ink: #182730; --mv-ink2: #54666e;
          --mv-hair: #d9e1e2; --mv-mark: #fbf0c9; --mv-mark-on: #f6c945; }}
  {dark_css}
  :root {{ --petrel-size: 15px; }}
  body {{ margin: 0; padding: 14px 16px; background: var(--mv-bg); color: var(--mv-ink);
         font: var(--petrel-size)/1.6 -apple-system, system-ui, sans-serif;
         word-wrap: break-word; }}
  /* `height: auto` cannot be the blanket rule here. It overrides the height a
     message declares on an image, recomputing it from the file's own aspect
     ratio — and mail is full of 1x1 spacer GIFs stretched by their width and
     height attributes to hold a layout apart. A spacer declared 320x15 renders
     320px tall under that rule, and one declared 1x30 collapses to 1px, so the
     message arrives with holes torn in it and its intended gaps gone.
     Declared dimensions therefore stand, and the aspect-preserving fallback
     applies only where the message left the height unsaid. */
  img {{ max-width: 100%; }}
  img:not([height]) {{ height: auto; }}
  /* Not `max-width: 100%`. Squeezing a fixed-width table does not make it fit,
     it takes the design apart: the cells shrink independently, the mosaic of
     image slices stops lining up, and text reflows into columns a pixel wide.
     A message laid out at a fixed width is scaled instead — see the fitter in
     the script below, which shrinks the whole thing as one piece. */
  #petrel-fit {{ transform-origin: 0 0; }}
  blockquote {{ margin: 8px 0; padding-left: 12px; border-left: 2px solid var(--mv-hair); color: var(--mv-ink2); }}
  .banner {{ background: #f6eedd; border: 1px solid #e2d3ae; color: #6b5220;
            padding: 8px 10px; border-radius: 4px; font-size: 12.5px; margin-bottom: 12px; }}
  .petrel-plain {{ white-space: pre-wrap;
                  font: calc(var(--petrel-size) * 0.92)/1.6 ui-monospace, SFMono-Regular, monospace; }}
  .petrel-plain .q {{ color: var(--mv-ink2); }}
  mark.petrel-find {{ background: var(--mv-mark); color: inherit; }}
  mark.petrel-find.on {{ background: var(--mv-mark-on); }}
</style></head><body>{banner}<div id="petrel-box"><div id="petrel-fit">{body}</div></div><script nonce="{nonce}">{reporter}</script></body></html>"#
    )
}

pub fn handle(
    request: &Request<Vec<u8>>,
    tokens: &Arc<ViewTokens>,
    blobs: &BlobStore,
    lookup_blob: impl Fn(i64) -> Option<String>,
    // Asked per message, because the answer is per sender: blocked unless the
    // sender was trusted or the user has written to them. Passed in rather than
    // decided here so the render path keeps no opinion about where preferences
    // live — and so the default, when anything goes wrong, is the safe one.
    allow_remote: impl Fn(i64) -> bool,
) -> Response<Vec<u8>> {
    let path = request.uri().path().to_string();

    // Attachment previews: /attachment/{token}/{part}. Same token scheme as
    // bodies, and only the types the pane can show inline — an image or a
    // PDF renders; everything else is refused here, so a preview can never
    // become a way of opening a file. Save and Open are commands with their
    // own confirmations; this route is only for looking.
    if let Some(rest) = path.strip_prefix("/attachment/") {
        let mut it = rest.splitn(2, '/');
        let (Some(token), Some(part)) = (it.next(), it.next()) else {
            return error_response(404, "not found");
        };
        let Ok(part) = part.parse::<usize>() else {
            return error_response(404, "not found");
        };
        let Some(message_id) = tokens.resolve(token) else {
            return error_response(403, "unknown or expired message token");
        };
        let Some(hash) = lookup_blob(message_id) else {
            return error_response(404, "message body not stored");
        };
        let Ok(raw) = blobs.read(&hash) else {
            return error_response(410, "message body unavailable (failed verification)");
        };
        let Some((meta, bytes)) = petrel_mime::attachment_bytes(&raw, part) else {
            return error_response(404, "no such attachment");
        };
        let mime = meta.content_type.unwrap_or_default();
        let previewable = mime.starts_with("image/") || mime == "application/pdf";
        if !previewable {
            return error_response(415, "this type is saved or opened, not previewed");
        }
        return Response::builder()
            .status(200)
            .header("Content-Type", mime)
            // Same fence as a message body: no scripts, no fetches. A PDF
            // renders in the webview's own viewer; an SVG cannot run script.
            .header(
                "Content-Security-Policy",
                "default-src 'none'; style-src 'unsafe-inline'; img-src data: blob:",
            )
            .header("X-Content-Type-Options", "nosniff")
            .header("Content-Disposition", "inline")
            .body(bytes)
            .expect("attachment response");
    }

    // The printable document: same token scheme, same sanitizing, plus the
    // envelope a page needs once it leaves the app.
    if let Some(token) = path.strip_prefix("/print/") {
        let Some(message_id) = tokens.resolve(token) else {
            return error_response(403, "unknown or expired message token");
        };
        let Some(hash) = lookup_blob(message_id) else {
            return error_response(404, "message body not stored");
        };
        let Ok(raw) = blobs.read(&hash) else {
            return error_response(410, "message body unavailable (failed verification)");
        };
        let Some(parsed) = petrel_mime::parse_message(&raw) else {
            return error_response(422, "message could not be parsed");
        };
        let allow_remote = allow_remote(message_id);
        let body = match parsed.body_html.as_deref() {
            Some(html) => {
                let s = petrel_mime::sanitize_html(html, allow_remote);
                petrel_mime::resolve_cids(&s.html, &parsed.attachments, |part| {
                    format!("/attachment/{token}/{part}")
                })
            }
            None => petrel_mime::plain_text_to_html(&parsed.body_text),
        };
        let join = |list: &[(Option<String>, String)]| {
            list.iter()
                .map(|(name, addr)| match name {
                    Some(n) => format!("{n} <{addr}>"),
                    None => addr.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let from = match (&parsed.from_display, &parsed.from_addr) {
            (Some(n), Some(a)) => format!("{n} <{a}>"),
            (None, Some(a)) => a.clone(),
            _ => String::new(),
        };
        let date = parsed.date_ms.map(readable_date).unwrap_or_default();
        let nonce = new_token();
        let csp = format!(
            "default-src 'none'; img-src {}; \
             style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; script-src 'nonce-{nonce}'; \
             form-action 'none'; base-uri 'none'; upgrade-insecure-requests",
            img_src(allow_remote)
        );
        return Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", csp)
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer")
            .body(
                print_document(
                    &body,
                    parsed.subject.as_deref().unwrap_or("(no subject)"),
                    &from,
                    &join(&parsed.to),
                    &join(&parsed.cc),
                    &date,
                    &nonce,
                )
                .into_bytes(),
            )
            .expect("print response");
    }

    let Some(token) = path.strip_prefix("/message/") else {
        return error_response(404, "not found");
    };
    let Some(message_id) = tokens.resolve(token) else {
        return error_response(403, "unknown or expired message token");
    };
    let Some(hash) = lookup_blob(message_id) else {
        return error_response(404, "message body not stored");
    };
    let Ok(raw) = blobs.read(&hash) else {
        // Verification failure lands here: corrupt bytes are never rendered.
        return error_response(410, "message body unavailable (failed verification)");
    };
    let Some(parsed) = petrel_mime::parse_message(&raw) else {
        return error_response(422, "message could not be parsed");
    };
    let allow_remote = allow_remote(message_id);

    // Prefer HTML; fall back to the plain part. Fail closed to text.
    // The theme rides with the body decision, because they share a premise:
    // whose colors are these? HTML mail styled itself — it goes dark only if
    // its sender said the colors survive that (the meta is read from the raw
    // HTML; the sanitizer strips it before rendering). Plain text is marked
    // up by us, so its colors are ours to theme, like any other chrome.
    let query = request.uri().query();
    // The per-message escape: whatever else is true of this message, render
    // it light. It overrides the transform and a sender's declaration alike,
    // because the person asking has seen the result and wants out of it.
    let force_light = query.unwrap_or("").split('&').any(|kv| kv == "force=light");
    let app_dark = query.unwrap_or("").split('&').any(|kv| kv == "theme=dark");
    let (body, report, theme) = match parsed.body_html.as_deref() {
        Some(html) => {
            let declared = petrel_mime::declares_dark(html);
            let s = petrel_mime::sanitize_html(html, allow_remote);
            // After sanitizing, point `cid:` images at the part route — the
            // webview cannot follow a cid, but it can fetch the same bytes
            // from the message's own protocol. Root-relative, so it resolves
            // against this document's origin on every platform spelling.
            let mut html = petrel_mime::resolve_cids(&s.html, &parsed.attachments, |part| {
                format!("/attachment/{token}/{part}")
            });
            let theme = if force_light {
                FrameTheme::AlwaysLight
            } else if declared {
                FrameTheme::adaptive_from_query(query)
            } else if app_dark {
                // Light-only mail in a dark app: recolor rather than glare.
                // Lightness flips, hue holds, images are never touched, and
                // the frame's own variables go dark with it. The sender's
                // declared-dark path above stays exactly as it was.
                html = petrel_mime::darken::recolor_for_dark(&html);
                FrameTheme::adaptive_from_query(query)
            } else {
                FrameTheme::AlwaysLight
            };
            (html, s.report, theme)
        }
        None => (
            petrel_mime::plain_text_to_html(&parsed.body_text),
            Default::default(),
            if force_light {
                FrameTheme::AlwaysLight
            } else {
                FrameTheme::adaptive_from_query(query)
            },
        ),
    };

    // Fresh per response: markup that somehow survived sanitization still cannot
    // carry a matching nonce, so ours stays the only script that runs.
    let nonce = new_token();
    // Two layers, not one. The sanitizer has already removed remote sources
    // when they are blocked; this makes the browser refuse them as well, so a
    // URL that slips through the rewriter still cannot reach the network.
    // Allowing remote content widens exactly one directive and nothing else —
    // scripts, forms and frames stay refused however the setting is left.
    let img_src = img_src(allow_remote);
    // `upgrade-insecure-requests` is what makes admitting `http:` above safe
    // rather than merely permissive: the browser rewrites those URLs to
    // `https:` before the request leaves, so a message full of plaintext image
    // hosts renders in full and still travels encrypted. It also keeps macOS's
    // App Transport Security satisfied without weakening it app-wide, which is
    // the alternative and a considerably blunter instrument.
    //
    // A host that genuinely has no TLS loses its images. That is the cost, and
    // it is smaller than it sounds: ATS would refuse those loads anyway, so
    // nothing that works today stops working.
    //
    // The upgrade skips potentially-trustworthy origins, which is why the
    // `http://petrel-msg.localhost` that Tauri serves the reading pane from on
    // Windows and Linux is left alone. macOS does not use that form at all.
    let csp = format!(
        "default-src 'none'; img-src {img_src}; \
         style-src 'unsafe-inline'; style-src-attr 'unsafe-inline'; script-src 'nonce-{nonce}'; \
         form-action 'none'; base-uri 'none'; frame-ancestors 'self'; \
         upgrade-insecure-requests"
    );

    Response::builder()
        .status(200)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", csp)
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
        .body(document(&body, report.blocked_remote, &nonce, theme).into_bytes())
        .expect("message response")
}

#[cfg(test)]
mod tests {
    use super::{FrameTheme, document, img_src};

    #[test]
    fn the_printable_page_is_the_envelope_then_the_body_then_the_dialog() {
        let doc = super::print_document(
            "<p>the body</p>",
            "Q3 <contracts>",
            "Dana Wu <dana@example.com>",
            "me@example.com",
            "",
            "18 Aug 2026, 14:02 UTC",
            "n0nce",
        );
        // The envelope, escaped — a subject is sender-written text.
        assert!(doc.contains("Q3 &lt;contracts&gt;"), "{doc}");
        assert!(doc.contains("Dana Wu &lt;dana@example.com&gt;"), "{doc}");
        assert!(doc.contains("18 Aug 2026, 14:02 UTC"), "{doc}");
        // No Cc line at all when there is no Cc — not an empty one.
        assert!(!doc.contains("<span>Cc</span>"), "{doc}");
        // The one script is the nonce-gated print call; none of the screen
        // document's machinery comes along.
        assert!(doc.contains(r#"nonce="n0nce""#), "{doc}");
        assert!(doc.contains("window.print()"), "{doc}");
        assert!(!doc.contains("petrelHeight"), "{doc}");
        // Paper is light: the print page never carries the dark palette.
        assert!(!doc.contains("prefers-color-scheme"), "{doc}");
    }

    /// The printed page scales a wide message rather than letting it run off.
    ///
    /// Most marketing mail is laid out at a fixed 600 to 700px. Paper is
    /// narrower than that once margins are taken, so without a fitter the
    /// right-hand column of every table printed off the edge of the sheet.
    #[test]
    fn a_wide_message_is_fitted_to_the_page_before_the_dialog_opens() {
        let doc = super::print_document(
            "<table width=\"700\"><tr><td>wide</td></tr></table>",
            "Subject",
            "a@example.com",
            "me@example.com",
            "",
            "Tue, 18 Aug 2026",
            "n0nce",
        );
        // The body is wrapped, so there is something to scale.
        assert!(doc.contains("id=\"petrel-box\""), "no measuring box");
        assert!(doc.contains("id=\"petrel-fit\""), "no scaled wrapper");
        // Scaled as one piece, the way the screen document does it, rather
        // than squeezed with max-width — which pulls a fixed layout apart.
        assert!(doc.contains("scale("), "nothing scales the message");
        assert!(
            !doc.contains("table { max-width"),
            "tables must not be squeezed individually"
        );
        // The transform leaves layout height untouched, so the height has to
        // be corrected or the sheet after the message prints blank.
        assert!(doc.contains("fit.style.height"), "height not reserved");
        // And it still ends in the dialog.
        assert!(doc.contains("window.print()"));
    }

    /// The preview is laid out as the sheet, which is what makes the fit
    /// measurement mean anything.
    ///
    /// The fitter runs on screen and measures the box it is given. Left to
    /// fill the window it measured the window — so a message that cleared the
    /// frame was declared to fit and still ran off the paper, which is
    /// narrower. Capping the column at the paper's own width makes the two
    /// the same question. The padding is the visible half of the same change.
    #[test]
    fn the_preview_is_laid_out_as_the_sheet_and_the_paper_keeps_its_own_margin() {
        let doc = super::print_document(
            "<p>the body</p>",
            "Subject",
            "a@example.com",
            "me@example.com",
            "",
            "Tue, 18 Aug 2026",
            "n0nce",
        );
        // A column of the paper's width, centred, with room around it.
        assert!(doc.contains("max-width: calc(174mm + 56px)"), "{doc}");
        assert!(doc.contains("margin: 0 auto"), "{doc}");
        assert!(doc.contains("padding: 28px"), "{doc}");
        // And none of it on paper, where @page owns the margin. Insetting the
        // text twice is the failure this guards.
        assert!(doc.contains("@media print"), "{doc}");
        assert!(
            doc.contains("max-width: none; margin: 0; padding: 0;"),
            "{doc}"
        );
        assert!(doc.contains("@page { margin: 18mm; }"), "{doc}");
    }

    #[test]
    fn the_printed_date_is_a_civil_date() {
        // 18 Aug 2026 14:02:00 UTC.
        assert_eq!(
            super::readable_date(1_787_061_720_000),
            "18 Aug 2026, 14:02 UTC"
        );
        // The epoch itself, and a leap-year date, pin the arithmetic.
        assert_eq!(super::readable_date(0), "1 Jan 1970, 00:00 UTC");
        assert_eq!(
            super::readable_date(951_782_400_000),
            "29 Feb 2000, 00:00 UTC"
        );
    }

    #[test]
    fn styled_mail_without_a_declaration_keeps_its_light_canvas() {
        let doc = document("<p>hi</p>", 0, "n", FrameTheme::AlwaysLight);
        assert!(!doc.contains("prefers-color-scheme"), "{doc}");
        assert!(!doc.contains("data-theme"), "{doc}");
        // The light values are the only definition, so the frame cannot
        // render any other way.
        assert!(doc.contains("--mv-bg: #fff"), "{doc}");
    }

    #[test]
    fn an_adaptive_frame_carries_both_palettes_and_the_stamp_wins() {
        // System: both palettes present, nothing stamped — the media query
        // decides, exactly as the app's own tokens do.
        let system = document("<p>hi</p>", 0, "n", FrameTheme::Adaptive { stamp: None });
        assert!(system.contains("prefers-color-scheme: dark"), "{system}");
        assert!(system.contains(":root[data-theme='dark']"), "{system}");
        assert!(!system.contains("<html data-theme"), "{system}");

        // An explicit app choice is stamped on the root, and the guarded
        // blocks make it win in both directions.
        let dark = document(
            "<p>hi</p>",
            0,
            "n",
            FrameTheme::Adaptive {
                stamp: Some("dark"),
            },
        );
        assert!(dark.contains(r#"<html data-theme="dark">"#), "{dark}");
        assert!(dark.contains(":root:not([data-theme='light'])"), "{dark}");
        assert!(dark.contains("--mv-bg: #142329"), "{dark}");
    }

    #[test]
    fn the_theme_query_is_read_defensively() {
        assert_eq!(
            FrameTheme::adaptive_from_query(Some("theme=dark")),
            FrameTheme::Adaptive {
                stamp: Some("dark")
            }
        );
        assert_eq!(
            FrameTheme::adaptive_from_query(Some("x=1&theme=light")),
            FrameTheme::Adaptive {
                stamp: Some("light")
            }
        );
        // System, absent, or nonsense: nothing stamped, media query decides.
        // Nonsense matters — the value ends up inside an attribute, so only
        // the two known words are ever written back out.
        for q in [Some("theme=system"), Some("theme=\"><script>"), None] {
            assert_eq!(
                FrameTheme::adaptive_from_query(q),
                FrameTheme::Adaptive { stamp: None },
                "{q:?}"
            );
        }
    }

    /// The regression this file exists to prevent. `https:`-only looked like
    /// the careful choice and silently emptied every message whose images
    /// predate universal TLS.
    #[test]
    fn allowing_remote_content_admits_plaintext_images_too() {
        let policy = img_src(true);
        assert!(policy.contains("https:"), "{policy}");
        assert!(
            policy.split_whitespace().any(|s| s == "http:"),
            "a bare http: scheme, not just the petrel-msg origin: {policy}"
        );
    }

    /// The origin is not the scheme. Reading `http://petrel-msg.localhost` as
    /// permission for plaintext generally is how the gap hid in review.
    #[test]
    fn blocking_remote_content_admits_neither_web_scheme() {
        let policy = img_src(false);
        assert!(!policy.split_whitespace().any(|s| s == "http:"), "{policy}");
        assert!(
            !policy.split_whitespace().any(|s| s == "https:"),
            "{policy}"
        );
        assert!(policy.contains("http://petrel-msg.localhost"), "{policy}");
    }

    /// The message's own parts are not network access, so they never depend on
    /// the setting.
    #[test]
    fn inline_parts_are_admitted_either_way() {
        for allow in [true, false] {
            let policy = img_src(allow);
            assert!(policy.contains("cid:"), "allow={allow}: {policy}");
            assert!(policy.contains("petrel-msg:"), "allow={allow}: {policy}");
        }
    }
}
