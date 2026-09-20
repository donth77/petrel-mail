//! The grammar, asked of a real store.
//!
//! `search_query.rs` checks the shape a query parses to. This checks what it
//! finds. The Help screen documented eleven forms the first parser read as
//! ordinary words, and one it read backwards: `-draft` returned only mail
//! containing "draft", because a quoted `"-draft"` is the token `draft`.
//! Every form in docs 07 §5.1 is asserted here against SQLite and FTS5
//! themselves, so none of them can quietly go back to being words.

use petrel_engine::blob::BlobStore;
use petrel_engine::search_query::Period;
use petrel_engine::store::{CountMode, NewMessage, Store, flags};

struct Mailbox {
    store: Store,
    account: i64,
    _dir: tempfile::TempDir,
}

struct Mail<'a> {
    id: &'a str,
    from: &'a str,
    to: &'a str,
    cc: &'a str,
    subject: &'a str,
    body: &'a str,
    file: Option<&'a str>,
}

fn raw(m: &Mail) -> Vec<u8> {
    let cc = if m.cc.is_empty() {
        String::new()
    } else {
        format!("Cc: {}\r\n", m.cc)
    };
    let head = format!(
        "From: {}\r\nTo: {}\r\n{cc}Subject: {}\r\n\
         Date: Tue, 18 Aug 2026 14:02:00 +0000\r\nMessage-ID: <{}@grammar.example>\r\n\
         MIME-Version: 1.0\r\n",
        m.from, m.to, m.subject, m.id
    );
    let rest = match m.file {
        Some(name) => format!(
            "Content-Type: multipart/mixed; boundary=b\r\n\r\n\
             --b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n\
             --b\r\nContent-Type: application/octet-stream; name=\"{name}\"\r\n\
             Content-Disposition: attachment; filename=\"{name}\"\r\n\r\nx\r\n--b--\r\n",
            m.body
        ),
        None => format!(
            "Content-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n",
            m.body
        ),
    };
    format!("{head}{rest}").into_bytes()
}

/// Seven messages, each its own conversation, chosen so that every operator
/// has something to find and something to leave behind.
fn mailbox() -> Mailbox {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("t.db")).unwrap();
    let blobs = BlobStore::open(&dir.path().join("blobs")).unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    let mail = [
        Mail {
            id: "q3",
            from: "Sam Ortiz <sam@example.com>",
            to: "Dana Wu <dana@example.com>",
            cc: "Legal Team <legal@example.com>",
            subject: "Q3 vendor contracts",
            body: "The board pack and the revised annex are attached.",
            file: Some("annex-v2.pdf"),
        },
        Mail {
            id: "draft",
            from: "Dana Wu <dana@example.com>",
            to: "me@example.com",
            cc: "",
            subject: "Draft contract terms",
            // Both words of "board pack", but not side by side.
            body: "A draft of the contract. We pack it up once the board has seen it.",
            file: None,
        },
        Mail {
            id: "invoice",
            // "Élodie Martin", as a header has to spell it.
            from: "=?UTF-8?Q?=C3=89lodie_Martin?= <elodie@example.fr>",
            to: "me@example.com",
            cc: "",
            subject: "Invoice 2214",
            body: "Votre facture. Nothing here concerns contracts.",
            file: Some("facture_2214.xlsx"),
        },
        Mail {
            id: "lunch",
            from: "billing@vendorco.example",
            to: "me@example.com",
            cc: "",
            subject: "Lunch",
            body: "The invoice is mentioned here, in the body and nowhere else.",
            file: None,
        },
        Mail {
            id: "tokyo",
            from: "sato@example.jp",
            to: "me@example.com",
            cc: "",
            subject: "東京の会議",
            body: "契約について。",
            file: None,
        },
        Mail {
            id: "osaka",
            from: "sato@example.jp",
            to: "me@example.com",
            cc: "",
            subject: "大阪",
            body: "会議のメモです。",
            file: None,
        },
        Mail {
            id: "both",
            from: "sato@example.jp",
            to: "me@example.com",
            cc: "",
            subject: "Annex for 東京",
            body: "The annex, for the 東京 office.",
            file: None,
        },
    ];
    for (uid, m) in mail.iter().enumerate() {
        store
            .ingest_raw(&blobs, account, Some(inbox), Some(uid as u32 + 1), &raw(m))
            .unwrap();
    }
    Mailbox {
        store,
        account,
        _dir: dir,
    }
}

/// The subjects a query finds, sorted, so an assertion reads as a set.
fn found(store: &Store, query: &str) -> Vec<String> {
    let mut subjects = in_order(store, query);
    subjects.sort();
    subjects
}

fn in_order(store: &Store, query: &str) -> Vec<String> {
    store
        .search_threads(query, 50)
        .unwrap_or_else(|e| panic!("{query:?} failed: {e}"))
        .into_iter()
        .map(|row| row.subject)
        .collect()
}

const EVERYTHING: usize = 7;

/* Who. */

