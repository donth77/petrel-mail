//! What a query asks for, worked out before any of it is SQL.
//!
//! Nothing here touches the database. A parsed query becomes a small tree of
//! things to find and things to be true (`Node`), the NOTs are pushed down to
//! its leaves, and what is left is sorted into statements: words that can
//! share one full-text expression, conditions that can share one predicate,
//! and the few queries that can share neither. Which index each statement is
//! asked of, which word is still being typed, and what a statement is allowed
//! to see are all decided here, where they can be read without a mailbox.
use super::*;

/// A term as the query finally asks for it, with every NOT around it applied.
#[derive(Clone, Copy)]
pub(super) struct Lit<'a> {
    pub(super) negated: bool,
    pub(super) term: &'a Term,
}

/// The query with its NOTs pushed down to the terms, so that what is left is
/// only "all of these" and "any of these".
pub(super) enum Node<'a> {
    Lit(Lit<'a>),
    All(Vec<Node<'a>>),
    Any(Vec<Node<'a>>),
}

pub(super) fn without_nots<'a>(expr: &'a Expr, flip: bool) -> Node<'a> {
    match expr {
        Expr::Clause(clause) => Node::Lit(Lit {
            negated: clause.negated != flip,
            term: &clause.term,
        }),
        Expr::Not(inner) => without_nots(inner, !flip),
        // Not (a and b) is (not a) or (not b), and the other way about.
        //
        // Flattened as it is built. The parser's tree is tidy, but taking a
        // NOT off `a OR NOT (b c)` leaves a choice inside a choice, and those
        // are one choice of three: read as two, the inner one counted as a
        // statement of its own and the query reached the limit sooner.
        Expr::All(parts) | Expr::Any(parts) => {
            let all = matches!(expr, Expr::All(_)) != flip;
            let mut flat = Vec::new();
            for part in parts {
                match (without_nots(part, flip), all) {
                    (Node::All(inner), true) | (Node::Any(inner), false) => flat.extend(inner),
                    (other, _) => flat.push(other),
                }
            }
            if all {
                Node::All(flat)
            } else {
                Node::Any(flat)
            }
        }
    }
}

/// Words to find, as a tree FTS5 can be asked in one expression.
#[derive(Clone)]
pub(super) enum Words<'a> {
    Leaf(Option<&'static str>, &'a Text),
    All(Vec<Words<'a>>),
    Any(Vec<Words<'a>>),
}

/// Conditions, as a tree SQL can be asked in one predicate.
#[derive(Clone)]
pub(super) enum Cond<'a> {
    Lit(Lit<'a>),
    /// Words that must be there, looked up on their own. Only a query too
    /// wide to ask a statement at a time is written this way (`as_lookups`).
    With(Option<&'static str>, &'a Text),
    /// Words that must not be there, looked up on their own and taken away.
    Without(Option<&'static str>, &'a Text),
    All(Vec<Cond<'a>>),
    Any(Vec<Cond<'a>>),
}

/// The words a term looks for, and the column it looks in.
pub(super) fn text_of(term: &Term) -> Option<(Option<&'static str>, &Text)> {
    match term {
        Term::Text(text) => Some((None, text)),
        Term::Subject(text) => Some((Some("subject"), text)),
        _ => None,
    }
}

impl Words<'_> {
    /// Whether these words are asked of the per-character CJK index. A
    /// message with a CJK word in it is in that index with all of its words,
    /// so words that all have to be there can be asked of it together.
    ///
    /// A word the index does not keep has no say in it. `・` is punctuation
    /// that happens to live among the Japanese characters, and letting it
    /// choose the index sent `・ annex` to the CJK one, where the only word
    /// it had left was thrown away and the whole search with it.
    pub(super) fn cjk(&self) -> bool {
        match self {
            Words::Leaf(_, text) => indexable(text) && has_cjk(&text.value),
            Words::All(parts) | Words::Any(parts) => parts.iter().any(Words::cjk),
        }
    }

    pub(super) fn leaves<'a>(&'a self, out: &mut Vec<&'a Text>) {
        match self {
            Words::Leaf(_, text) => out.push(text),
            Words::All(parts) | Words::Any(parts) => parts.iter().for_each(|p| p.leaves(out)),
        }
    }

    /// Whether every word in it is of one script, CJK or not, and so can be
    /// asked of one index. Words the index does not keep are not words.
    pub(super) fn one_script(&self) -> bool {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        let mut scripts = leaves
            .iter()
            .filter(|text| indexable(text))
            .map(|text| has_cjk(&text.value));
        let first = scripts.next();
        scripts.all(|script| Some(script) == first)
    }

    /// Whether any of it is a word the index keeps.
    pub(super) fn indexable(&self) -> bool {
        match self {
            Words::Leaf(_, text) => indexable(text),
            Words::All(parts) | Words::Any(parts) => parts.iter().any(Words::indexable),
        }
    }
}

/// The node as words alone, if that is all it is, and all of one script.
///
/// Words have to share an index to share an expression. `東京 OR tokyo` asked
/// of the CJK index would miss every message that says "tokyo" and has no CJK
/// in it, because those are not in that index at all. `東京 sato` asked there
/// found nothing either: that index holds a message's subject and body, a
/// character to a token, and `sato` was in an address. And it has no phrases,
/// so `東京 "board pack"` found mail with the two words apart. Words of two
/// scripts are two questions, and `bind` asks each of its own index.
pub(super) fn as_words<'a>(node: &Node<'a>) -> Option<Words<'a>> {
    as_words_of_any_script(node).filter(Words::one_script)
}

