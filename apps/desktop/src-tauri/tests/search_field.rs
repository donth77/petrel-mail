//! The search field says when a query was cut short.
//!
//! The engine stops reading a query past its limits and says so only to
//! itself, so the field reads the query the same way and counts. It keeps
//! its own copy of the two limits to do that, in `search-limits.ts`. If the
//! engine's changed and the copy did not, the field would announce a cut the
//! engine never made, or stay quiet about one it did.

use petrel_engine::search_query::{MAX_CLAUSES, MAX_VALUE_CHARS};

#[test]
fn the_field_counts_to_the_limits_the_engine_holds_a_query_to() {
    let field = include_str!("../../ui/src/lib/search-limits.ts");
    for (name, value) in [
        ("MAX_CLAUSES", MAX_CLAUSES),
        ("MAX_VALUE_CHARS", MAX_VALUE_CHARS),
    ] {
        let line = format!("export const {name} = {value};");
        assert!(
            field.contains(&line),
            "search-limits.ts has to say `{line}`, as search_query.rs does"
        );
    }
}

/// What the field predicts, the engine has to do.
///
/// The window reads a query before the engine ever sees it: to light the
/// chips that match it, to mark the words a result was found by, and to say
/// when it was cut short. That reading is a second implementation of the
/// grammar (`search-grammar.ts`), and a second implementation drifts. The
/// cases live beside it, in `search-grammar.cases.json`, and are asserted
/// from both sides: the field's own suite checks them against its reading,
/// and this checks them against the engine's.
mod field_cases {
    use petrel_engine::search_query::{Clause, Expr, SearchQuery, State, Term, parse};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Cases {
        cases: Vec<Case>,
        long: Long,
        many: Many,
    }

    #[derive(Deserialize)]
    struct Case {
        query: String,
        reads: Vec<String>,
        cut: Option<String>,
    }

    #[derive(Deserialize)]
    struct Long {
        cases: Vec<LongCase>,
    }

    #[derive(Deserialize)]
    struct LongCase {
        prefix: String,
        value: String,
        times: usize,
        cut: Option<String>,
    }

    #[derive(Deserialize)]
    struct Many {
        cases: Vec<ManyCase>,
    }

    #[derive(Deserialize)]
    struct ManyCase {
        words: usize,
        cut: Option<String>,
    }

    fn cases() -> Cases {
        serde_json::from_str(include_str!("../../ui/src/lib/search-grammar.cases.json"))
            .expect("the cases the field and the engine share")
    }

    /// One term, written the way the cases write it.
    fn reads(clause: &Clause) -> String {
        let minus = if clause.negated { "-" } else { "" };
        let said = match &clause.term {
            Term::Text(text) => format!("text:{}", text.value),
            Term::Subject(text) => format!("subject:{}", text.value),
            Term::From(value) => format!("from:{value}"),
            Term::To(value) => format!("to:{value}"),
            Term::Cc(value) => format!("cc:{value}"),
            Term::In(value) => format!("in:{value}"),
            Term::Tag(value) => format!("tag:{value}"),
            Term::Filename(value) => format!("filename:{value}"),
            Term::Is(State::Unread) => "is:unread".into(),
            Term::Is(State::Read) => "is:read".into(),
            Term::Is(State::Starred) => "is:starred".into(),
            Term::Is(State::Snoozed) => "is:snoozed".into(),
            Term::HasAttachment => "has:attachment".into(),
            Term::After(when) => format!("after:{when}"),
            Term::Before(when) => format!("before:{when}"),
            Term::On(when) => format!("date:{when}"),
        };
        format!("{minus}{said}")
    }

    /// Every term of a query, in the order it was read.
    fn all(q: &SearchQuery) -> Vec<String> {
        fn walk(expr: &Expr, out: &mut Vec<String>) {
            match expr {
                Expr::Clause(clause) => out.push(reads(clause)),
                Expr::Not(inner) => walk(inner, out),
                Expr::All(parts) | Expr::Any(parts) => parts.iter().for_each(|p| walk(p, out)),
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &q.root {
            walk(root, &mut out);
        }
        out
    }

    #[test]
    fn the_engine_reads_a_query_the_way_the_field_says_it_will() {
        for case in cases().cases {
            let q = parse(&case.query);
            assert_eq!(all(&q), case.reads, "{:?}", case.query);
            assert_eq!(
                q.truncated,
                case.cut.is_some(),
                "{:?} is cut short or is not",
                case.query
            );
        }
    }

    #[test]
    fn the_engine_cuts_the_queries_the_field_says_it_will() {
        for case in cases().long.cases {
            let query = format!("{}{}", case.prefix, case.value.repeat(case.times));
            assert_eq!(
                parse(&query).truncated,
                case.cut.is_some(),
                "{}{} × {}",
                case.prefix,
                case.value,
                case.times
            );
        }
        for case in cases().many.cases {
            let query = (0..case.words)
                .map(|i| format!("w{i}"))
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                parse(&query).truncated,
                case.cut.is_some(),
                "{} words",
                case.words
            );
        }
    }
}