#[test]
fn to_and_cc_are_not_from() {
    let m = mailbox();
    assert_eq!(found(&m.store, "to:dana"), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "cc:legal"), ["Q3 vendor contracts"]);
    // By name as well as by address, like `from:`.
    assert_eq!(
        found(&m.store, r#"cc:"Legal Team""#),
        ["Q3 vendor contracts"]
    );
    // Dana wrote one and received one; each operator sees only its own half.
    assert_eq!(found(&m.store, "from:dana"), ["Draft contract terms"]);
    assert!(found(&m.store, "to:legal").is_empty());
    assert!(found(&m.store, "cc:dana").is_empty());
    assert_eq!(found(&m.store, "from:@vendorco.example"), ["Lunch"]);
}

/// SQLite's own `lower` folds A to Z and nothing else, so a name that opens
/// with an accented capital could not be found by typing it in lowercase —
/// which is how everybody types a search.
#[test]
fn a_name_is_matched_whatever_its_case_in_any_alphabet() {
    let m = mailbox();
    for typed in [
        "from:élodie",
        "from:Élodie",
        "from:ÉLODIE",
        "from:\"élodie martin\"",
    ] {
        assert_eq!(found(&m.store, typed), ["Invoice 2214"], "{typed}");
    }
}

/// An ASCII name is not folded before it is compared: LIKE already ignores
/// the case of ASCII letters, and folding every row's name first was most
/// of what a sender filter cost. This holds the case-blindness in place, in
/// case anything ever turns LIKE case-sensitive.
#[test]
fn an_ascii_name_is_matched_whatever_its_case() {
    let m = mailbox();
    for typed in ["from:SAM", "from:Ortiz", "from:SAM@EXAMPLE.COM"] {
        assert_eq!(found(&m.store, typed), ["Q3 vendor contracts"], "{typed}");
    }
    assert_eq!(found(&m.store, "to:DANA"), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "to:\"dana WU\""), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "cc:LEGAL"), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "-from:SAM").len(), EVERYTHING - 1);
    assert_eq!(
        found(&m.store, "from:SAM OR from:DANA"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
}

/* What. */

#[test]
fn subject_means_the_subject_line_only() {
    let m = mailbox();
    assert_eq!(found(&m.store, "invoice"), ["Invoice 2214", "Lunch"]);
    assert_eq!(found(&m.store, "subject:invoice"), ["Invoice 2214"]);
    assert_eq!(found(&m.store, "invoice -subject:invoice"), ["Lunch"]);
    assert_eq!(
        found(&m.store, r#"subject:"vendor contracts""#),
        ["Q3 vendor contracts"]
    );
    assert!(found(&m.store, r#"subject:"contracts vendor""#).is_empty());
    // As-you-type reaches into the operator too.
    assert_eq!(found(&m.store, "subject:invo"), ["Invoice 2214"]);
}

#[test]
fn a_filename_is_part_of_a_files_name() {
    let m = mailbox();
    assert_eq!(found(&m.store, "filename:.pdf"), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "filename:ANNEX"), ["Q3 vendor contracts"]);
    assert_eq!(found(&m.store, "filename:facture_2214"), ["Invoice 2214"]);
    assert_eq!(
        found(&m.store, "has:attachment -filename:.pdf"),
        ["Invoice 2214"]
    );
}

/// `_` and `%` are what was typed, not LIKE's wildcards. Unescaped,
/// `annex_v2` matches `annex-v2.pdf` and `%` matches every file there is.
#[test]
fn what_was_typed_is_never_a_wildcard() {
    let m = mailbox();
    assert!(found(&m.store, "filename:annex_v2").is_empty());
    assert!(found(&m.store, "filename:%").is_empty());
    assert!(found(&m.store, "from:%").is_empty());
    assert!(found(&m.store, "to:_").is_empty());
    assert!(found(&m.store, "in:inbo_").is_empty());
}

#[test]
fn a_tag_is_found_by_its_whole_name() {
    let m = mailbox();
    let later = m.store.ensure_tag(m.account, "Read later", None).unwrap();
    let draft = m.store.search_threads("subject:draft", 5).unwrap()[0].id;
    m.store.tag_message(draft, later).unwrap();

    assert_eq!(
        found(&m.store, r#"tag:"read later""#),
        ["Draft contract terms"]
    );
    assert_eq!(
        found(&m.store, r#"tag:"READ LATER""#),
        ["Draft contract terms"]
    );
    assert!(found(&m.store, "tag:read").is_empty(), "not part of a name");
    assert_eq!(
        found(&m.store, r#"contract -tag:"read later""#),
        ["Invoice 2214", "Q3 vendor contracts"]
    );
}

/* How words combine. */

#[test]
fn a_phrase_is_those_words_side_by_side() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "board pack"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
    assert_eq!(found(&m.store, r#""board pack""#), ["Q3 vendor contracts"]);
    assert_eq!(
        found(&m.store, r#"board -"board pack""#),
        ["Draft contract terms"]
    );
}

/// The one that ran backwards. `-draft` used to return only the draft.
#[test]
fn a_minus_excludes_rather_than_requires() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "contract -draft"),
        ["Invoice 2214", "Q3 vendor contracts"]
    );
    // With nothing else to look for it is everything but.
    let without = found(&m.store, "-draft");
    assert_eq!(without.len(), EVERYTHING - 1);
    assert!(!without.contains(&"Draft contract terms".to_string()));
    // Two at once, and an operator among them.
    assert_eq!(
        found(&m.store, "contract -draft -from:sam"),
        ["Invoice 2214"]
    );
}

#[test]
fn or_is_either_side_and_binds_looser_than_and() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "from:sam OR from:dana"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
    // (sam and lunch) or dana — not sam-or-dana, and lunch.
    assert_eq!(
        found(&m.store, "from:sam lunch OR from:dana"),
        ["Draft contract terms"]
    );
    assert_eq!(
        found(&m.store, "lunch OR facture"),
        ["Invoice 2214", "Lunch"]
    );
    // A message both sides find is one result.
    assert_eq!(
        found(&m.store, "annex OR from:sam OR filename:.pdf").len(),
        2
    );
}

#[test]
fn what_matched_words_comes_before_what_only_met_conditions() {
    let m = mailbox();
    assert_eq!(
        in_order(&m.store, "to:dana OR subject:lunch"),
        ["Lunch", "Q3 vendor contracts"],
        "ranked first, then listed, whichever was typed first"
    );
}

#[test]
fn a_lowercase_or_is_a_word() {
    let m = mailbox();
    // Nothing here contains the word "or", so this finds nothing — where
    // the operator would have found both.
    assert!(found(&m.store, "lunch or facture").is_empty());
}

/* As-you-type. */

#[test]
fn the_last_word_is_a_prefix_wherever_the_operators_sit() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "ann has:attachment"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "has:attachment ann"),
        ["Q3 vendor contracts"]
    );
    // Somebody closed the quotes: that word is finished.
    assert!(found(&m.store, r#""ann""#).is_empty());
    // An excluded word is never a prefix. `-dra` must not hide the draft
    // while `-draft` is still being typed — nor anything else with "dra".
    assert_eq!(
        found(&m.store, "contract -dra"),
        [
            "Draft contract terms",
            "Invoice 2214",
            "Q3 vendor contracts"
        ]
    );
}

#[test]
fn punctuation_alone_finds_nothing_rather_than_everything() {
    let m = mailbox();
    assert!(found(&m.store, "in:inbox [").is_empty());
    assert!(found(&m.store, "-").is_empty());
    assert!(found(&m.store, "-[").is_empty());
    // Among real words it is simply not a word.
    assert_eq!(found(&m.store, "lunch &"), ["Lunch"]);
    assert_eq!(found(&m.store, "lunch -&"), ["Lunch"]);
}

/* State and place. */

#[test]
fn read_and_unread_are_each_askable() {
    let m = mailbox();
    let lunch = m.store.search_threads("subject:lunch", 5).unwrap()[0].id;
    m.store.set_flags(lunch, flags::SEEN, 0).unwrap();

    assert_eq!(found(&m.store, "is:read"), ["Lunch"]);
    assert_eq!(found(&m.store, "is:unread").len(), EVERYTHING - 1);
    assert_eq!(found(&m.store, "-is:unread"), ["Lunch"]);
    assert_eq!(found(&m.store, "invoice -is:read"), ["Invoice 2214"]);
    assert!(found(&m.store, "is:read is:unread").is_empty());
    // A bracket shared between two states, which is the one way of asking for
    // either of them: `is:read is:unread` is nothing, as it should be.
    m.store.set_flags(lunch, flags::FLAGGED, 0).unwrap();
    assert_eq!(
        found(&m.store, "is:(read OR starred)"),
        found(&m.store, "is:read OR is:starred")
    );
    assert_eq!(found(&m.store, "is:(read OR starred)"), ["Lunch"]);
    assert_eq!(
        found(&m.store, "is:(unread OR starred)").len(),
        EVERYTHING,
        "every message is one or the other"
    );
    assert_eq!(
        found(&m.store, "invoice -is:(read OR starred)"),
        ["Invoice 2214"]
    );
}

/// Junk stays out unless it was asked for, and asking on one side of an `OR`
/// is not asking on the other.
#[test]
fn the_bin_is_let_in_one_alternative_at_a_time() {
    let m = mailbox();
    let spam = m.store.ensure_folder(m.account, "spam", "Junk").unwrap();
    let lunch = m.store.search_threads("subject:lunch", 5).unwrap()[0].id;
    assert!(m.store.remove_placement(lunch, m.account, "INBOX").unwrap());
    m.store.place_message(lunch, spam).unwrap();

    assert_eq!(found(&m.store, "invoice"), ["Invoice 2214"]);
    assert_eq!(found(&m.store, "in:spam invoice"), ["Lunch"]);
    assert_eq!(
        found(&m.store, "in:spam OR subject:invoice"),
        ["Invoice 2214", "Lunch"]
    );
    // The right-hand side never asked for spam, so it does not get it.
    assert_eq!(
        found(&m.store, "in:spam facture OR invoice"),
        ["Invoice 2214"]
    );
    // Excluding the bin is not asking for it.
    assert_eq!(found(&m.store, "invoice -in:spam"), ["Invoice 2214"]);
    assert_eq!(found(&m.store, "-in:inbox invoice"), Vec::<String>::new());
}

/* When. */

/// 07 §5.1: a date is that calendar day where the reader is, not in UTC.
///
/// Forty-eight messages an hour apart straddle two midnights in whatever
/// zone this runs in. The store says which local day each one falls on;
/// `date:`, `after:` and `before:` have to agree with it message for message,
/// boundaries included. The two directions use different halves of SQLite's
/// date code, so this is a real check and not the code agreeing with itself.
#[test]
fn a_date_is_a_day_where_the_reader_is() {
    let mut store = Store::open_in_memory().unwrap();
    let account = store.ensure_test_account().unwrap();
    let inbox = store.ensure_folder(account, "inbox", "INBOX").unwrap();
    const HOUR: i64 = 3_600_000;
    // 2026-08-18T00:00Z, with no daylight-saving change near it anywhere.
    let start = 1_787_011_200_000;
    let hours: Vec<i64> = (0..48).map(|h| start + h * HOUR).collect();
    let msgs: Vec<NewMessage> = hours
        .iter()
        .map(|at| NewMessage {
            account_id: account,
            date_ms: *at,
            from_addr: "clock@example.com".into(),
            from_display: "Clock".into(),
            to_addr: "me@example.com".into(),
            subject: format!("hour {at}"),
            body_text: "tick".into(),
        })
        .collect();
    for id in store.insert_messages(&msgs).unwrap() {
        store.place_message(id, inbox).unwrap();
    }

    let day_of = |at: i64| {
        let d = store.local_day(at).unwrap();
        Period::day(d.year, d.month, d.day).unwrap()
    };
    let subjects = |wanted: &dyn Fn(i64) -> bool| {
        let mut s: Vec<String> = hours
            .iter()
            .filter(|at| wanted(**at))
            .map(|at| format!("hour {at}"))
            .collect();
        s.sort();
        s
    };
    let search = |q: String| {
        let mut s: Vec<String> = store
            .search_threads(&q, 200)
            .unwrap()
            .into_iter()
            .map(|r| r.subject)
            .collect();
        s.sort();
        s
    };

    // The day in the middle of the run is whole, whatever the zone.
    let day = day_of(start + 24 * HOUR);
    let on_it = |at: i64| day_of(at) == day;
    let first = hours.iter().copied().find(|at| on_it(*at)).unwrap();
    assert!(
        hours.iter().filter(|at| on_it(**at)).count() >= 23,
        "a whole day of the run"
    );

    assert_eq!(search(format!("date:{day}")), subjects(&on_it));
    // `after:` takes in the day it names; `before:` leaves it out.
    assert_eq!(search(format!("after:{day}")), subjects(&|at| at >= first));
    assert_eq!(search(format!("before:{day}")), subjects(&|at| at < first));
    assert_eq!(search(format!("-after:{day}")), subjects(&|at| at < first));
    // And together they are exactly a range: this day and no other.
    let next = {
        let d = day.day_after();
        Period::day(d.year, d.month, d.day).unwrap()
    };
    assert_eq!(
        search(format!("after:{day} before:{next}")),
        subjects(&on_it)
    );
    // With words as well as without: the ranked path binds the same dates.
    assert_eq!(search(format!("tick date:{day}")), subjects(&on_it));

    // A month and a year are periods too.
    let d = day.first_day();
    let month = Period::month(d.year, d.month).unwrap();
    assert_eq!(search(format!("date:{month}")).len(), hours.len());
    assert_eq!(search(format!("date:{}", d.year)).len(), hours.len());
    assert!(search(format!("date:{}", d.year + 1)).is_empty());
    assert!(search(format!("before:{}", d.year)).is_empty());
}

/* CJK: a second index, and the two have to agree about exclusion. */

#[test]
fn cjk_words_combine_like_any_others() {
    let m = mailbox();
    assert_eq!(found(&m.store, "東京"), ["Annex for 東京", "東京の会議"]);
    assert_eq!(found(&m.store, "会議 -東京"), ["大阪"]);
    assert_eq!(found(&m.store, "subject:東京 -annex"), ["東京の会議"]);
    assert_eq!(found(&m.store, "東京 OR 大阪").len(), 3);
    // Excluded with nothing else asked: everything without it.
    assert_eq!(found(&m.store, "-会議").len(), EVERYTHING - 2);
}

/// The wanted word is Latin, so the ranking runs on the Latin index — where
/// `東京` is not a token at all. The exclusion has to be asked of the CJK
/// index on its own, or it silently excludes nothing.
#[test]
fn a_cjk_word_is_excluded_from_a_latin_search() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "annex"),
        ["Annex for 東京", "Q3 vendor contracts"]
    );
    assert_eq!(found(&m.store, "annex -東京"), ["Q3 vendor contracts"]);
}