pub(super) fn as_words_of_any_script<'a>(node: &Node<'a>) -> Option<Words<'a>> {
    match node {
        Node::Lit(lit) if !lit.negated => {
            text_of(lit.term).map(|(column, text)| Words::Leaf(column, text))
        }
        Node::Lit(_) => None,
        Node::All(parts) => parts
            .iter()
            .map(as_words_of_any_script)
            .collect::<Option<_>>()
            .map(Words::All),
        Node::Any(parts) => parts
            .iter()
            .map(as_words_of_any_script)
            .collect::<Option<_>>()
            .map(Words::Any),
    }
}

/// The node as conditions alone, if that is all it is. Words that must not
/// be there count: they are a lookup and a subtraction, which SQL can do
/// anywhere. Words that must be there do not, because they are what ranks.
pub(super) fn as_cond<'a>(node: &Node<'a>) -> Option<Cond<'a>> {
    match node {
        Node::Lit(lit) => match text_of(lit.term) {
            None => Some(Cond::Lit(*lit)),
            Some((column, text)) if lit.negated => Some(Cond::Without(column, text)),
            Some(_) => None,
        },
        Node::All(parts) => parts
            .iter()
            .map(as_cond)
            .collect::<Option<_>>()
            .map(Cond::All),
        Node::Any(parts) => parts
            .iter()
            .map(as_cond)
            .collect::<Option<_>>()
            .map(Cond::Any),
    }
}

/// The whole node as conditions, the words in it as lookups of their own.
///
/// Always possible, and always right: every leaf is a set of messages and
/// SQL does the algebra. It is not how a query is normally asked, because a
/// lookup cannot rank and has no snippet to show. It is how one is asked when
/// multiplying it out would pass the limit, where the choice used to be
/// between a hundred statements and quietly leaving some out.
pub(super) fn as_lookups<'a>(node: &Node<'a>) -> Cond<'a> {
    match node {
        Node::Lit(lit) => match text_of(lit.term) {
            None => Cond::Lit(*lit),
            Some((column, text)) if lit.negated => Cond::Without(column, text),
            Some((column, text)) => Cond::With(column, text),
        },
        Node::All(parts) => Cond::All(parts.iter().map(as_lookups).collect()),
        Node::Any(parts) => Cond::Any(parts.iter().map(as_lookups).collect()),
    }
}

