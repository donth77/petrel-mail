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

fn document(body: &str, blocked_remote: usize, nonce: &str) -> String {
    // The count goes out to the app rather than into a banner here. A notice
    // drawn inside the frame can only ever be a notice: the frame has no script
    // of its own, no IPC and no same-origin access, so "Show images" drawn here
    // would be a button that cannot do anything. Outside, it can.
    let reporter = HEIGHT_REPORTER.replace("BLOCKED", &blocked_remote.to_string());
    let banner = "";
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<style>
  /* The frame is sized to its content by the host, so it must never grow its
     own vertical scrollbar — a scroll region inside a scroll region. Sideways
     is different: it is the last resort for content too wide to shrink to a
     readable size, and reaching it beats having it silently cut off. */
  html {{ overflow-y: hidden; overflow-x: auto; }}
  :root {{ color-scheme: light; }}
  :root {{ --petrel-size: 15px; }}
  body {{ margin: 0; padding: 14px 16px; background: #fff; color: #182730;
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
  blockquote {{ margin: 8px 0; padding-left: 12px; border-left: 2px solid #d9e1e2; color: #54666e; }}
  .banner {{ background: #f6eedd; border: 1px solid #e2d3ae; color: #6b5220;
            padding: 8px 10px; border-radius: 4px; font-size: 12.5px; margin-bottom: 12px; }}
  .petrel-plain {{ white-space: pre-wrap;
                  font: calc(var(--petrel-size) * 0.92)/1.6 ui-monospace, SFMono-Regular, monospace; }}
  .petrel-plain .q {{ color: #54666e; }}
  mark.petrel-find {{ background: #fbf0c9; color: inherit; }}
  mark.petrel-find.on {{ background: #f6c945; }}
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
    let (body, report) = match parsed.body_html.as_deref() {
        Some(html) => {
            let s = petrel_mime::sanitize_html(html, allow_remote);
            (s.html, s.report)
        }
        None => (
            petrel_mime::plain_text_to_html(&parsed.body_text),
            Default::default(),
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
        .body(document(&body, report.blocked_remote, &nonce).into_bytes())
        .expect("message response")
}

#[cfg(test)]
mod tests {
    use super::img_src;

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