/* Whatever is typed, the answer is a result and never an error. */

#[test]
fn hostile_queries_never_error_and_never_reach_sql() {
    let m = mailbox();
    let long = "x".repeat(20_000);
    let many = ["a"; 200].join(" OR ");
    for typed in [
        "\"",
        "\"\"",
        "-\"\"",
        "--",
        "OR OR OR",
        "a OR",
        "(",
        ")",
        "(a OR b) c",
        "a AND b",
        "NOT",
        "a NOT b",
        "NEAR(a b)",
        "NEAR(",
        "*",
        "a*",
        "^a",
        "a + b",
        "{subject} : a",
        "subject:\"",
        "subject:(",
        "subject:*",
        "subject:a\"b",
        "-subject:)",
        "body_text:annex",
        "from:\\",
        "from:'",
        "tag:' OR 1=1 --",
        "filename:'; DROP TABLE messages; --",
        "in:\"",
        "in:%' ESCAPE '",
        "annex\u{0}pricing",
        "東京 -\"",
        "-東京 -\"会議",
        "subject:東京 -subject:(",
        "date:2026-02-29",
        "after:9998 before:1970",
        long.as_str(),
        many.as_str(),
    ] {
        let shown: String = typed.chars().take(40).collect();
        m.store
            .search_threads(typed, 20)
            .unwrap_or_else(|e| panic!("{shown:?} failed: {e}"));
    }
    // And the mailbox is as it was.
    assert_eq!(found(&m.store, "has:attachment").len(), 2);
}