/// A condition with the parts that ask nothing taken out, or `None` when
/// that was all of it.
///
/// Punctuation is not in the index, so a word made of it asks nothing,
/// wanted or not. Among things that all have to hold it is simply skipped.
/// As one side of a choice it is no side at all: written as "true of every
/// message", `lunch OR -[` listed the whole mailbox for the length of the
/// keystroke before `-[acme]` became a word.
pub(super) fn pruned(cond: Cond<'_>) -> Option<Cond<'_>> {
    fn kept(parts: Vec<Cond<'_>>) -> Vec<Cond<'_>> {
        parts.into_iter().filter_map(pruned).collect()
    }
    match cond {
        Cond::With(_, text) | Cond::Without(_, text) if !indexable(text) => None,
        Cond::All(parts) => match kept(parts) {
            parts if parts.is_empty() => None,
            mut parts if parts.len() == 1 => parts.pop(),
            parts => Some(Cond::All(parts)),
        },
        Cond::Any(parts) => match kept(parts) {
            parts if parts.is_empty() => None,
            mut parts if parts.len() == 1 => parts.pop(),
            parts => Some(Cond::Any(parts)),
        },
        other => Some(other),
    }
}

/// The word still being typed: the last one asked for that the index keeps,
/// unless it was put in quotes, which is somebody saying it is finished.
///
/// One word for the whole query, not one per statement. Chosen a statement
/// at a time, `ann` in `ann OR (from:dana draft)` was the last word of its
/// own statement and found "annex", while in `ann OR draft` it was not and
/// did not. Last among the words the index keeps, too: `lun -` is somebody
/// starting an exclusion, and the dash took the prefix off `lun` and emptied
/// the list for a keystroke.
pub(super) fn typing_word<'a>(node: &Node<'a>) -> Option<&'a Text> {
    fn last<'a>(node: &Node<'a>) -> Option<&'a Text> {
        match node {
            Node::Lit(lit) if !lit.negated => text_of(lit.term)
                .map(|(_, text)| text)
                .filter(|text| indexable(text)),
            Node::Lit(_) => None,
            Node::All(parts) | Node::Any(parts) => parts.iter().rev().find_map(last),
        }
    }
    last(node).filter(|text| !text.exact)
}

/// One statement's worth of a query: words to rank by, words to leave out,
/// and conditions, all of which have to hold.
#[derive(Default, Clone)]
pub(super) struct Bundle<'a> {
    pub(super) wanted: Vec<Words<'a>>,
    pub(super) unwanted: Vec<(Option<&'static str>, &'a Text)>,
    pub(super) conds: Vec<Cond<'a>>,
}

/// The query as statements, any of which may match.
///
/// Like is kept with like. Words joined by `OR` are one FTS5 expression and
/// conditions joined by `OR` are one SQL predicate, so `(from:sam OR
/// from:dana) (invoice OR receipt)` is a single statement and a single
/// ranking, and costs what `from:sam invoice` costs. Only an `OR` with words
/// on one side and a condition on the other has to be asked once per side —
/// a ranking has nothing to say about a message that matched no words — and
/// those are multiplied out.
///
/// `None` when that would pass the limit: three bracketed choices of a word
/// or a condition are already eight statements, twice what is allowed.
/// Keeping the first few and dropping the rest left out every statement that
/// took the second side of the first choice, so a query whose only match was
/// there found nothing, and nothing said so. The caller asks it as one
/// statement of lookups instead (`as_lookups`).
pub(super) fn bundles<'a>(node: &Node<'a>) -> Option<Vec<Bundle<'a>>> {
    if let Some(words) = as_words(node) {
        return Some(vec![Bundle {
            wanted: vec![words],
            ..Bundle::default()
        }]);
    }
    Some(match node {
        Node::Lit(lit) => vec![match text_of(lit.term) {
            Some(unwanted) => Bundle {
                unwanted: vec![unwanted],
                ..Bundle::default()
            },
            None => Bundle {
                conds: vec![Cond::Lit(*lit)],
                ..Bundle::default()
            },
        }],
        Node::All(parts) => {
            let mut out = vec![Bundle::default()];
            for part in parts {
                let options = bundles(part)?;
                // A choice with nothing left in it asked nothing, and is
                // skipped the way a word of punctuation is.
                if options.is_empty() {
                    continue;
                }
                if out.len() * options.len() > MAX_ALTERNATIVES {
                    return None;
                }
                out = out
                    .iter()
                    .flat_map(|left| {
                        options.iter().map(move |right| {
                            let mut both = left.clone();
                            both.wanted.extend(right.wanted.iter().cloned());
                            both.unwanted.extend(right.unwanted.iter().copied());
                            both.conds.extend(right.conds.iter().cloned());
                            both
                        })
                    })
                    .collect();
            }
            out
        }
        Node::Any(parts) => {
            let (mut latin, mut cjk, mut conds, mut rest) = (vec![], vec![], vec![], vec![]);
            for part in parts {
                if let Some(words) = as_words(part) {
                    // Words the index does not keep find nothing, so they
                    // are no alternative: `(& OR from:sam) contract` is mail
                    // from Sam, where `&` as a statement of its own was
                    // skipped inside it and became every contract there is.
                    if !words.indexable() {
                        continue;
                    }
                    if words.cjk() { &mut cjk } else { &mut latin }.push(words);
                } else if let Some(cond) = as_cond(part) {
                    conds.extend(pruned(cond));
                } else {
                    rest.extend(bundles(part)?);
                }
            }
            let mut out = Vec::new();
            for mut words in [latin, cjk] {
                let words = match words.len() {
                    0 => continue,
                    1 => words.remove(0),
                    _ => Words::Any(words),
                };
                out.push(Bundle {
                    wanted: vec![words],
                    ..Bundle::default()
                });
            }
            if !conds.is_empty() {
                out.push(Bundle {
                    conds: vec![if conds.len() == 1 {
                        conds.remove(0)
                    } else {
                        Cond::Any(conds)
                    }],
                    ..Bundle::default()
                });
            }
            out.extend(rest);
            if out.len() > MAX_ALTERNATIVES {
                return None;
            }
            out
        }
    })
}

