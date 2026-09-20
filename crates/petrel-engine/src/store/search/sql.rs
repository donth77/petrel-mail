//! A statement, written as SQL.
//!
//! Every value is bound, never written into the text: `from:` and `in:` carry
//! whatever somebody typed, and a search box that reaches SQL is the oldest
//! mistake there is. The only text written into a statement here is text this
//! file wrote.
use super::*;

/// One alternative of a query, bound into SQL.
pub(super) struct Bound {
    /// The words to rank by, when there are any.
    pub(super) asked: Option<Asked>,
    /// The conditions, each beginning ` AND`, and the values they bind.
    pub(super) conditions: String,
    pub(super) args: Vec<Box<dyn rusqlite::ToSql>>,
}

/// The words one alternative asked for, as the index has to be asked.
pub(super) struct Asked {
    /// The FTS5 expression.
    pub(super) expr: String,
    /// Whether it is asked of the per-character CJK index.
    pub(super) cjk: bool,
    /// The words a CJK snippet should mark. FTS5 marks its own.
    pub(super) marks: String,
}

impl Store {
    /// One statement's worth of a query, bound into SQL — or `None` when it
    /// asks for nothing, or for something nothing can match.
    ///
    /// Built rather than interpolated: `from:` and `in:` carry whatever was
    /// typed, and a search box that reaches SQL is the oldest mistake there
    /// is. Every value is a bound parameter; every word that reaches MATCH is
    /// a quoted phrase. The only text written into a statement here is text
    /// this file wrote.
    pub(super) fn bind(
        &self,
        bundle: &Bundle,
        account: i64,
        typing: Option<&Text>,
    ) -> Result<Option<Bound>> {
        // The words first, because they decide which index is asked.
        //
        // A word with no letter or digit in it is simply skipped among other
        // words. On its own it finds nothing, which is what a half-typed
        // `[acme]` should do: for the length of one keystroke the query is
        // `in:inbox [`, and that must not list the whole inbox.
        let mut leaves = Vec::new();
        bundle.wanted.iter().for_each(|w| w.leaves(&mut leaves));
        let cjk = bundle.wanted.iter().any(Words::cjk);
        // As-you-type: the last word is a prefix while it is still being
        // typed (`typing_word`). Last among the words, not last in the field
        // — a chip writes its token after them, and clicking one mid-word
        // must not empty the list. Never a quoted word, which somebody
        // finished; never one in the CJK index, where a character is already
        // a whole token.
        //
        // One index ranks, and it is the CJK one when any word is CJK. Words
        // of the other script are asked of their own index, beside it
        // (`as_words` says why they cannot share the expression).
        let (ranking, beside): (Vec<&Words>, Vec<&Words>) =
            bundle.wanted.iter().partition(|w| w.cjk() == cjk);
        let wanted: Vec<String> = ranking.iter().filter_map(|w| fts(w, cjk, typing)).collect();
        let beside: Vec<String> = beside
            .iter()
            .filter_map(|w| fts(w, false, typing))
            .collect();
        if wanted.is_empty() && !bundle.wanted.is_empty() {
            return Ok(None);
        }
        let mut expr = wanted.join(" AND ");

        let mut sql = String::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !beside.is_empty() {
            sql.push_str(
                " AND m.id IN (SELECT rowid FROM fts_messages WHERE fts_messages MATCH ?)",
            );
            args.push(Box::new(beside.join(" AND ")));
        }

        // The account on screen, always. Search used to run over the whole
        // store, so standing in one account quietly answered with the other
        // account's mail — the one wall the multi-account design promises
        // never leaks (03 §4.1), broken precisely where it is least visible.
        sql.push_str(" AND m.account_id = ?");
        args.push(Box::new(account));

        // Junk and deleted mail stay out unless they are what was asked for.
        //
        // Searching a mailbox is not an invitation to reopen what was already
        // judged and discarded, and a result that quietly comes from Spam is
        // worse than no result at all: it puts a message the filter rejected
        // back in front of the reader looking exactly like ordinary mail. The
        // grammar is the way in — `in:spam` and `in:trash` search them, and
        // nothing else does. Asking on one side of an `OR` is not asking on
        // the other: `in:spam OR from:sam` lets spam in on the left only, and
        // `any_sql` is where a bracketed `(in:spam OR from:sam)` is held to
        // the same rule.
        let guard = not_binned("m");
        if !bundle.conds.iter().any(|c| asks(c, &is_the_bin)) {
            sql.push_str(&format!(" AND {guard}"));
        }

        // Words that must not be there. Beside wanted ones of the same script
        // they are FTS5's own NOT, which costs nothing. Otherwise they are
        // looked up on their own and taken away: NOT needs something on its
        // left, so `-draft` with no other word cannot be a MATCH, and a word
        // has to be asked of the index its script is in, whichever index the
        // wanted words are in. That holds both ways round. The CJK index has
        // a message's subject and body and nothing else, one character to a
        // token, so `東京 -sato` asked there never saw the address sato is in,
        // and `東京 -"board pack"` lost the phrase and dropped mail that had
        // the two words apart.
        let mut refused: Vec<String> = Vec::new();
        let mut excluded = 0;
        for (column, text) in bundle.unwanted.iter().filter(|(_, t)| indexable(t)) {
            excluded += 1;
            if !wanted.is_empty() && cjk == has_cjk(&text.value) {
                refused.push(phrase(*column, text, cjk, false));
            } else {
                sql.push_str(&format!(
                    " AND {}",
                    Self::lookup_sql(*column, text, false, false, &mut args)
                ));
            }
        }
        if !refused.is_empty() {
            expr = format!("({expr}) NOT ({})", refused.join(" OR "));
        }

        let around = Around {
            snoozed: alongside(&bundle.conds, &is_snoozed),
            binned: alongside(&bundle.conds, &is_the_bin),
            walking: wanted.is_empty() && !bundle.conds.iter().any(looks_up_words),
            typing,
        };
        for cond in &bundle.conds {
            let predicate = self.cond_sql(cond, around, &guard, &mut args)?;
            sql.push_str(&format!(" AND {predicate}"));
        }

        // Every word in it was punctuation and every one was excluded:
        // nothing is left to ask, and that is not the same as everything.
        if wanted.is_empty() && excluded == 0 && bundle.conds.is_empty() {
            return Ok(None);
        }
        Ok(Some(Bound {
            asked: (!wanted.is_empty()).then(|| Asked {
                expr,
                cjk,
                marks: leaves
                    .iter()
                    .find(|text| has_cjk(&text.value))
                    .map(|text| text.value.clone())
                    .unwrap_or_default(),
            }),
            conditions: sql,
            args,
        }))
    }