/* Brackets, AND and NOT. */

#[test]
fn brackets_say_which_things_are_either_or() {
    let m = mailbox();
    // Without brackets: sam, or (dana and "contract").
    assert_eq!(
        found(&m.store, "from:sam OR from:dana contract"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
    // With them the word applies to both senders, and sam's says "contracts".
    assert_eq!(
        found(&m.store, "(from:sam OR from:dana) annex"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "(annex OR facture) has:attachment"),
        ["Invoice 2214", "Q3 vendor contracts"]
    );
    // Both groups at once: still one question, and the same answer.
    assert_eq!(
        found(&m.store, "(from:sam OR from:élodie) (annex OR facture)"),
        ["Invoice 2214", "Q3 vendor contracts"]
    );
    // Nested, and said aloud.
    assert_eq!(
        found(&m.store, "((from:sam OR from:dana) AND contract) NOT draft"),
        ["Q3 vendor contracts"]
    );
}

#[test]
fn not_is_the_minus_and_a_group_can_be_excluded() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "contract NOT draft"),
        found(&m.store, "contract -draft")
    );
    assert_eq!(
        found(&m.store, "contract -(draft OR facture)"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "contract NOT (from:dana OR from:élodie)"),
        ["Q3 vendor contracts"]
    );
    // Not (a and b) is (not a) or (not b): everything that is not both.
    let not_both = found(&m.store, "-(contract draft)");
    assert_eq!(not_both.len(), EVERYTHING - 1);
    assert!(!not_both.contains(&"Draft contract terms".to_string()));
    // Excluded words beside a condition, inside an OR.
    assert_eq!(
        found(&m.store, "invoice (from:billing OR -subject:invoice)"),
        ["Lunch"]
    );
}