/// A word with no letter or digit in it is nothing the tokenizer keeps.
pub(super) fn indexable(text: &Text) -> bool {
    text.value.chars().any(char::is_alphanumeric)
}

/// One term as FTS5 has to be asked for it: a quoted phrase, exactly as
/// `match_expr` builds them, so that what was typed never reaches MATCH bare.
pub(super) fn phrase(column: Option<&str>, text: &Text, for_cjk: bool, prefix: bool) -> String {
    let inner = if for_cjk {
        let parts = cjk_parts(&text.value);
        if parts.len() == 1 {
            parts[0].clone()
        } else {
            format!("({})", parts.join(" AND "))
        }
    } else if prefix && last_alnum_run(&text.value).chars().count() >= 2 {
        format!("{}*", quote_token(&text.value))
    } else {
        quote_token(&text.value)
    };
    match column {
        Some(column) => format!("{column} : {inner}"),
        None => inner,
    }
}

/// Words as one FTS5 expression, or `None` when none of them is a word the
/// index keeps. Every AND and OR is written out: FTS5 will not take an
/// implicit AND beside a bracket.
pub(super) fn fts(words: &Words, for_cjk: bool, typing: Option<&Text>) -> Option<String> {
    match words {
        Words::Leaf(_, text) if !indexable(text) => None,
        Words::Leaf(column, text) => {
            let prefix = typing.is_some_and(|last| std::ptr::eq(last, *text));
            Some(phrase(*column, text, for_cjk, prefix))
        }
        Words::All(parts) | Words::Any(parts) => {
            let asked: Vec<String> = parts
                .iter()
                .filter_map(|p| fts(p, for_cjk, typing))
                .collect();
            let joiner = if matches!(words, Words::All(_)) {
                " AND "
            } else {
                " OR "
            };
            match asked.len() {
                0 => None,
                1 => asked.into_iter().next(),
                _ => Some(format!("({})", asked.join(joiner))),
            }
        }
    }
}

/// Whether a condition asks, un-negated, for something a term names.
pub(super) fn asks(cond: &Cond, wanted: &dyn Fn(&Term) -> bool) -> bool {
    match cond {
        Cond::Lit(lit) => !lit.negated && wanted(lit.term),
        Cond::With(..) | Cond::Without(..) => false,
        Cond::All(parts) | Cond::Any(parts) => parts.iter().any(|p| asks(p, wanted)),
    }
}

pub(super) fn is_the_bin(term: &Term) -> bool {
    matches!(term, Term::In(role) if role == "spam" || role == "trash")
}

/// Whether words are looked up among these conditions. Only a query too
/// wide to ask a statement at a time has any (`as_lookups`), and they narrow
/// the walk hard enough to change which way round a filter is cheaper.
pub(super) fn looks_up_words(cond: &Cond) -> bool {
    match cond {
        Cond::With(..) => true,
        Cond::Lit(_) | Cond::Without(..) => false,
        Cond::All(parts) | Cond::Any(parts) => parts.iter().any(looks_up_words),
    }
}

pub(super) fn is_snoozed(term: &Term) -> bool {
    *term == Term::Is(State::Snoozed)
}

/// Whether one of these, which all have to hold, is the thing itself: asked
/// for outright, not somewhere inside a choice further down.
pub(super) fn alongside(parts: &[Cond], wanted: &dyn Fn(&Term) -> bool) -> bool {
    parts
        .iter()
        .any(|p| matches!(p, Cond::Lit(lit) if !lit.negated && wanted(lit.term)))
}

