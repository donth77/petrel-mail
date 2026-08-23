//! The search field's grammar.
//!
//! The rule that shapes all of this: the field holds the whole query, and an
//! operator nobody recognises stays in the text rather than being rejected. A
//! field that argues with what you typed is worse than one that searches for
//! it — and "re:pricing" is a subject line far more often than a failed
//! attempt at an operator.

use petrel_engine::search_query::parse;

#[test]
fn words_alone_are_just_words() {
    let q = parse("annex pricing");
    assert_eq!(q.text, "annex pricing");
    assert!(q.from.is_none());
    assert!(!q.has_attachment);
}

#[test]
fn operators_come_out_and_the_words_stay() {
    let q = parse("from:sam has:attachment annex");
    assert_eq!(q.text, "annex");
    assert_eq!(q.from.as_deref(), Some("sam"));
    assert!(q.has_attachment);
}

#[test]
fn a_quoted_value_holds_together() {
    let q = parse(r#"from:"Dana Wu" contract"#);
    assert_eq!(q.from.as_deref(), Some("Dana Wu"));
    assert_eq!(q.text, "contract");
}

#[test]
fn conditions_need_no_words() {
    let q = parse("has:attachment");
    assert!(q.has_attachment);
    assert!(q.text.is_empty());
    assert!(
        q.conditions_only(),
        "a condition on its own is a real search"
    );
    assert!(!q.is_empty());
}

#[test]
fn nothing_is_nothing() {
    assert!(parse("").is_empty());
    assert!(parse("   ").is_empty());
}

/* The forgiving half, and the reason it matters. */
#[test]
fn an_operator_we_do_not_know_is_searched_for() {
    let q = parse("re:pricing");
    assert_eq!(
        q.text, "re:pricing",
        "a subject line, not a broken operator"
    );

    let half = parse("from:");
    assert_eq!(half.text, "from:", "mid-typing is not an error");
    assert!(half.from.is_none());
}

#[test]
fn a_year_that_is_not_a_year_stays_text() {
    let q = parse("after:soon");
    assert!(q.after_ms.is_none());
    assert_eq!(q.text, "after:soon");
}

#[test]
fn after_a_year_is_the_first_of_january() {
    let q = parse("after:2026");
    // 2026-01-01T00:00:00Z, checked against a known value rather than the same
    // arithmetic the code uses.
    assert_eq!(q.after_ms, Some(1_767_225_600_000));
}

#[test]
fn is_read_cancels_unread_rather_than_asking_for_both() {
    let q = parse("is:unread is:read");
    assert!(!q.unread);
}

#[test]
fn operators_are_case_insensitive_but_values_are_not_mangled() {
    let q = parse("FROM:Sam HAS:Attachment IS:Starred");
    assert_eq!(q.from.as_deref(), Some("Sam"), "the name keeps its case");
    assert!(q.has_attachment);
    assert!(q.starred);
}

#[test]
fn a_mailbox_can_be_named() {
    assert_eq!(parse("in:Sent").in_role.as_deref(), Some("sent"));
}