    /// Words as a lookup of their own, in whichever index holds their
    /// script: the messages that have them, or the ones that do not.
    pub(super) fn lookup_sql(
        column: Option<&str>,
        text: &Text,
        there: bool,
        prefix: bool,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> String {
        let cjk = has_cjk(&text.value);
        let index = if cjk { "fts_cjk" } else { "fts_messages" };
        args.push(Box::new(phrase(column, text, cjk, prefix)));
        let not = if there { "" } else { "NOT " };
        format!("m.id {not}IN (SELECT rowid FROM {index} WHERE {index} MATCH ?)")
    }

    /// A tree of conditions as one SQL predicate, its values bound in the
    /// order it writes them.
    pub(super) fn cond_sql(
        &self,
        cond: &Cond,
        around: Around,
        guard: &str,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> Result<String> {
        Ok(match cond {
            Cond::Lit(lit) if lit.negated => {
                format!("NOT ({})", self.predicate(lit.term, around, args)?)
            }
            Cond::Lit(lit) => self.predicate(lit.term, around, args)?,
            // Punctuation asks nothing. `pruned` takes it out before it gets
            // here; what is left says "true", which is what skipping means
            // among things that all have to hold.
            Cond::With(_, text) | Cond::Without(_, text) if !indexable(text) => "1".to_string(),
            Cond::With(column, text) => {
                let prefix = around.typing.is_some_and(|last| std::ptr::eq(last, *text));
                Self::lookup_sql(*column, text, true, prefix, args)
            }
            Cond::Without(column, text) => Self::lookup_sql(*column, text, false, false, args),
            Cond::All(parts) => {
                let around = Around {
                    snoozed: around.snoozed || alongside(parts, &is_snoozed),
                    binned: around.binned || alongside(parts, &is_the_bin),
                    ..around
                };
                let parts = parts
                    .iter()
                    .map(|p| self.cond_sql(p, around, guard, args))
                    .collect::<Result<Vec<_>>>()?;
                format!("({})", parts.join(" AND "))
            }
            Cond::Any(parts) => {
                // The bin is let in one alternative at a time, in brackets as
                // out of them: the side that named it, and no other. Unless
                // the bin is what all of this is being asked of, and then no
                // side needs keeping out of it.
                let named = !around.binned && parts.iter().any(|p| asks(p, &is_the_bin));
                let parts = parts
                    .iter()
                    .map(|p| {
                        let sql = self.cond_sql(p, around, guard, args)?;
                        Ok(if named && !asks(p, &is_the_bin) {
                            format!("({sql} AND {guard})")
                        } else {
                            sql
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                format!("({})", parts.join(" OR "))
            }
        })
    }

    /// One condition as SQL, its values bound as it goes.
    pub(super) fn predicate(
        &self,
        term: &Term,
        around: Around,
        args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ) -> Result<String> {
        // SQLite's `lower` folds ASCII only. A value that is not ASCII goes
        // through ours; an ASCII one can only ever match ASCII letters, so
        // where it is compared with `=` it keeps the built-in, which is
        // several times cheaper per row.
        //
        // Where it is compared with LIKE it needs nothing at all: LIKE already
        // ignores the case of ASCII letters. Folding the column anyway made a
        // copy of it per row and per condition, and that was most of what a
        // sender filter cost. Eight senders joined by OR that match nobody took
        // 265ms at a hundred thousand messages with it, and 103ms without.
        let fold = |value: &str| {
            if value.is_ascii() {
                "lower"
            } else {
                "petrel_lower"
            }
        };
        let like_fold = |value: &str| {
            if value.is_ascii() { "" } else { "petrel_lower" }
        };
        let contains = |value: &str| format!("%{}%", folders::like_escape(&value.to_lowercase()));
        Ok(match term {
            // Words are asked of the index, never here.
            Term::Text(_) | Term::Subject(_) => "1".to_string(),
            Term::From(who) => {
                for _ in 0..2 {
                    args.push(Box::new(contains(who)));
                }
                let lower = like_fold(who);
                format!(
                    "({lower}(coalesce(m.from_addr,'')) LIKE ? ESCAPE '\\'
                      OR {lower}(coalesce(m.from_display,'')) LIKE ? ESCAPE '\\')"
                )
            }
            Term::To(who) | Term::Cc(who) => {
                let role = if matches!(term, Term::To(_)) {
                    "to"
                } else {
                    "cc"
                };
                for _ in 0..2 {
                    args.push(Box::new(contains(who)));
                }
                // `addr_norm` was lowercased on the way in, by Rust.
                let lower = like_fold(who);
                let holds = format!(
                    "a.role = '{role}'
                     AND (a.addr_norm LIKE ? ESCAPE '\\'
                          OR {lower}(coalesce(a.display,'')) LIKE ? ESCAPE '\\')"
                );
                // One pass or a lookup each, as `walking` explains.
                if around.walking {
                    format!("m.id IN (SELECT a.message_id FROM message_addresses a WHERE {holds})")
                } else {
                    format!(
                        "EXISTS (SELECT 1 FROM message_addresses a
                                 WHERE a.message_id = m.id AND {holds})"
                    )
                }
            }
            Term::In(name) => {
                // A role, or a folder the user made — by full path or by leaf, so
                // `in:receipts` and `in:projects/petrel` both say what they mean.
                // The parser lowercased the value; the comparisons follow suit.
                // EXISTS rather than `m.id IN (…)`. The IN form was tried, on the
                // theory that the correlated subquery made the planner walk the
                // mailbox; measured on a real store of twenty-nine thousand it was
                // slower at every width — 7.4ms against 1.3ms for one exact token,
                // 92ms against 84ms for the broadest prefix — because it builds the
                // whole mailbox's placement list whatever the match narrows to.
                // Both plans open on the FTS match, so neither walks the mailbox.
                for _ in 0..2 {
                    args.push(Box::new(name.clone()));
                }
                for _ in 0..2 {
                    args.push(Box::new(folders::like_escape(name)));
                }
                let lower = fold(name);
                let holds = format!(
                    "(f.role = ?
                      OR {lower}(f.path) = ?
                      OR {lower}(f.path) LIKE '%/' || ? ESCAPE '\\'
                      OR {lower}(f.path) LIKE '%.' || ? ESCAPE '\\')"
                );
                // A lookup per message, even where the others are asked of
                // their whole table at once (`walking`). A mailbox is usually
                // most of the mailbox, and a listing stops as soon as it has
                // a page: `in:inbox is:unread` answers in 4ms that way and in
                // 22ms asked at once, because asking at once builds every
                // placement in the account first. It is the empty folder that
                // pays for it — `in:trash` with nothing in it reads every
                // message, 110ms against 49ms — and that is the rarer search
                // and still inside the budget.
                let placed = format!(
                    "EXISTS (SELECT 1 FROM placements p JOIN folders f ON f.id = p.folder_id
                             WHERE p.message_id = m.id AND {holds})"
                );
                // Snoozing takes a message out of the inbox until it comes back.
                // That is what the Inbox view's predicate says and what its unread
                // badge counts, and search used to disagree: `in:inbox is:unread`
                // returned the snoozed ones too, so the list and the number beside
                // the mailbox differed by exactly the mail somebody had put off.
                //
                // Only the inbox, because that is the only view snoozing hides
                // from — and not when `is:snoozed` asked for them by name, since
                // snoozing hides mail rather than burying it.
                if name == "inbox" && !around.snoozed {
                    format!(
                        "({placed} AND coalesce(m.snoozed_until_ms, 0)
                                       <= (strftime('%s','now') * 1000))"
                    )
                } else {
                    placed
                }
            }
            Term::Tag(name) => {
                args.push(Box::new(name.to_lowercase()));
                let lower = fold(name);
                let holds = format!("{lower}(tg.name) = ?");
                if around.walking {
                    format!(
                        "m.id IN (SELECT mt.message_id FROM message_tags mt
                                  JOIN tags tg ON tg.id = mt.tag_id WHERE {holds})"
                    )
                } else {
                    format!(
                        "EXISTS (SELECT 1 FROM message_tags mt JOIN tags tg ON tg.id = mt.tag_id
                                 WHERE mt.message_id = m.id AND {holds})"
                    )
                }
            }
            Term::Filename(part) => {
                args.push(Box::new(contains(part)));
                let lower = like_fold(part);
                let holds = format!("{lower}(coalesce(a.filename,'')) LIKE ? ESCAPE '\\'");
                if around.walking {
                    format!("m.id IN (SELECT a.message_id FROM attachments a WHERE {holds})")
                } else {
                    format!(
                        "EXISTS (SELECT 1 FROM attachments a
                                 WHERE a.message_id = m.id AND {holds})"
                    )
                }
            }
            Term::HasAttachment => "m.has_attachments = 1".to_string(),
            Term::Is(State::Unread) => format!("m.flags & {} = 0", flags::SEEN),
            Term::Is(State::Read) => format!("m.flags & {} != 0", flags::SEEN),
            Term::Is(State::Starred) => format!("m.flags & {} != 0", flags::FLAGGED),
            Term::Is(State::Snoozed) => {
                "coalesce(m.snoozed_until_ms, 0) > (strftime('%s','now') * 1000)".to_string()
            }
            // `after:` takes in the day it names and `before:` leaves it
            // out, so `after:2026-08-01 before:2026-09-01` is August and
            // nothing else. The year chip has always written `after:2026`
            // for "this year"; read the other way round it would mean 2027.
            Term::After(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                "m.date_ms >= ?".to_string()
            }
            Term::Before(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                "m.date_ms < ?".to_string()
            }
            Term::On(when) => {
                args.push(Box::new(self.local_midnight_ms(when.first_day())?));
                args.push(Box::new(self.local_midnight_ms(when.day_after())?));
                "(m.date_ms >= ? AND m.date_ms < ?)".to_string()
            }
        })
    }
}
