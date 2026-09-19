//! The search field's grammar.
//!
//! The rule that shapes all of this: the field holds the whole query, and an
//! operator nobody recognises stays in the text rather than being rejected. A
//! field that argues with what you typed is worse than one that searches for
//! it — and "re:pricing" is a subject line far more often than a failed
//! attempt at an operator.
//!
//! The operators are the table in docs 07 §5.1. Everything that table
//! promises is asserted here as a shape, and in `search_grammar.rs` as a
//! result.

use petrel_engine::search_query::{
    Clause, Expr, MAX_CLAUSES, MAX_DEPTH, MAX_VALUE_CHARS, Period, SearchQuery, State, Term, Text,
    parse,
};

fn word(value: &str) -> Term {
    Term::Text(Text {
        value: value.into(),
        exact: false,
    })
}

fn phrase(value: &str) -> Term {
    Term::Text(Text {
        value: value.into(),
        exact: true,
    })
}

fn yes(term: Term) -> Clause {
    Clause {
        negated: false,
        term,
    }
}

fn no(term: Term) -> Clause {
    Clause {
        negated: true,
        term,
    }
}

/// Clauses that all have to hold, however many there are.
fn all(expr: Expr) -> Vec<Clause> {
    match expr {
        Expr::Clause(clause) => vec![clause],
        Expr::All(parts) => parts
            .into_iter()
            .map(|part| match part {
                Expr::Clause(clause) => clause,
                other => panic!("a clause was expected, not {other:?}"),
            })
            .collect(),
        other => panic!("clauses were expected, not {other:?}"),
    }
}

/// The clauses of a query that has no `OR` and no brackets in it.
fn clauses(input: &str) -> Vec<Clause> {
    all(parse(input)
        .root
        .unwrap_or_else(|| panic!("{input:?} asked nothing")))
}

/// What `OR` separates, at the top of a query.
fn alternatives(input: &str) -> Vec<Vec<Clause>> {
    match parse(input).root {
        Some(Expr::Any(parts)) => parts.into_iter().map(all).collect(),
        Some(one) => vec![all(one)],
        None => Vec::new(),
    }
}

fn leaf(clause: Clause) -> Expr {
    Expr::Clause(clause)
}

#[test]
fn words_alone_are_just_words() {
    assert_eq!(
        clauses("annex pricing"),
        [yes(word("annex")), yes(word("pricing"))]
    );
}

#[test]
fn operators_come_out_and_the_words_stay() {
    assert_eq!(
        clauses("from:sam has:attachment annex"),
        [
            yes(Term::From("sam".into())),
            yes(Term::HasAttachment),
            yes(word("annex")),
        ]
    );
}