#[test]
fn an_or_of_words_and_a_condition_asks_each_in_its_own_way() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "(lunch OR to:dana) -facture"),
        ["Lunch", "Q3 vendor contracts"]
    );
    // Two scripts cannot share an index, so they cannot share an expression:
    // asked of the CJK index alone, "tokyo" in Latin-only mail would be lost.
    assert_eq!(found(&m.store, "大阪 OR lunch"), ["Lunch", "大阪"]);
    assert_eq!(found(&m.store, "(大阪 OR lunch) from:sato"), ["大阪"]);
}

#[test]
fn the_bin_is_let_in_one_side_at_a_time_inside_brackets_too() {
    let m = mailbox();
    let spam = m.store.ensure_folder(m.account, "spam", "Junk").unwrap();
    let lunch = m.store.search_threads("subject:lunch", 5).unwrap()[0].id;
    assert!(m.store.remove_placement(lunch, m.account, "INBOX").unwrap());
    m.store.place_message(lunch, spam).unwrap();
    let invoice = m.store.search_threads("subject:invoice", 5).unwrap()[0].id;
    assert!(
        m.store
            .remove_placement(invoice, m.account, "INBOX")
            .unwrap()
    );
    m.store.place_message(invoice, spam).unwrap();

    // Both invoices are in spam now. Naming spam lets them in…
    assert_eq!(
        found(&m.store, "(in:spam OR in:inbox) invoice"),
        ["Invoice 2214", "Lunch"]
    );
    // …but the other side of the OR never asked, and still may not have them.
    assert_eq!(
        found(&m.store, "(in:spam from:billing OR from:élodie) invoice"),
        ["Lunch"]
    );
}