/// What holds around a condition, which changes what it means.
///
/// Both are about the things a condition is ANDed with, so both are carried
/// down the tree rather than read off the whole statement. Read off the
/// whole, `(in:inbox from:billing) OR (is:snoozed from:dana)` showed snoozed
/// mail from billing, because somewhere in the statement snoozed mail had
/// been asked for; and `in:trash (in:spam OR from:sam)` found nothing,
/// because the choice kept its second side out of the very bin the query
/// was searching.
#[derive(Clone, Copy)]
pub(super) struct Around<'a> {
    /// `is:snoozed` is asked alongside, so `in:inbox` does not hide snoozed
    /// mail here.
    pub(super) snoozed: bool,
    /// Spam or Trash is asked for alongside, so nothing here is kept out
    /// of it.
    pub(super) binned: bool,
    /// Nothing narrows this statement, so every message is being read.
    ///
    /// Not quite the same as having no words to rank by. A query too wide to
    /// ask a statement at a time looks its words up instead (`as_lookups`),
    /// and that lookup narrows the walk to almost nothing, which puts a
    /// filter back to being cheaper asked per message: `(meeting OR to:avery)
    /// (budget OR to:blake) (report OR to:casey)` took 91ms asked of the
    /// whole table and 35ms asked message by message.
    ///
    /// Which way round a condition is cheaper follows from that. A recipient,
    /// a tag or a file name lives in a table of its own, and asking it of one
    /// message is a lookup: cheap where words have already narrowed the
    /// search to a few, and a hundred thousand lookups where nothing has.
    /// Asked once of the whole table it is one pass, and then a message
    /// either is in the answer or is not. At a hundred thousand messages, a
    /// recipient nobody wrote to went from 135ms to 25ms, two of them joined
    /// by `OR` from 209ms to 47ms, a tag nobody used from 81ms to 6ms, and a
    /// file name nobody has from 75ms to 6ms. What it costs is the opposite
    /// case — a recipient in much of the mailbox, 19ms against 26ms, because
    /// a listing stops as soon as it has a page and one pass cannot.
    pub(super) walking: bool,
    /// The word still being typed.
    pub(super) typing: Option<&'a Text>,
}