#[test]
fn a_quoted_value_holds_together() {
    assert_eq!(
        clauses(r#"from:"Dana Wu" contract"#),
        [yes(Term::From("Dana Wu".into())), yes(word("contract"))]
    );
    assert_eq!(
        clauses(r#"in:"Contracts/2026""#),
        [yes(Term::In("contracts/2026".into()))]
    );
}

#[test]
fn conditions_need_no_words() {
    let q = parse("has:attachment");
    assert!(!q.is_empty(), "a condition on its own is a real search");
    assert_eq!(clauses("has:attachment"), [yes(Term::HasAttachment)]);
}

#[test]
fn nothing_is_nothing() {
    assert!(parse("").is_empty());
    assert!(parse("   ").is_empty());
    assert!(parse("\"\"").is_empty(), "an empty pair of quotes");
    assert!(
        parse("OR").is_empty(),
        "a separator with nothing to separate"
    );
}

/// Every operator in the 07 §5.1 table, in the form the table gives it.
#[test]
fn every_operator_in_the_table_is_read() {
    let day = |y, m, d| Period::day(y, m, d).unwrap();
    for (typed, term) in [
        ("from:sam", Term::From("sam".into())),
        (
            "from:@vendorco.example",
            Term::From("@vendorco.example".into()),
        ),
        ("to:dana", Term::To("dana".into())),
        ("cc:legal", Term::Cc("legal".into())),
        (
            "subject:invoice",
            Term::Subject(Text {
                value: "invoice".into(),
                exact: false,
            }),
        ),
        ("in:archive", Term::In("archive".into())),
        ("is:unread", Term::Is(State::Unread)),
        ("is:read", Term::Is(State::Read)),
        ("is:starred", Term::Is(State::Starred)),
        ("is:flagged", Term::Is(State::Starred)),
        ("is:snoozed", Term::Is(State::Snoozed)),
        ("has:attachment", Term::HasAttachment),
        ("has:attachments", Term::HasAttachment),
        ("has:file", Term::HasAttachment),
        ("filename:.pdf", Term::Filename(".pdf".into())),
        ("tag:urgent", Term::Tag("urgent".into())),
        ("after:2026-06-01", Term::After(day(2026, 6, 1))),
        ("before:2026-06-01", Term::Before(day(2026, 6, 1))),
        ("date:2026-08-14", Term::On(day(2026, 8, 14))),
    ] {
        assert_eq!(clauses(typed), [yes(term.clone())], "{typed}");
        // And `-` in front of any of them excludes it.
        assert_eq!(clauses(&format!("-{typed}")), [no(term)], "-{typed}");
    }
}

#[test]
fn a_subject_can_be_a_phrase() {
    assert_eq!(
        clauses(r#"subject:"board pack""#),
        [yes(Term::Subject(Text {
            value: "board pack".into(),
            exact: true,
        }))]
    );
}

/* The forgiving half, and the reason it matters. */
#[test]
fn an_operator_we_do_not_know_is_searched_for() {
    assert_eq!(
        clauses("re:pricing"),
        [yes(word("re:pricing"))],
        "a subject line, not a broken operator"
    );
    assert_eq!(
        clauses("from:"),
        [yes(word("from:"))],
        "mid-typing is not an error"
    );
    assert_eq!(clauses("is:whatever"), [yes(word("is:whatever"))]);
    assert_eq!(clauses("has:cheese"), [yes(word("has:cheese"))]);
}

#[test]
fn an_operator_inside_quotes_is_the_words_it_looks_like() {
    assert_eq!(clauses(r#""from:sam""#), [yes(phrase("from:sam"))]);
}

#[test]
fn a_date_that_is_not_a_date_stays_text() {
    for typed in [
        "after:soon",
        "after:26",
        "after:2026-13",
        "after:2026-02-30",
        "date:2026-06-01-02",
        "before:1969",
        "after:20260601",
    ] {
        assert_eq!(clauses(typed), [yes(word(typed))], "{typed}");
    }
}

#[test]
fn a_date_is_a_year_a_month_or_a_day() {
    assert_eq!(
        clauses("after:2026"),
        [yes(Term::After(Period::year(2026).unwrap()))]
    );
    assert_eq!(
        clauses("date:2026-08"),
        [yes(Term::On(Period::month(2026, 8).unwrap()))]
    );
    // What people arriving from Gmail type, and a month without its zero.
    assert_eq!(
        clauses("after:2026/6/1"),
        [yes(Term::After(Period::day(2026, 6, 1).unwrap()))]
    );
    // A leap day exists in the years that have one.
    assert!(Period::day(2028, 2, 29).is_some());
    assert!(Period::day(2026, 2, 29).is_none());
}

/// Typing `after:2026-08-14` passes through `after:2026-` and
/// `after:2026-08-`. Those are the date so far, not a search for the word
/// "after", or the results would blink out twice on the way.
#[test]
fn a_date_still_being_typed_is_the_date_so_far() {
    assert_eq!(
        clauses("after:2026-"),
        [yes(Term::After(Period::year(2026).unwrap()))]
    );
    assert_eq!(
        clauses("after:2026-08-"),
        [yes(Term::After(Period::month(2026, 8).unwrap()))]
    );
}

#[test]
fn a_period_knows_its_first_day_and_the_day_after_it() {
    let days = |p: Period| {
        let (a, b) = (p.first_day(), p.day_after());
        ((a.year, a.month, a.day), (b.year, b.month, b.day))
    };
    assert_eq!(
        days(Period::year(2026).unwrap()),
        ((2026, 1, 1), (2027, 1, 1))
    );
    assert_eq!(
        days(Period::month(2026, 12).unwrap()),
        ((2026, 12, 1), (2027, 1, 1))
    );
    assert_eq!(
        days(Period::month(2026, 2).unwrap()),
        ((2026, 2, 1), (2026, 3, 1))
    );
    assert_eq!(
        days(Period::day(2026, 8, 14).unwrap()),
        ((2026, 8, 14), (2026, 8, 15))
    );
    assert_eq!(
        days(Period::day(2028, 2, 29).unwrap()),
        ((2028, 2, 29), (2028, 3, 1))
    );
    assert_eq!(
        days(Period::day(2026, 12, 31).unwrap()),
        ((2026, 12, 31), (2027, 1, 1))
    );
}

#[test]
fn a_day_in_utc_is_the_number_it_should_be() {
    // Checked against known values rather than the same arithmetic the code
    // uses: 2026-01-01T00:00:00Z, the epoch, and a leap day.
    let ms = |y, m, d| Period::day(y, m, d).unwrap().first_day().utc_ms();
    assert_eq!(ms(2026, 1, 1), 1_767_225_600_000);
    assert_eq!(ms(1970, 1, 1), 0);
    assert_eq!(ms(2024, 2, 29), 1_709_164_800_000);
    assert_eq!(ms(2026, 8, 18), 1_787_011_200_000);
}

/// A state of its own, not "whichever came last". With real negation in the
/// grammar, asking for both is asking for both.
#[test]
fn is_read_is_a_state_and_not_a_cancellation() {
    assert_eq!(clauses("is:read"), [yes(Term::Is(State::Read))]);
    assert_eq!(
        clauses("is:unread is:read"),
        [yes(Term::Is(State::Unread)), yes(Term::Is(State::Read))]
    );
}

#[test]
fn operators_are_case_insensitive_but_values_are_not_mangled() {
    assert_eq!(
        clauses("FROM:Sam HAS:Attachment IS:Starred"),
        [
            yes(Term::From("Sam".into())),
            yes(Term::HasAttachment),
            yes(Term::Is(State::Starred)),
        ],
        "the name keeps its case"
    );
}

#[test]
fn a_mailbox_can_be_named() {
    assert_eq!(clauses("in:Sent"), [yes(Term::In("sent".into()))]);
    // Lowercased the way the language does it, not only A to Z.
    assert_eq!(clauses("in:ÜBUNG"), [yes(Term::In("übung".into()))]);
}

/* Phrases, exclusion and OR: the forms Help documented and nothing read. */

#[test]
fn a_phrase_keeps_its_quotes() {
    assert_eq!(
        clauses(r#""board pack" annex"#),
        [yes(phrase("board pack")), yes(word("annex"))]
    );
    // Still being typed: the quote has not been closed yet.
    assert_eq!(clauses(r#""board pa"#), [yes(phrase("board pa"))]);
    // Curly quotes are what macOS hands over from half the places a phrase
    // gets copied from.
    assert_eq!(
        clauses("\u{201C}board pack\u{201D}"),
        [yes(phrase("board pack"))]
    );
    // However it was spaced.
    assert_eq!(clauses("\"board \t  pack\""), [yes(phrase("board pack"))]);
}

#[test]
fn a_leading_minus_excludes() {
    assert_eq!(
        clauses("contract -draft"),
        [yes(word("contract")), no(word("draft"))]
    );
    assert_eq!(clauses(r#"-"board pack""#), [no(phrase("board pack"))]);
    assert_eq!(
        clauses(r#"-from:"Dana Wu""#),
        [no(Term::From("Dana Wu".into()))]
    );
}

#[test]
fn a_hyphen_that_is_not_leading_is_part_of_the_word() {
    assert_eq!(clauses("e-mail"), [yes(word("e-mail"))]);
    assert_eq!(
        clauses("follow-up -e-mail"),
        [yes(word("follow-up")), no(word("e-mail"))]
    );
    // A dash on its own is a dash, and inside quotes it is text.
    assert_eq!(clauses("-"), [yes(word("-"))]);
    assert_eq!(clauses(r#""-5""#), [yes(phrase("-5"))]);
}

#[test]
fn or_separates_alternatives_and_binds_looser_than_and() {
    assert_eq!(
        alternatives("from:sam annex OR from:dana"),
        [
            vec![yes(Term::From("sam".into())), yes(word("annex"))],
            vec![yes(Term::From("dana".into()))],
        ]
    );
}

/// People type "or" in ordinary searches, and a query must not change its
/// meaning because of a conjunction. Only the capitals are the operator.
#[test]
fn only_a_capital_or_is_the_operator() {
    assert_eq!(
        clauses("now or never"),
        [yes(word("now")), yes(word("or")), yes(word("never"))]
    );
    assert_eq!(clauses("Or"), [yes(word("Or"))]);
    // And the word itself is still reachable.
    assert_eq!(
        clauses(r#""OR" theatre"#),
        [yes(phrase("OR")), yes(word("theatre"))]
    );
}

/// Mid-typing, `from:sam OR` is in the field for as long as it takes to type
/// the next word. It must go on meaning `from:sam`.
#[test]
fn a_dangling_or_is_ignored() {
    let sam = parse("from:sam");
    assert_eq!(parse("from:sam OR").root, sam.root);
    assert_eq!(parse("OR from:sam").root, sam.root);
    assert_eq!(parse("from:sam OR OR").root, sam.root);
    assert_eq!(alternatives("a OR OR b").len(), 2);
}

/* The limits. */

#[test]
fn a_query_stays_bounded_and_says_when_it_was_cut() {
    let fine = parse(&["word"; MAX_CLAUSES].join(" "));
    assert_eq!(all(fine.root.clone().unwrap()).len(), MAX_CLAUSES);
    assert!(!fine.truncated);

    let long = parse(&["word"; MAX_CLAUSES + 5].join(" "));
    assert_eq!(all(long.root.clone().unwrap()).len(), MAX_CLAUSES);
    assert!(long.truncated);

    // Brackets nest so far and no further. Deeper ones are read as if they
    // were not there, and ten thousand of them are no deeper than nine.
    let nested = |depth: usize| format!("{}word{}", "(".repeat(depth), ")".repeat(depth));
    assert!(!parse(&nested(MAX_DEPTH)).truncated);
    assert!(parse(&nested(MAX_DEPTH + 1)).truncated);
    assert_eq!(parse(&nested(MAX_DEPTH + 1)).root, parse("word").root);
    let absurd = format!("{}a OR b", "(".repeat(10_000));
    assert_eq!(parse(&absurd).root, parse("a OR b").root);

    let endless = "x".repeat(MAX_VALUE_CHARS * 4);
    let held = parse(&format!("from:{endless}"));
    assert!(held.truncated);
    assert_eq!(
        all(held.root.unwrap()),
        [yes(Term::From("x".repeat(MAX_VALUE_CHARS)))]
    );

    // Lowering can lengthen: `İ` becomes two characters. The limit holds
    // after it as well as before.
    let dotted = parse(&format!("in:{}", "İ".repeat(MAX_VALUE_CHARS)));
    assert!(dotted.truncated);
    let named = all(dotted.root.unwrap());
    let [
        Clause {
            term: Term::In(name),
            ..
        },
    ] = &named[..]
    else {
        panic!("one mailbox");
    };
    assert_eq!(name.chars().count(), MAX_VALUE_CHARS);
}

/// A run of NOTs is counted, not descended into. One call per NOT went as
/// deep as the field was long, and ten thousand pasted in overflowed the
/// stack, which aborts the process rather than failing a search. Read here on
/// a stack far smaller than any the app runs on, so that coming back is the
/// proof.
#[test]
fn a_long_run_of_nots_is_no_deeper_than_one() {
    let reader = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let run = |lead: &str, count: usize| format!("{}word", lead.repeat(count));
            assert_eq!(parse(&run("NOT ", 100_000)).root, parse("word").root);
            assert_eq!(parse(&run("NOT ", 100_001)).root, parse("-word").root);
            assert_eq!(parse(&run("-(", 100_000)).root, parse("word").root);
            assert_eq!(parse(&run("NOT( ", 100_001)).root, parse("-word").root);
        })
        .unwrap();
    reader.join().unwrap();
}

#[test]
fn control_characters_are_spacing() {
    assert_eq!(
        clauses("annex\u{0}pricing\u{7}"),
        [yes(word("annex")), yes(word("pricing"))]
    );
}

/* Reading back what was written. */

/// 07 §5.1's acceptance: every chip's output round-trips through the parser.
/// These are the tokens `search-chips.ts` writes — a role, a user folder with
/// a space in its name, the two state scopes, a sender with and without a
/// space, and the four fixed chips.
#[test]
fn every_chip_reads_back_as_the_clause_it_means() {
    for (token, term) in [
        ("in:inbox", Term::In("inbox".into())),
        ("in:trash", Term::In("trash".into())),
        (r#"in:"Client contact""#, Term::In("client contact".into())),
        ("is:starred", Term::Is(State::Starred)),
        ("is:snoozed", Term::Is(State::Snoozed)),
        ("from:Slack", Term::From("Slack".into())),
        (r#"from:"Dana Wu""#, Term::From("Dana Wu".into())),
        ("has:attachment", Term::HasAttachment),
        ("is:unread", Term::Is(State::Unread)),
        ("after:2026", Term::After(Period::year(2026).unwrap())),
    ] {
        let q = parse(token);
        assert_eq!(clauses(token), [yes(term)], "{token}");
        if !token.contains("Client") {
            // Written back exactly as the chip wrote it. The folder is the one
            // exception: `in:` is held lowercased, because the roles are.
            assert_eq!(q.to_string(), token);
        }
        assert_eq!(parse(&q.to_string()).root, q.root, "{token}");
    }
    for chip in ["attachment", "unread", "starred", "inbox"] {
        let token = petrel_engine::search_query::token_for(chip).unwrap();
        assert_eq!(parse(token).to_string(), token);
    }
}

#[test]
fn a_query_is_written_the_way_it_would_be_typed() {
    for typed in [
        "annex pricing",
        r#"from:"Dana Wu" contract -draft"#,
        r#""board pack" OR subject:"board pack""#,
        "from:sam OR from:dana",
        "after:2026-08-01 before:2026-09-01 rent",
        "date:2026-08 -is:read -has:attachment",
        r#"tag:"read later" filename:.pdf to:dana cc:legal"#,
        "re:pricing -wat:ever",
        r#""OR" "-5" "from:sam""#,
    ] {
        assert_eq!(parse(typed).to_string(), typed);
    }
    // What was typed loosely comes back in the one way it is written.
    assert_eq!(
        parse("FROM:sam   or  OR  IS:Flagged").to_string(),
        "from:sam or OR is:starred"
    );
    assert_eq!(parse("after:2026/6/1").to_string(), "after:2026-06-01");
    // However many minuses, it is one exclusion.
    assert_eq!(parse("--draft").to_string(), "-draft");
    assert_eq!(parse("---").to_string(), "---");
}

/// A word built by hand, rather than typed, still has to survive the field:
/// these would each mean something else if they went in bare.
#[test]
fn text_that_would_read_back_as_something_else_wears_quotes() {
    for value in [
        "OR",
        "AND",
        "NOT",
        "-draft",
        "from:sam",
        "two words",
        "(annex",
        "annex)",
    ] {
        let q = SearchQuery {
            root: Some(leaf(yes(word(value)))),
            ..SearchQuery::default()
        };
        assert_eq!(q.to_string(), format!("\"{value}\""));
        assert_eq!(clauses(&q.to_string()), [yes(phrase(value))]);
    }
}

/// Writing a query out and reading it back is the identity, for anything the
/// parser can produce. Pieces are drawn from a pool that covers every form,
/// by a generator with a fixed seed so a failure is the same failure twice.
#[test]
fn reading_back_is_the_identity() {
    const POOL: &[&str] = &[
        "annex",
        "pricing",
        "e-mail",
        "re:pricing",
        "from:",
        "is:whatever",
        "after:soon",
        "OR",
        "OR",
        "or",
        "-",
        "-draft",
        "--draft",
        r#""board pack""#,
        r#"-"board pack""#,
        r#""OR""#,
        r#""from:sam""#,
        r#""unclosed"#,
        "from:sam",
        "-from:sam",
        r#"from:"Dana Wu""#,
        "FROM:Sam",
        "to:dana",
        "cc:legal",
        "subject:invoice",
        r#"subject:"board pack""#,
        "-subject:draft",
        "in:Sent",
        r#"in:"Contracts/2026""#,
        "is:unread",
        "-is:read",
        "is:flagged",
        "has:file",
        "filename:.pdf",
        "tag:urgent",
        r#"tag:"read later""#,
        "after:2026",
        "before:2026-08",
        "date:2026/8/14",
        "after:2026-",
        "AND",
        "NOT",
        "(",
        "(",
        ")",
        ")",
        "-(",
        "NOT(",
        "(annex",
        "pricing)",
        "foo(bar)",
        "--(",
        // Values a bare `)` would cut short, or close a group with.
        r#"from:"a)""#,
        r#"in:"Archive (old)""#,
        r#"tag:"p(1)""#,
        r#"filename:"a))b""#,
        r#"subject:"f(x)""#,
        "tag:p(1)x",
        "東京",
        "-会議",
        "subject:契約",
        "\u{201C}curly quotes\u{201D}",
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |below: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % below as u64) as usize
    };
    for _ in 0..2_000 {
        let typed = (0..next(9))
            .map(|_| POOL[next(POOL.len())])
            .collect::<Vec<_>>()
            .join(" ");
        let first = parse(&typed);
        let written = first.to_string();
        let second = parse(&written);
        assert_eq!(
            second.root, first.root,
            "{typed:?} was written as {written:?}"
        );
        assert_eq!(
            second.to_string(),
            written,
            "and writing it again changes nothing"
        );
    }
}

/* Brackets, AND and NOT: Boolean search as everyone has met it. */

#[test]
fn brackets_group_and_the_order_is_brackets_not_and_or() {
    let sam = || leaf(yes(Term::From("sam".into())));
    let dana = || leaf(yes(Term::From("dana".into())));
    // Without brackets OR binds loosest…
    assert_eq!(
        parse("from:sam OR from:dana invoice").root,
        Some(Expr::Any(vec![
            sam(),
            Expr::All(vec![dana(), leaf(yes(word("invoice")))]),
        ]))
    );
    // …and with them it is the senders that are either-or.
    assert_eq!(
        parse("(from:sam OR from:dana) invoice").root,
        Some(Expr::All(vec![
            Expr::Any(vec![sam(), dana()]),
            leaf(yes(word("invoice"))),
        ]))
    );
    // NOT binds tighter than AND, which binds tighter than OR.
    assert_eq!(
        parse("a OR b AND NOT c").root,
        Some(Expr::Any(vec![
            leaf(yes(word("a"))),
            Expr::All(vec![leaf(yes(word("b"))), leaf(no(word("c")))]),
        ]))
    );
}

#[test]
fn and_is_the_space_said_aloud_and_not_is_the_minus() {
    assert_eq!(
        parse("invoice AND receipt").root,
        parse("invoice receipt").root
    );
    assert_eq!(
        parse("contract NOT draft").root,
        parse("contract -draft").root
    );
    assert_eq!(parse("NOT from:sam").root, parse("-from:sam").root);
    assert_eq!(parse("NOT NOT draft").root, parse("draft").root);
    assert_eq!(parse("NOT -draft").root, parse("draft").root);
}

#[test]
fn a_group_can_be_excluded() {
    let excluded = Some(Expr::All(vec![
        leaf(yes(word("contract"))),
        Expr::Not(Box::new(Expr::Any(vec![
            leaf(yes(word("draft"))),
            leaf(yes(word("wip"))),
        ]))),
    ]));
    assert_eq!(parse("contract -(draft OR wip)").root, excluded);
    assert_eq!(parse("contract NOT (draft OR wip)").root, excluded);
    // With no space, which is what half of everyone types.
    assert_eq!(parse("contract NOT(draft OR wip)").root, excluded);
    // Excluding a group of one is excluding the one.
    assert_eq!(
        parse("contract -(draft)").root,
        parse("contract -draft").root
    );
}

#[test]
fn brackets_that_change_nothing_leave_nothing_behind() {
    assert_eq!(parse("(annex)").root, parse("annex").root);
    assert_eq!(parse("a (b c)").root, parse("a b c").root);
    assert_eq!(parse("a OR (b OR c)").root, parse("a OR b OR c").root);
    assert_eq!(parse("((a OR b))").root, parse("a OR b").root);
    assert!(parse("()").is_empty());
    assert!(parse("( )").is_empty());
    assert_eq!(parse("annex ()").root, parse("annex").root);
}

/// The field is read on every keystroke, so every half-typed state has to
/// mean something sensible, and none may be an error.
#[test]
fn half_typed_boolean_is_what_it_says_so_far() {
    assert_eq!(
        parse("(from:sam OR from:dana").root,
        parse("(from:sam OR from:dana)").root
    );
    assert_eq!(parse("(from:sam OR").root, parse("from:sam").root);
    assert_eq!(parse("invoice AND").root, parse("invoice").root);
    assert_eq!(parse("invoice NOT").root, parse("invoice").root);
    assert_eq!(parse("invoice -(").root, parse("invoice").root);
    assert_eq!(parse("AND OR NOT").root, None);
    // A bracket that closes nothing closes one taken to open at the start.
    assert_eq!(parse("a OR b) c").root, parse("(a OR b) c").root);
    assert_eq!(parse(") annex").root, parse("annex").root);
    // And what follows it carries on from that group: an OR after it is
    // still an OR. A word that ends in a bracket is the usual way to meet
    // one, and it must not turn the rest of the query into AND.
    assert_eq!(parse("a) OR b").root, parse("a OR b").root);
    assert_eq!(
        parse("a OR b) OR c) d").root,
        parse("((a OR b) OR c) d").root
    );
    assert_eq!(alternatives("fn(x) OR draft memo").len(), 2);
    // A control character is spacing, so a bracket closes before one.
    assert_eq!(parse("(a OR b)\u{0}c").root, parse("(a OR b) c").root);
    // However many dashes, one exclusion, in front of a group as of a word.
    assert_eq!(parse("--(a OR b)").root, parse("-(a OR b)").root);
}

/// A value cut at the length limit reads back as itself, even when the cut
/// lands on a space or leaves a `)` at the end.
#[test]
fn a_value_cut_at_the_limit_still_reads_back() {
    let filler = "x".repeat(MAX_VALUE_CHARS - 1);
    for typed in [
        format!(r#""{filler} and more words""#),
        format!(r#"from:"{filler} and more words""#),
        format!("subject:{filler})and-more"),
        format!("{filler})and-more"),
    ] {
        let first = parse(&typed);
        assert!(first.truncated);
        let written = first.to_string();
        assert_eq!(parse(&written).root, first.root, "written as {written:?}");
    }
}

#[test]
fn a_bracket_inside_a_word_is_part_of_the_word() {
    assert_eq!(clauses("foo(bar)x"), [yes(word("foo(bar)x"))]);
    assert_eq!(clauses("fn(x)"), [yes(word("fn(x"))], "the last one closes");
    // In quotes a bracket is only ever text.
    assert_eq!(clauses(r#""(annex)""#), [yes(phrase("(annex)"))]);
    assert_eq!(
        clauses(r#"in:"Archive (old)""#),
        [yes(Term::In("archive (old)".into()))]
    );
    // Pasted prose: brackets around plain words group nothing that matters.
    assert_eq!(
        parse("ready for review (#142)").root,
        parse("ready for review #142").root
    );
}

#[test]
fn only_capitals_are_operators() {
    assert_eq!(
        clauses("this and that or not"),
        [
            yes(word("this")),
            yes(word("and")),
            yes(word("that")),
            yes(word("or")),
            yes(word("not")),
        ]
    );
    assert_eq!(clauses(r#""NOT" fade away"#)[0], yes(phrase("NOT")));
}

/// Capitals are operators whatever surrounds them. `DO NOT REPLY` reads as
/// Boolean, and the way to search for the words is to quote the keyword —
/// which the field offers as a chip, rather than the parser guessing.
#[test]
fn capitals_are_operators_whatever_is_around_them() {
    assert_eq!(parse("DO NOT REPLY").root, parse("DO -REPLY").root);
    assert_eq!(
        parse("TERMS AND CONDITIONS").root,
        parse("TERMS CONDITIONS").root
    );
    assert_eq!(alternatives("IBM OR HP").len(), 2);
    // What the chip writes: the keyword in quotes is the word.
    assert_eq!(
        clauses(r#"DO "NOT" REPLY"#),
        [yes(word("DO")), yes(phrase("NOT")), yes(word("REPLY"))]
    );
}

#[test]
fn a_tree_is_written_with_the_fewest_brackets_that_keep_its_meaning() {
    for (typed, written) in [
        (
            "(from:sam OR from:dana) invoice",
            "(from:sam OR from:dana) invoice",
        ),
        (
            "invoice (from:sam OR from:dana)",
            "invoice (from:sam OR from:dana)",
        ),
        ("(a b) OR (c d)", "a b OR c d"),
        ("a AND b", "a b"),
        ("contract NOT draft", "contract -draft"),
        ("contract NOT (draft OR wip)", "contract -(draft OR wip)"),
        ("NOT (a b)", "-(a b)"),
        ("((a OR b) c) OR d", "(a OR b) c OR d"),
        ("(a OR (b c)) d", "(a OR b c) d"),
    ] {
        let q = parse(typed);
        assert_eq!(q.to_string(), written, "{typed}");
        assert_eq!(parse(written).root, q.root, "{written} reads back");
    }
}