/// `DO NOT REPLY` reads as Boolean and so leaves out the very mail it names.
/// The chip the field offers writes the keyword in quotes, and that finds it.
#[test]
fn a_keyword_in_quotes_is_the_word() {
    let mut m = mailbox();
    m.store
        .ingest_raw(
            &BlobStore::open(&m._dir.path().join("blobs")).unwrap(),
            m.account,
            None,
            None,
            &raw(&Mail {
                id: "noreply",
                from: "noreply@example.com",
                to: "me@example.com",
                cc: "",
                subject: "Your statement",
                body: "This mailbox is not monitored. Please do not reply to this message.",
                file: None,
            }),
        )
        .unwrap();
    assert!(found(&m.store, "DO NOT REPLY").is_empty());
    // What the chip writes: the line as a phrase, those words in that order.
    assert_eq!(found(&m.store, r#""DO NOT REPLY""#), ["Your statement"]);
    assert_eq!(found(&m.store, r#"DO "NOT" REPLY"#), ["Your statement"]);
    assert_eq!(found(&m.store, "do not reply"), ["Your statement"]);
    assert!(found(&m.store, "statement NOT reply").is_empty());
}

/* What a review of the grammar found, each held here so it stays found. */

/// An `OR` with a word on one side and a condition on the other is asked a
/// statement per side, and several of them multiply. Past the limit the
/// first few statements were kept and the rest left out, so a query whose
/// only match took the second side of its first choice found nothing, and
/// nothing said so. It is asked whole now, as lookups.
#[test]
fn a_query_too_wide_to_multiply_out_is_still_answered() {
    let m = mailbox();
    // Four choices are sixteen statements.
    assert_eq!(
        found(
            &m.store,
            "(zzone OR from:sam) (zztwo OR to:dana) (zzthree OR cc:legal) (zzfour OR has:attachment)"
        ),
        ["Q3 vendor contracts"]
    );
    // Nine alternatives, each a word and a condition, and the match in the last.
    let nine = (0..8)
        .map(|i| format!("(zz{i} from:nobody{i})"))
        .chain(["(annex from:sam)".to_string()])
        .collect::<Vec<_>>()
        .join(" OR ");
    assert_eq!(found(&m.store, &nine), ["Q3 vendor contracts"]);
    // The word still being typed is still a prefix there…
    assert_eq!(
        found(
            &m.store,
            "(zzone OR from:sam) (zztwo OR to:dana) (zzthree OR cc:legal) (zzfour OR contr)"
        ),
        ["Q3 vendor contracts"]
    );
    // …an excluded word is still excluded, and nothing matches by default.
    assert!(
        found(
            &m.store,
            "(zzone OR from:sam) (zztwo OR to:dana) (zzthree OR cc:legal) (zzfour OR has:attachment) -annex"
        )
        .is_empty()
    );
    assert!(
        found(
            &m.store,
            "(zzone OR from:nobody) (zztwo OR to:dana) (zzthree OR cc:legal) (zzfour OR has:attachment)"
        )
        .is_empty()
    );
}

/// The CJK index holds a message's subject and body, a character to a token,
/// and nothing else. A Latin word excluded beside a CJK one was asked there:
/// it never saw the address `sato` is in, and it lost the phrase in
/// `"board pack"` and dropped mail that had the two words apart.
#[test]
fn a_latin_word_is_excluded_from_a_cjk_search() {
    let mut m = mailbox();
    m.store
        .ingest_raw(
            &BlobStore::open(&m._dir.path().join("blobs")).unwrap(),
            m.account,
            None,
            None,
            &raw(&Mail {
                id: "crates",
                from: "Sam Ortiz <sam@example.com>",
                to: "me@example.com",
                cc: "",
                subject: "Crates for 東京",
                body: "We pack the crates once the board has met. 東京",
                file: None,
            }),
        )
        .unwrap();
    assert_eq!(
        found(&m.store, "東京"),
        ["Annex for 東京", "Crates for 東京", "東京の会議"]
    );
    assert_eq!(found(&m.store, "東京 -sato"), ["Crates for 東京"]);
    assert_eq!(found(&m.store, "東京 -from:sato"), ["Crates for 東京"]);
    // Both words, but not side by side: the phrase is not there to exclude.
    assert_eq!(
        found(&m.store, r#"東京 -"board pack""#),
        ["Annex for 東京", "Crates for 東京", "東京の会議"]
    );
    assert_eq!(
        found(&m.store, "東京 -crates"),
        ["Annex for 東京", "東京の会議"]
    );

    // Wanted beside a CJK word, a Latin one is asked of its own index too:
    // an address is found, a phrase is a phrase, and a word still being
    // typed is still a prefix.
    assert_eq!(
        found(&m.store, "東京 sato"),
        ["Annex for 東京", "東京の会議"]
    );
    assert_eq!(found(&m.store, "東京 sam"), ["Crates for 東京"]);
    assert!(found(&m.store, r#"東京 "board pack""#).is_empty());
    assert_eq!(
        found(&m.store, r#"東京 "pack the crates""#),
        ["Crates for 東京"]
    );
    assert_eq!(found(&m.store, "東京 ann"), ["Annex for 東京"]);
    assert_eq!(
        found(&m.store, "(東京 annex) OR lunch"),
        ["Annex for 東京", "Lunch"]
    );
    assert_eq!(found(&m.store, "東京 -annex -crates"), ["東京の会議"]);
}

/// A choice keeps the sides that did not name the bin out of it. Not when
/// the bin is what the whole of it is being asked of.
#[test]
fn a_choice_inside_the_bin_is_not_kept_out_of_it() {
    let m = mailbox();
    let trash = m.store.ensure_folder(m.account, "trash", "Trash").unwrap();
    let q3 = m.store.search_threads("subject:vendor", 5).unwrap()[0].id;
    assert!(m.store.remove_placement(q3, m.account, "INBOX").unwrap());
    m.store.place_message(q3, trash).unwrap();

    assert_eq!(
        found(&m.store, "in:trash from:sam"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "in:trash (in:spam OR from:sam)"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "in:trash (from:dana OR from:sam)"),
        ["Q3 vendor contracts"]
    );
    // Outside the bin the choice still keeps its other side out.
    assert!(found(&m.store, "in:spam OR from:sam").is_empty());
}

/// `is:snoozed` lifts the inbox's rule about snoozed mail for what it is
/// asked alongside, not for the whole statement.
#[test]
fn asking_for_snoozed_mail_on_one_side_does_not_show_it_on_the_other() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};

    let m = mailbox();
    let lunch = m.store.search_threads("subject:lunch", 5).unwrap()[0].id;
    let thread = m.store.thread_of(lunch).unwrap().unwrap_or(-lunch);
    m.store
        .apply_thread_action(
            m.account,
            thread,
            ActionKind::Snooze,
            Some(1_900_000_000_000),
            PlacementPolicy::Exclusive,
        )
        .unwrap();

    assert!(found(&m.store, "in:inbox from:billing").is_empty());
    assert_eq!(
        found(&m.store, "in:inbox is:snoozed from:billing"),
        ["Lunch"]
    );
    assert_eq!(
        found(&m.store, "is:snoozed (in:inbox OR in:archive)"),
        ["Lunch"]
    );
    // Lunch is snoozed, so it is not in the inbox; and it is not from Dana.
    assert!(
        found(
            &m.store,
            "(in:inbox from:billing) OR (is:snoozed from:dana)"
        )
        .is_empty()
    );
}

/// The word being typed is the last one the index keeps, and there is one
/// of it for the whole query.
#[test]
fn the_word_being_typed_is_the_same_word_however_the_query_is_asked() {
    let m = mailbox();
    // Starting an exclusion, or a bracket, after a partial word used to take
    // the prefix off it and empty the list for a keystroke.
    for typed in ["lun", "lun -", "lun &", "lun ["] {
        assert_eq!(found(&m.store, typed), ["Lunch"], "{typed}");
    }
    // `ann` is not the last word in either of these, so it is a whole word
    // in both. Chosen a statement at a time it was a prefix in the second.
    assert_eq!(found(&m.store, "ann OR draft"), ["Draft contract terms"]);
    assert_eq!(
        found(&m.store, "ann OR (from:dana draft)"),
        ["Draft contract terms"]
    );
    assert_eq!(
        found(&m.store, "draft OR ann"),
        [
            "Annex for 東京",
            "Draft contract terms",
            "Q3 vendor contracts"
        ]
    );
}

/// Punctuation asks nothing, so as one side of a choice it is no side at
/// all. Written as "true of every message" it listed the whole mailbox for
/// the keystroke before `-[acme]` became a word.
#[test]
fn punctuation_is_no_side_of_a_choice() {
    let m = mailbox();
    assert_eq!(found(&m.store, "lunch OR -["), ["Lunch"]);
    assert_eq!(found(&m.store, "from:sam OR -["), ["Q3 vendor contracts"]);
    assert_eq!(
        found(&m.store, "(& OR from:sam) contract"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(
        found(&m.store, "(& OR annex) contract"),
        ["Q3 vendor contracts"]
    );
    // Among things that all have to hold it is still simply skipped.
    assert_eq!(found(&m.store, "from:sam OR (from:dana -[)").len(), 2);
}

/// Punctuation is not a word, wherever its characters happen to live in
/// Unicode. `・` sits among the Japanese characters but is no more a word
/// than `[` is, and taking it for one sent `・ annex` to the per-character
/// index, where the only real word was thrown away and the search with it.
#[test]
fn cjk_punctuation_is_skipped_like_any_other() {
    let m = mailbox();
    let annex = found(&m.store, "annex");
    assert_eq!(annex, ["Annex for 東京", "Q3 vendor contracts"]);
    for typed in ["・ annex", "annex ・", "゠ annex", "annex ゛"] {
        assert_eq!(found(&m.store, typed), annex, "{typed}");
    }
    // The Latin side of the same rule, unchanged.
    assert_eq!(found(&m.store, "[ annex"), annex);
    // On its own it is not a word, so it finds nothing rather than everything.
    for alone in ["・", "゠", "["] {
        assert!(found(&m.store, alone).is_empty(), "{alone}");
    }
    // Beside a real CJK word it changes nothing either.
    assert_eq!(found(&m.store, "・ 東京"), found(&m.store, "東京"));
}

/// A filter with no words beside it is asked of its whole table in one pass,
/// rather than of each message in turn, because every message is being read
/// anyway: a recipient nobody wrote to went from 135ms to 25ms at a hundred
/// thousand messages. Excluding is where that would go wrong quietly — one
/// missing row in the answer and `NOT IN` is false for everybody — so both
/// directions are held here, with and without a word beside them.
#[test]
fn a_filter_finds_and_excludes_the_same_mail_with_or_without_words() {
    let m = mailbox();
    let urgent = m.store.ensure_tag(m.account, "Urgent", None).unwrap();
    let draft = m.store.search_threads("subject:draft", 5).unwrap()[0].id;
    m.store.tag_message(draft, urgent).unwrap();

    for (finds, excludes, whole) in [
        ("to:dana", "-to:dana", "Q3 vendor contracts"),
        ("cc:legal", "-cc:legal", "Q3 vendor contracts"),
        ("filename:.pdf", "-filename:.pdf", "Q3 vendor contracts"),
        ("tag:urgent", "-tag:urgent", "Draft contract terms"),
    ] {
        assert_eq!(found(&m.store, finds), [whole], "{finds}");
        let rest = found(&m.store, excludes);
        assert_eq!(rest.len(), EVERYTHING - 1, "{excludes}");
        assert!(!rest.contains(&whole.to_string()), "{excludes}");
    }
    // Beside a word the same filter is asked the other way round, and the
    // two have to agree.
    assert_eq!(
        found(&m.store, "contracts to:dana"),
        ["Q3 vendor contracts"]
    );
    assert_eq!(found(&m.store, "contract -to:dana").len(), 2);
    assert_eq!(
        found(&m.store, "draft tag:urgent"),
        ["Draft contract terms"]
    );
    assert_eq!(found(&m.store, "contract -tag:urgent").len(), 2);
    // Either side of an OR, where one side has words and the other does not.
    assert_eq!(
        found(&m.store, "to:dana OR (contract from:dana)"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
}

/// `from:(sam OR dana)` is how Gmail writes it, and it used to find the
/// wrong mail here without a word of warning: `from:(sam` was read as a
/// sender called `(sam`, which matched nobody, while `dana)` became a plain
/// word, so the search quietly returned everything mentioning Dana.
#[test]
fn a_bracket_can_be_shared_between_one_operators_values() {
    let m = mailbox();
    assert_eq!(
        found(&m.store, "from:(sam OR dana)"),
        found(&m.store, "from:sam OR from:dana")
    );
    assert_eq!(
        found(&m.store, "from:(sam OR dana)"),
        ["Draft contract terms", "Q3 vendor contracts"]
    );
    assert_eq!(found(&m.store, "from:(sam)"), ["Q3 vendor contracts"]);
    assert_eq!(
        found(&m.store, "subject:(contract OR invoice)"),
        found(&m.store, "subject:contract OR subject:invoice")
    );
    assert_eq!(
        found(&m.store, "subject:(vendor contracts)"),
        ["Q3 vendor contracts"]
    );
    // The bracket narrows what is beside it, as any bracket does.
    assert_eq!(
        found(&m.store, "from:(sam OR dana) draft"),
        ["Draft contract terms"]
    );
    // And excluded, it takes both senders out.
    assert_eq!(found(&m.store, "-from:(sam OR dana)").len(), EVERYTHING - 2);
}

/// `in:` escapes what was typed as well. The mailbox needs a folder the
/// unescaped pattern would have matched, or this could never fail.
#[test]
fn a_mailbox_name_is_never_a_wildcard() {
    let m = mailbox();
    let filed = m.store.ensure_folder(m.account, "", "Work/Inbox").unwrap();
    let lunch = m.store.search_threads("subject:lunch", 5).unwrap()[0].id;
    m.store.place_message(lunch, filed).unwrap();

    assert_eq!(found(&m.store, "in:work/inbox"), ["Lunch"]);
    assert_eq!(found(&m.store, "in:inbox lunch"), ["Lunch"]);
    assert!(found(&m.store, "in:inbo_").is_empty());
    assert!(found(&m.store, "in:%").is_empty());
}

/* What a saved search's badge asks. */

/// A badge counts what the list shows, or it is a number that lies.
///
/// The count takes a different path on purpose — one statement answered inside
/// SQLite, rather than a page of the ranking rolled up to conversations — so
/// the two are asserted against each other across every shape the grammar has.
#[test]
fn counting_a_query_agrees_with_listing_it() {
    let m = mailbox();
    for query in [
        "invoice",
        "contract",
        "from:sam",
        "from:sato",
        "is:unread",
        "has:attachment",
        "subject:annex",
        "-draft",
        "contract -from:dana",
        "from:sam OR from:dana",
        "(from:sam OR from:dana) contract",
        "from:(sam OR dana)",
        "subject:(annex OR invoice)",
        "is:(unread OR starred)",
        "東京",
        "annex OR 東京",
        "nothing-here-at-all",
        "in:inbox invoice",
    ] {
        let listed = found(&m.store, query).len() as i64;
        assert_eq!(
            m.store.count_search(query, CountMode::Total).unwrap(),
            listed,
            "{query:?}"
        );
    }
}

/// The unread count is the unread half of the same answer, and stays in step
/// when a message is read.
#[test]
fn counting_unread_follows_what_has_been_read() {
    let m = mailbox();
    let total = m.store.count_search("contract", CountMode::Total).unwrap();
    assert_eq!(
        m.store.count_search("contract", CountMode::Unread).unwrap(),
        total,
        "nothing has been read yet"
    );

    let draft = m.store.search_threads("subject:draft", 5).unwrap()[0].id;
    m.store.set_flags(draft, flags::SEEN, 0).unwrap();
    assert_eq!(
        m.store.count_search("contract", CountMode::Unread).unwrap(),
        total - 1
    );
    assert_eq!(
        m.store.count_search("contract", CountMode::Total).unwrap(),
        total,
        "reading one did not change how many there are"
    );
    // Off is a mode the sidebar has, and it asks for nothing rather than
    // counting and throwing the number away.
    assert_eq!(m.store.count_search("contract", CountMode::Off).unwrap(), 0);
}

/// An empty or half-typed query counts nothing rather than everything.
#[test]
fn counting_nothing_is_nothing() {
    let m = mailbox();
    for query in ["", "   ", "from:", "-", "AND", "\"\"", "()"] {
        assert_eq!(
            m.store.count_search(query, CountMode::Total).unwrap(),
            0,
            "{query:?}"
        );
    }
}