/// What a query is planned as, before a mailbox is anywhere near it.
///
/// The plan is where a search is won or lost: how many statements a query
/// becomes, which index each one is asked of, which word is still being
/// typed, whether a filter is asked of its whole table. None of it is
/// visible in a result — a plan that quietly drops a word looks exactly like
/// a mailbox that does not hold it — and all of it is decided by the pure
/// functions above, so all of it can be read back here in microseconds.
///
/// Every case below was a bug once, or is the boundary of one.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_query::parse;

    /// A query as it would be asked: one line per statement, in the form
    /// `<index> <words> | <conditions> | typing:<word>`.
    fn plan(query: &str) -> String {
        let q = parse(query);
        let Some(root) = &q.root else {
            return String::new();
        };
        let node = without_nots(root, false);
        let typing = typing_word(&node);
        let bundles = bundles(&node).unwrap_or_else(|| {
            vec![Bundle {
                conds: pruned(as_lookups(&node)).into_iter().collect(),
                ..Bundle::default()
            }]
        });
        bundles
            .iter()
            .map(|bundle| {
                let cjk = bundle.wanted.iter().any(Words::cjk);
                let words: Vec<String> = bundle
                    .wanted
                    .iter()
                    .filter_map(|w| fts(w, w.cjk(), typing))
                    .collect();
                let index = match (words.is_empty(), cjk) {
                    (true, _) => "rows",
                    (_, true) => "cjk",
                    (_, false) => "fts",
                };
                let unwanted: Vec<&str> = bundle
                    .unwanted
                    .iter()
                    .map(|(_, text)| text.value.as_str())
                    .collect();
                let conds = bundle.conds.len();
                format!(
                    "{index} {}{}{} | conds {conds}",
                    words.join(" AND "),
                    if unwanted.is_empty() { "" } else { " NOT " },
                    unwanted.join(" "),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
            + &typing.map_or(String::new(), |text| format!(" | typing:{}", text.value))
    }

    #[test]
    fn like_is_kept_with_like() {
        // One word, one statement, and the word still being typed.
        assert_eq!(
            plan("meeting"),
            "fts \"meeting\"* | conds 0 | typing:meeting"
        );
        // Bracketed choices of the same kind stay one statement and one
        // ranking, which is what makes a bracketed query cost what its
        // words cost.
        assert_eq!(
            plan("(from:sam OR from:dana) meeting"),
            "fts \"meeting\"* | conds 1 | typing:meeting"
        );
        // The prefix goes to the last word in the query, inside the
        // brackets as outside them.
        assert_eq!(
            plan("(meeting OR budget) from:sam"),
            "fts (\"meeting\" OR \"budget\"*) | conds 1 | typing:budget"
        );
        // Only a choice between a word and a condition is asked twice.
        assert_eq!(
            plan("meeting OR from:sam"),
            "fts \"meeting\"* | conds 0\nrows  | conds 1 | typing:meeting"
        );
    }

    /// Past the limit a query is asked whole, as one statement of lookups,
    /// rather than having the statements past it dropped.
    #[test]
    fn a_query_too_wide_to_multiply_is_asked_whole() {
        let wide = "(a OR from:one) (b OR from:two) (c OR from:three)";
        assert!(bundles(&without_nots(parse(wide).root.as_ref().unwrap(), false)).is_none());
        assert_eq!(plan(wide), "rows  | conds 1 | typing:c");
        // One fewer choice still multiplies out, four statements at most.
        let narrow = "(a OR from:one) (b OR from:two)";
        let count = bundles(&without_nots(parse(narrow).root.as_ref().unwrap(), false))
            .expect("four statements are allowed")
            .len();
        assert_eq!(count, 4);
    }

    /// The word still being typed is the last one the index keeps, for the
    /// whole query: a dash or a bracket after it is the start of what comes
    /// next, not the end of the word.
    #[test]
    fn the_word_being_typed_is_the_last_one_the_index_keeps() {
        assert_eq!(plan("lun"), "fts \"lun\"* | conds 0 | typing:lun");
        // A lone dash is a dash, not an exclusion, and the index keeps
        // neither it nor a bracket.
        assert_eq!(plan("lun -"), "fts \"lun\"* | conds 0 | typing:lun");
        assert_eq!(plan("lun ["), "fts \"lun\"* | conds 0 | typing:lun");
        // Finished with quotes, it is not being typed.
        assert_eq!(plan("\"lun\""), "fts \"lun\" | conds 0");
        // A filter after it does not take the prefix away.
        assert_eq!(plan("lun is:unread"), "fts \"lun\"* | conds 1 | typing:lun");
    }

    /// Words of two scripts are two questions. The per-character index holds
    /// a message's subject and body and nothing else, so a Latin word asked
    /// of it never sees an address, and a phrase loses its order.
    #[test]
    fn each_script_is_asked_of_its_own_index() {
        // A CJK word is asked character by character, as the index holds
        // it, so its phrase is what keeps the characters together.
        assert_eq!(plan("東京"), "cjk \"東 京\" | conds 0 | typing:東京");
        assert_eq!(
            plan("東京 会議"),
            "cjk (\"東 京\" AND \"会 議\") | conds 0 | typing:会議"
        );
        // Mixed: the CJK word ranks, the Latin word is asked beside it.
        assert_eq!(
            plan("東京 sato"),
            "cjk \"東 京\" AND \"sato\"* | conds 0 | typing:sato"
        );
    }

    /// Punctuation is not a word, wherever its characters live in Unicode.
    /// `・` is Japanese punctuation, and taking it for a word sent `・ annex`
    /// to the per-character index and lost the only word it had.
    #[test]
    fn punctuation_is_not_a_word_and_not_a_script() {
        assert_eq!(plan("annex"), "fts \"annex\"* | conds 0 | typing:annex");
        assert_eq!(plan("・ annex"), "fts \"annex\"* | conds 0 | typing:annex");
        assert_eq!(plan("[ annex"), "fts \"annex\"* | conds 0 | typing:annex");
        // Beside a real CJK word it changes nothing either.
        assert_eq!(plan("・ 東京"), plan("東京"));
        assert_eq!(plan("・"), "rows  | conds 0");
    }

    /// A word that must not be there sits in the same expression when it is
    /// of the same script, and is taken away separately when it is not.
    #[test]
    fn what_must_not_be_there_goes_where_it_can_be_asked() {
        assert_eq!(
            plan("contract -draft"),
            "fts \"contract\"* NOT draft | conds 0 | typing:contract"
        );
        assert_eq!(
            plan("東京 -sato"),
            "cjk \"東 京\" NOT sato | conds 0 | typing:東京"
        );
        assert_eq!(plan("-draft"), "rows  NOT draft | conds 0");
    }
}
