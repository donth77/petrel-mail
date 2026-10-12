//! Conversation threading against the store.
//!
//! The interesting cases are not "a reply joins its parent" but the ones that
//! make threading wrong in real mailboxes: messages arriving out of order,
//! references stripped by mailing lists, generic subjects that must *not*
//! merge, and accounts that must never bleed into each other.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::{ListView, Store};

const DAY: i64 = 24 * 60 * 60 * 1000;
const T0: i64 = 1_800_000_000_000;

/// Builds a message with explicit threading headers.
fn mail(msgid: &str, subject: &str, refs: &[&str], date_ms: i64, body: &str) -> Vec<u8> {
    let mut headers = format!(
        "From: Someone <someone@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: {subject}\r\n\
         Message-ID: <{msgid}>\r\n\
         Date: {}\r\n",
        httpdate(date_ms)
    );
    if let Some((last, rest)) = refs.split_last() {
        if !rest.is_empty() {
            headers.push_str(&format!(
                "References: {}\r\n",
                rest.iter()
                    .map(|r| format!("<{r}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        headers.push_str(&format!("In-Reply-To: <{last}>\r\n"));
    }
    format!("{headers}\r\n{body}\r\n").into_bytes()
}

/// A real RFC 5322 date for a millisecond timestamp.
///
/// The first version of this faked it by walking the day-of-month, which put
/// every message in the same week no matter how far apart the tests placed
/// them — quietly disabling the time-window logic under test. A test fixture
/// that silently collapses the dimension you are testing is worse than no
/// fixture.
fn httpdate(ms: i64) -> String {
    // civil_from_days (Howard Hinnant): days since epoch -> y/m/d.
    let z = ms.div_euclid(DAY) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "Mon, {d:02} {} {year} 12:00:00 +0000",
        MONTHS[(m - 1) as usize]
    )
}

/// Every threading fixture lives in the inbox: the view reads membership.
fn inboxed(store: &Store, account: i64) -> i64 {
    store.ensure_folder(account, "inbox", "INBOX").unwrap()
}

fn setup() -> (tempfile::TempDir, Store, BlobStore, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(&dir.path().join("petrel.db")).expect("store");
    let blobs = BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    (dir, store, blobs, account)
}

#[test]
fn a_reply_chain_becomes_one_conversation() {
    let (_d, mut store, blobs, account) = setup();
    let a = mail("a@x", "Q3 vendor contracts", &[], T0, "opening");
    let b = mail(
        "b@x",
        "Re: Q3 vendor contracts",
        &["a@x"],
        T0 + DAY,
        "reply",
    );
    let c = mail(
        "c@x",
        "Re: Q3 vendor contracts",
        &["a@x", "b@x"],
        T0 + 2 * DAY,
        "third",
    );

    let ids: Vec<i64> = [a, b, c]
        .iter()
        .map(|m| {
            store
                .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, m)
                .expect("ingest")
                .message_id
        })
        .collect();

    let threads: Vec<i64> = ids
        .iter()
        .map(|id| store.thread_of(*id).expect("thread").expect("assigned"))
        .collect();
    assert_eq!(threads[0], threads[1]);
    assert_eq!(threads[1], threads[2]);

    // The list shows one row for three messages.
    let listed = store
        .list_threads(
            &ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .expect("list");
    assert_eq!(listed.len(), 1, "one conversation, not three rows");
    assert_eq!(listed[0].message_count, 3);
    assert_eq!(listed[0].id, ids[2], "the row shows the newest message");

    let msgs = store.messages_in_thread(threads[0]).expect("thread");
    assert_eq!(msgs.len(), 3);
    assert!(
        msgs[0].date_ms <= msgs[2].date_ms,
        "oldest first for reading"
    );
}

/// The case that breaks naive implementations: the middle of a conversation
/// arrives last and has to fuse two separate threads.
#[test]
fn a_late_middle_message_merges_two_threads() {
    let (_d, mut store, blobs, account) = setup();

    // A deliberately generic subject, so the subject fallback cannot fire and
    // this test measures the reference graph alone.
    let first = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("m1@x", "Hi", &[], T0, "start"),
        )
        .expect("m1");
    // m3 references m2, which we have not seen yet — so it starts its own thread.
    let third = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("m3@x", "Re: Hi", &["m2@x"], T0 + 2 * DAY, "third"),
        )
        .expect("m3");

    assert_ne!(
        store.thread_of(first.message_id).unwrap(),
        store.thread_of(third.message_id).unwrap(),
        "without the link they are genuinely separate"
    );

    // The missing middle arrives and closes the gap.
    let second = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("m2@x", "Re: Hi", &["m1@x"], T0 + DAY, "second"),
        )
        .expect("m2");

    let t1 = store.thread_of(first.message_id).unwrap().unwrap();
    let t2 = store.thread_of(second.message_id).unwrap().unwrap();
    let t3 = store.thread_of(third.message_id).unwrap().unwrap();
    assert_eq!(t1, t2, "middle joins the head");
    assert_eq!(t2, t3, "and drags the tail with it");

    let listed = store
        .list_threads(
            &ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .expect("list");
    assert_eq!(listed.len(), 1, "the two threads became one");
    assert_eq!(listed[0].message_count, 3);
}

#[test]
fn stripped_references_fall_back_to_a_distinctive_subject() {
    let (_d, mut store, blobs, account) = setup();
    // Mailing lists routinely drop References; the subject is all that is left.
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("s1@x", "Quarterly logistics review", &[], T0, "one"),
        )
        .expect("a");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail(
                "s2@x",
                "Re: Quarterly logistics review",
                &[],
                T0 + DAY,
                "two",
            ),
        )
        .expect("b");

    assert_eq!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "a distinctive subject is enough evidence"
    );
}

/// `mail` from a sender to the test account's own address and nobody else,
/// so the only correspondent two of these share is the account itself.
fn mail_to_me(from: &str, msgid: &str, subject: &str, date_ms: i64) -> Vec<u8> {
    let raw = String::from_utf8(mail(msgid, subject, &[], date_ms, "hi")).unwrap();
    raw.replacen(
        "From: Someone <someone@example.com>\r\nTo: me@example.com",
        &format!("From: {from}\r\nTo: test@example.com"),
        1,
    )
    .into_bytes()
}

/// Two services' "Your verification code", an hour apart: neither is a reply,
/// and they are two conversations. Joined, a thread-wide Delete forever on
/// one deleted the other.
#[test]
fn two_messages_neither_a_reply_are_not_joined_by_subject() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_from(
                "GitHub <noreply@github.example>",
                "v1@x",
                "Your verification code",
                &[],
                T0,
            ),
        )
        .expect("a");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_from(
                "Bank <alerts@bank.example>",
                "v2@x",
                "Your verification code",
                &[],
                T0 + DAY / 24,
            ),
        )
        .expect("b");
    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap()
    );
}

/// The reply can arrive first — backfill walks newest first — and the post
/// still joins it.
#[test]
fn a_post_joins_the_reply_that_arrived_before_it() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let reply = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail("o2@x", "Re: Offsite agenda draft", &[], T0 + DAY, "two"),
        )
        .expect("reply");
    let post = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail("o1@x", "Offsite agenda draft", &[], T0, "one"),
        )
        .expect("post");
    assert_eq!(
        store.thread_of(reply.message_id).unwrap(),
        store.thread_of(post.message_id).unwrap()
    );
}

/// A "Re:" from someone the conversation never included, sharing nothing with
/// it but the account's own address, is not part of it.
#[test]
fn a_reply_sharing_only_the_account_is_not_joined_by_subject() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let post = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_to_me(
                "Alice <alice@example.com>",
                "p1@x",
                "Invoice 4471 overdue",
                T0,
            ),
        )
        .expect("post");
    let stranger = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_to_me(
                "Mallory <mallory@elsewhere.example>",
                "p2@x",
                "Re: Invoice 4471 overdue",
                T0 + DAY,
            ),
        )
        .expect("stranger");
    assert_ne!(
        store.thread_of(post.message_id).unwrap(),
        store.thread_of(stranger.message_id).unwrap()
    );
    let answer = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_from(
                "Alice <alice@example.com>",
                "p3@x",
                "Re: Invoice 4471 overdue",
                &[],
                T0 + 2 * DAY,
            ),
        )
        .expect("answer");
    assert_eq!(
        store.thread_of(post.message_id).unwrap(),
        store.thread_of(answer.message_id).unwrap(),
        "Alice's own follow-up is the same conversation"
    );
}

/// Mail in the bin is not a conversation to join: a reply arriving later
/// would otherwise sit in a thread whose other half Empty Trash deletes.
#[test]
fn nothing_joins_a_message_in_the_bin_by_subject() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let trash = store.ensure_folder(account, "trash", "Trash").unwrap();
    let binned = store
        .ingest_raw(
            &blobs,
            account,
            Some(trash),
            Some(1),
            &mail("b1@x", "Venue shortlist for March", &[], T0, "one"),
        )
        .expect("binned");
    let reply = store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            Some(2),
            &mail(
                "b2@x",
                "Re: Venue shortlist for March",
                &[],
                T0 + DAY,
                "two",
            ),
        )
        .expect("reply");
    assert_ne!(
        store.thread_of(binned.message_id).unwrap(),
        store.thread_of(reply.message_id).unwrap()
    );
}

/// The failure mode that matters more than a missed merge: a *wrong* merge
/// hides mail inside an unrelated conversation, where nobody looks for it.
#[test]
fn generic_subjects_do_not_merge_strangers() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("g1@x", "Hi", &[], T0, "from alice"),
        )
        .expect("a");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("g2@x", "Hi", &[], T0 + DAY, "from bob"),
        )
        .expect("b");

    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "\"Hi\" is not evidence of a conversation"
    );
    assert_eq!(
        store
            .list_threads(
                &ListView::Inbox,
                0,
                20,
                petrel_engine::store::Sort::default()
            )
            .expect("list")
            .len(),
        2
    );
}

#[test]
fn the_subject_window_stops_distant_mail_from_joining() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("w1@x", "Annual budget planning", &[], T0, "this year"),
        )
        .expect("a");
    // Same subject, a year later — almost certainly a new conversation.
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail(
                "w2@x",
                "Annual budget planning",
                &[],
                T0 + 365 * DAY,
                "next year",
            ),
        )
        .expect("b");

    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "a recurring subject a year apart is not one thread"
    );
}

#[test]
fn threads_never_span_accounts() {
    let (_d, mut store, blobs, account) = setup();
    let other = store.ensure_test_account().expect("second account");
    let raw = mail(
        "shared@x",
        "Q3 vendor contracts",
        &[],
        T0,
        "same message, two accounts",
    );

    let a = store
        .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, &raw)
        .expect("a");
    let b = store
        .ingest_raw(&blobs, other, Some(inboxed(&store, other)), None, &raw)
        .expect("b");

    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "each account keeps its own conversation; actions must not cross"
    );
    // And the list shows one account at a time — never both merged. A
    // merged inbox is the single largest source of send-from-the-wrong-
    // address mistakes, and it was exactly what this listed before the
    // query learned which account was on screen.
    assert_eq!(
        store
            .list_threads(
                &ListView::Inbox,
                0,
                20,
                petrel_engine::store::Sort::default()
            )
            .expect("list")
            .len(),
        1
    );
    store.set_active_account(other).unwrap();
    let rows = store
        .list_threads(
            &ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].id, b.message_id,
        "the other account's copy, once switched"
    );
}

#[test]
fn deleted_messages_leave_the_conversation() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail("d1@x", "Vendor contract terms", &[], T0, "one"),
        )
        .expect("a");
    store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &mail(
                "d2@x",
                "Re: Vendor contract terms",
                &["d1@x"],
                T0 + DAY,
                "two",
            ),
        )
        .expect("b");

    let thread = store.thread_of(a.message_id).unwrap().unwrap();
    assert_eq!(store.messages_in_thread(thread).expect("msgs").len(), 2);

    // The server drops the reply.
    store
        .reconcile_server_absences(account, &["d1@x".to_string()], T0 + 2 * DAY)
        .expect("reconcile");

    assert_eq!(
        store.messages_in_thread(thread).expect("msgs").len(),
        1,
        "a deleted message must vanish from the conversation too"
    );
    let listed = store
        .list_threads(
            &ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .expect("list");
    assert_eq!(listed[0].message_count, 1);
}

#[test]
fn resyncing_a_thread_does_not_inflate_its_count() {
    let (_d, mut store, blobs, account) = setup();
    let msgs = [
        mail("r1@x", "Shipping schedule", &[], T0, "one"),
        mail("r2@x", "Re: Shipping schedule", &["r1@x"], T0 + DAY, "two"),
    ];
    for _ in 0..3 {
        for m in &msgs {
            store
                .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, m)
                .expect("ingest");
        }
    }
    let listed = store
        .list_threads(
            &ListView::Inbox,
            0,
            20,
            petrel_engine::store::Sort::default(),
        )
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].message_count, 2,
        "three syncs, still two messages"
    );
}

/// Thread rows roll flags up from their messages: a conversation is unread if
/// any message in it is, which is what pulls a replied-to thread back to the top
/// of attention rather than leaving it looking handled.
#[test]
fn thread_rows_roll_up_flags_from_their_messages() {
    use petrel_engine::store::flags;

    let (_d, mut store, blobs, account) = setup();
    let a = mail("roll-a@x", "Contract terms", &[], T0, "first");
    let b = mail(
        "roll-b@x",
        "Re: Contract terms",
        &["roll-a@x"],
        T0 + DAY,
        "reply",
    );

    let ia = store
        .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, &a)
        .unwrap();
    let ib = store
        .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, &b)
        .unwrap();
    // Older message read; the reply unread, starred, and carrying a file.
    store.set_flags(ia.message_id, flags::SEEN, 0).unwrap();
    store
        .set_flags(ib.message_id, flags::FLAGGED, flags::SEEN)
        .unwrap();
    store.set_has_attachments(ib.message_id, true).unwrap();

    let rows = store
        .list_threads(
            &ListView::Inbox,
            0,
            10,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.subject.contains("Contract terms"))
        .expect("the conversation should appear as one row");

    assert_eq!(row.message_count, 2, "two messages, one row");
    assert!(
        row.unread,
        "one unread message makes the whole conversation unread"
    );
    assert!(row.starred, "flagging any message stars the conversation");
    assert!(
        row.has_attachments,
        "an attachment anywhere shows on the row"
    );

    // And the inverse: reading everything clears it.
    store.set_flags(ib.message_id, flags::SEEN, 0).unwrap();
    let rows = store
        .list_threads(
            &ListView::Inbox,
            0,
            10,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    let row = rows
        .iter()
        .find(|r| r.subject.contains("Contract terms"))
        .unwrap();
    assert!(!row.unread, "reading every message clears the conversation");
    assert!(row.starred, "but the star survives being read");
}

/// Search shows conversations, so a query matching four messages in one thread
/// must return that thread once — not four near-identical rows.
#[test]
fn search_collapses_hits_to_one_row_per_conversation() {
    let (_d, mut store, blobs, account) = setup();
    for (i, id) in ["s1@x", "s2@x", "s3@x"].iter().enumerate() {
        let refs: Vec<&str> = if i == 0 { vec![] } else { vec!["s1@x"] };
        let m = mail(
            id,
            if i == 0 {
                "Annex review"
            } else {
                "Re: Annex review"
            },
            &refs,
            T0 + i as i64 * DAY,
            "the annex needs a signature",
        );
        store
            .ingest_raw(&blobs, account, Some(inboxed(&store, account)), None, &m)
            .unwrap();
    }
    // A separate conversation that also matches.
    let other = mail(
        "other@x",
        "Unrelated annex question",
        &[],
        T0 + 9 * DAY,
        "annex",
    );
    store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            &other,
        )
        .unwrap();

    let msg_hits = store.search_listing("annex", 50).unwrap();
    assert!(msg_hits.len() >= 4, "the query matches several messages");

    let rows = store.search_threads("annex", 50).unwrap();
    assert_eq!(rows.len(), 2, "…but only two conversations");

    let chain = rows
        .iter()
        .find(|r| r.subject.contains("Annex review"))
        .unwrap();
    assert_eq!(chain.message_count, 3, "the row reports the whole thread");
}

/// Mail must never be invisible. A message whose thread was never assigned is a
/// conversation of one — not a row that silently vanishes because `NULL = NULL`
/// is false and the join dropped it.
#[test]
fn messages_without_a_thread_still_appear_as_single_conversations() {
    let (_d, mut store, _blobs, account) = setup();

    // insert_messages is the bulk path — it carries no headers, so nothing here
    // can be threaded even in principle.
    let msgs: Vec<_> = (0..3)
        .map(|i| petrel_engine::store::NewMessage {
            account_id: account,
            date_ms: T0 + i * DAY,
            from_addr: "bulk@example.com".into(),
            from_display: "Bulk".into(),
            to_addr: "me@example.com".into(),
            subject: format!("Unthreaded {i}"),
            body_text: "body".into(),
        })
        .collect();
    let ids = store.insert_messages(&msgs).unwrap();
    for id in &ids {
        store.place_message(*id, inboxed(&store, account)).unwrap();
    }
    assert_eq!(ids.len(), 3);

    let rows = store
        .list_threads(
            &ListView::Inbox,
            0,
            50,
            petrel_engine::store::Sort::default(),
        )
        .unwrap();
    assert_eq!(rows.len(), 3, "every unthreaded message gets its own row");
    for r in &rows {
        assert_eq!(r.message_count, 1, "a conversation of one");
    }

    // And search must not drop them either.
    let hits = store.search_threads("Unthreaded", 50).unwrap();
    assert_eq!(hits.len(), 3, "search returns them too");
}

/// thread_detail hydrates recipients and files for invitations and tests.
/// The first version of this query referenced a column that does not exist,
/// and only the running app found out.
#[test]
fn thread_detail_returns_recipients_and_files() {
    let (_d, mut store, blobs, account) = setup();
    let raw = format!(
        "From: Sam Ortiz <sam@vendorco.example>\r\n\
         To: Dana Wu <dana@northbay.example>, me@example.com\r\n\
         Cc: Legal <legal@northbay.example>\r\n\
         Subject: Contract terms\r\n\
         Message-ID: <detail-1@x>\r\n\
         Date: {}\r\n\r\nbody text\r\n",
        httpdate(T0)
    );
    let ing = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            raw.as_bytes(),
        )
        .unwrap();
    let tid = store
        .thread_of(ing.message_id)
        .unwrap()
        .unwrap_or(-ing.message_id);

    let msgs = store.thread_detail(tid).unwrap();
    assert_eq!(msgs.len(), 1, "one message in this conversation");

    let m = &msgs[0];
    assert_eq!(m.id, ing.message_id);
    assert!(
        m.from_display.contains("Sam"),
        "sender display: {:?}",
        m.from_display
    );
    assert!(
        m.recipients.iter().any(|r| r.contains("Dana")),
        "To recipients must be present: {:?}",
        m.recipients
    );
    assert!(
        m.recipients.iter().any(|r| r.contains("Legal")),
        "Cc recipients count as recipients too: {:?}",
        m.recipients
    );
    assert!(
        m.to.iter().any(|r| r.contains("Dana")),
        "To line is To, not Cc: {:?}",
        m.to
    );
    assert!(
        m.cc.iter().any(|r| r.contains("Legal")),
        "Cc line is Cc, not folded into To: {:?}",
        m.cc
    );
    assert!(
        !m.to.iter().any(|r| r.contains("Legal")),
        "Cc must not appear on To: {:?}",
        m.to
    );
    assert!(m.attachments.is_empty(), "this message carries no files");
}

#[test]
fn thread_detail_keeps_every_cc_on_its_own_line() {
    let (_d, mut store, blobs, account) = setup();
    let raw = format!(
        "From: Sam Ortiz <sam@vendorco.example>\r\n\
         To: me@example.com\r\n\
         Cc: one@example.com, two@example.com, three@example.com, \
four@example.com, five@example.com\r\n\
         Subject: Many copies\r\n\
         Message-ID: <detail-cc-5@x>\r\n\
         Date: {}\r\n\r\nbody text\r\n",
        httpdate(T0)
    );
    let ing = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            raw.as_bytes(),
        )
        .unwrap();
    let tid = store
        .thread_of(ing.message_id)
        .unwrap()
        .unwrap_or(-ing.message_id);
    let m = &store.thread_detail(tid).unwrap()[0];
    assert_eq!(
        m.cc,
        [
            "one@example.com",
            "two@example.com",
            "three@example.com",
            "four@example.com",
            "five@example.com"
        ]
    );
    assert_eq!(m.to, ["me@example.com"]);
}

#[test]
fn thread_detail_keeps_stacked_cc_headers() {
    let (_d, mut store, blobs, account) = setup();
    let raw = format!(
        "From: Sam Ortiz <sam@vendorco.example>\r\n\
         To: me@example.com\r\n\
         Cc: first@example.com\r\n\
         Cc: second@example.com\r\n\
         Subject: Stacked copies\r\n\
         Message-ID: <detail-cc-stack@x>\r\n\
         Date: {}\r\n\r\nbody text\r\n",
        httpdate(T0)
    );
    let ing = store
        .ingest_raw(
            &blobs,
            account,
            Some(inboxed(&store, account)),
            None,
            raw.as_bytes(),
        )
        .unwrap();
    let tid = store
        .thread_of(ing.message_id)
        .unwrap()
        .unwrap_or(-ing.message_id);
    let m = &store.thread_detail(tid).unwrap()[0];
    assert_eq!(m.cc, ["first@example.com", "second@example.com"]);
}

#[test]
fn gmail_thread_ids_are_authoritative_where_known() {
    // Two notification messages with no References thread alone under JWZ;
    // Gmail says they are one conversation. And a shared subject once glued
    // two messages Gmail says are different conversations. The regroup
    // makes Gmail's word the store's word, both directions.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = petrel_engine::store::Store::open(&dir.path().join("p.db")).expect("store");
    let blobs = petrel_engine::blob::BlobStore::open(&dir.path().join("blobs")).expect("blobs");
    let account = store.ensure_test_account().expect("account");
    let all = store
        .ensure_folder(account, "archive", "[Gmail]/All Mail")
        .expect("folder");
    let raw = |mid: &str, subject: &str| {
        format!(
            "From: Jobs <no-reply@example.com>\r\nTo: me@example.com\r\n\
             Subject: {subject}\r\nDate: Tue, 18 Aug 2026 14:02:00 +0000\r\n\
             Message-ID: <{mid}>\r\nMIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\nbody\r\n"
        )
        .into_bytes()
    };
    // Distinct subjects: JWZ threads them apart.
    let a = store
        .ingest_raw(
            &blobs,
            account,
            Some(all),
            Some(1),
            &raw("a@x", "Your application"),
        )
        .expect("ingest");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            Some(all),
            Some(2),
            &raw("b@x", "Interview times"),
        )
        .expect("ingest");
    // A distinctive subject and a reply to it, same hour, no References: the
    // subject fallback glues them together.
    let c = store
        .ingest_raw(
            &blobs,
            account,
            Some(all),
            Some(3),
            &raw("c@x", "Quarterly planning review"),
        )
        .expect("ingest");
    let d = store
        .ingest_raw(
            &blobs,
            account,
            Some(all),
            Some(4),
            &raw("d@x", "Re: Quarterly planning review"),
        )
        .expect("ingest");
    let thread_of = |store: &petrel_engine::store::Store, id| {
        store
            .thread_of(id)
            .expect("thread lookup")
            .expect("threaded")
    };
    assert_ne!(
        thread_of(&store, a.message_id),
        thread_of(&store, b.message_id)
    );
    assert_eq!(
        thread_of(&store, c.message_id),
        thread_of(&store, d.message_id),
        "the subject fallback glued these — the setup this test needs"
    );

    // Gmail's word: a+b are one conversation; c and d are two.
    let applied = store
        .apply_gm_thrids(all, &[(1, 900), (2, 900), (3, 901), (4, 902)])
        .expect("apply");
    assert_eq!(applied, 4);
    let regrouped = store.regroup_gmail_threads(account).expect("regroup");
    // a and c already sit on their canonical ids; b joins a, d leaves c.
    assert_eq!(regrouped, 2, "b and d move");
    assert_eq!(
        thread_of(&store, a.message_id),
        thread_of(&store, b.message_id),
        "References could not connect these; X-GM-THRID does"
    );
    assert_ne!(
        thread_of(&store, c.message_id),
        thread_of(&store, d.message_id),
        "a shared subject is not a shared conversation when Gmail says otherwise"
    );
    // Idempotent: nothing left to move.
    assert_eq!(store.regroup_gmail_threads(account).expect("again"), 0);
}

/// The cursor conversation may have moved since the page was loaded: a reply
/// lands and it jumps to the top. The next page must follow from where the
/// conversation *was*, not from its new place — "after it" from the top is
/// the whole mailbox again, and every row of that page is already on screen.
#[test]
fn a_list_page_follows_a_cursor_that_gained_a_reply() {
    use petrel_engine::store::Sort;
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    for i in 0..6 {
        let raw = mail(
            &format!("root-{i}"),
            &format!("Thread {i}"),
            &[],
            T0 + i * DAY,
            "hi",
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), None, &raw)
            .unwrap();
    }
    let sort = Sort::default();
    let before = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
    let first = store.list_threads(&ListView::Inbox, 0, 3, sort).unwrap();
    let cursor = &first[2];
    let followers: Vec<i64> = before[3..6].iter().map(|r| r.thread_id).collect();

    // A reply to the cursor conversation, newer than everything.
    let reply = mail(
        "reply-1",
        "Re: Thread 3",
        &["root-3"],
        T0 + 9 * DAY,
        "and again",
    );
    store
        .ingest_raw(&blobs, account, Some(inbox), None, &reply)
        .unwrap();
    let after = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
    assert_eq!(
        after[0].thread_id, cursor.thread_id,
        "the reply moved it to the top"
    );

    let next = store
        .list_threads_after(&ListView::Inbox, 3, sort, cursor.date_ms, cursor.thread_id)
        .unwrap();
    assert_eq!(
        next.iter().map(|r| r.thread_id).collect::<Vec<_>>(),
        followers,
        "the next page is still the three conversations that followed the cursor"
    );
    assert!(
        !next.iter().any(|r| r.thread_id == cursor.thread_id),
        "the moved conversation is not served again"
    );
}

/// `mail`, from someone of one's own, for the sorts that read the sender.
fn mail_from(from: &str, msgid: &str, subject: &str, refs: &[&str], date_ms: i64) -> Vec<u8> {
    let raw = String::from_utf8(mail(msgid, subject, refs, date_ms, "hi")).unwrap();
    raw.replacen(
        "From: Someone <someone@example.com>",
        &format!("From: {from}"),
        1,
    )
    .into_bytes()
}

/// What a row was listed under, as the list sends it back with a cursor: the
/// walk's own value, carried on the row.
fn shown_for(
    _key: petrel_engine::store::SortKey,
    row: &petrel_engine::store::ThreadListing,
) -> String {
    row.sort_value
        .clone()
        .expect("a sender or subject row says what it sorts under")
}

/// Sorted by sender or subject, the conversation a page ended on can move
/// before the next page is asked for: a reply from someone else, or a "Re:"
/// subject, sorts it elsewhere. Looked up afresh, the next page started from
/// its new place, and the rows between were never listed or were listed
/// twice. Told what the row showed, the page follows from where it was.
#[test]
fn a_sender_or_subject_page_follows_where_the_cursor_was_listed() {
    use petrel_engine::store::{Sort, SortKey};
    for (key, ascending) in [
        (SortKey::Sender, true),
        (SortKey::Sender, false),
        (SortKey::Subject, true),
        (SortKey::Subject, false),
    ] {
        let case = format!("{key:?} ascending={ascending}");
        let (_d, mut store, blobs, account) = setup();
        let inbox = inboxed(&store, account);
        for i in 0..8 {
            let raw = mail_from(
                &format!("Sender {i} <s{i}@example.com>"),
                &format!("root-{i}"),
                &format!("Topic {i}"),
                &[],
                T0 + i * DAY,
            );
            store
                .ingest_raw(&blobs, account, Some(inbox), None, &raw)
                .unwrap();
        }
        let sort = Sort { key, ascending };
        let all = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
        let first = store.list_threads(&ListView::Inbox, 0, 3, sort).unwrap();
        let cursor = first[2].clone();
        let followers: Vec<i64> = all[3..6].iter().map(|r| r.thread_id).collect();
        let shown = shown_for(key, &cursor);

        // A reply that sorts the cursor conversation elsewhere: from someone
        // who sorts last, or under a "Re:" subject.
        let root = format!(
            "root-{}",
            cursor.subject.trim_start_matches("Topic ").trim()
        );
        let from = if ascending {
            "Zed Last <z@example.com>"
        } else {
            "Aaa First <a@example.com>"
        };
        let reply = mail_from(
            from,
            "reply-1",
            &format!("Re: {}", cursor.subject),
            &[root.as_str()],
            T0 + 20 * DAY,
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), None, &reply)
            .unwrap();
        let moved = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
        assert_ne!(
            moved.iter().position(|r| r.thread_id == cursor.thread_id),
            Some(2),
            "{case}: the reply moved it"
        );

        let next = store
            .list_threads_after_shown(
                &ListView::Inbox,
                3,
                sort,
                cursor.date_ms,
                cursor.thread_id,
                Some(&shown),
            )
            .unwrap();
        assert_eq!(
            next.iter().map(|r| r.thread_id).collect::<Vec<_>>(),
            followers,
            "{case}: the next page is the three that followed it"
        );
    }
}

/// Told what it showed, a cursor whose conversation has left the view (read
/// and archived on another device) still says where the page starts.
#[test]
fn a_sender_page_follows_a_cursor_that_left_the_view() {
    use petrel_engine::actions::{ActionKind, PlacementPolicy};
    use petrel_engine::store::{Sort, SortKey};
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    store.ensure_folder(account, "archive", "Archive").unwrap();
    for i in 0..6 {
        let raw = mail_from(
            &format!("Sender {i} <s{i}@example.com>"),
            &format!("root-{i}"),
            &format!("Topic {i}"),
            &[],
            T0 + i * DAY,
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), None, &raw)
            .unwrap();
    }
    let sort = Sort {
        key: SortKey::Sender,
        ascending: true,
    };
    let all = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
    let cursor = all[2].clone();
    store
        .apply_thread_action(
            account,
            cursor.thread_id,
            ActionKind::Archive,
            None,
            PlacementPolicy::Exclusive,
        )
        .unwrap();
    let next = store
        .list_threads_after_shown(
            &ListView::Inbox,
            3,
            sort,
            cursor.date_ms,
            cursor.thread_id,
            Some(&shown_for(SortKey::Sender, &cursor)),
        )
        .unwrap();
    assert_eq!(
        next.iter().map(|r| r.thread_id).collect::<Vec<_>>(),
        all[3..6].iter().map(|r| r.thread_id).collect::<Vec<_>>()
    );
}

/// Two conversations from one sender at one instant sort by key, so a page
/// can end between them and the next begins with the other, rather than
/// skipping it as level with the cursor.
#[test]
fn a_sender_page_ending_between_two_level_conversations_lists_both() {
    use petrel_engine::store::{Sort, SortKey};
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    for i in 0..4 {
        let raw = mail_from(
            "Same Sender <same@example.com>",
            &format!("root-{i}"),
            &format!("Topic {i}"),
            &[],
            T0,
        );
        store
            .ingest_raw(&blobs, account, Some(inbox), None, &raw)
            .unwrap();
    }
    for ascending in [true, false] {
        let sort = Sort {
            key: SortKey::Sender,
            ascending,
        };
        let all = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
        assert_eq!(all.len(), 4);
        // One row a page, from the first to the last.
        let mut walked = vec![all[0].thread_id];
        let mut last = all[0].clone();
        loop {
            let next = store
                .list_threads_after_shown(
                    &ListView::Inbox,
                    1,
                    sort,
                    last.date_ms,
                    last.thread_id,
                    Some(&shown_for(SortKey::Sender, &last)),
                )
                .unwrap();
            let Some(row) = next.into_iter().next() else {
                break;
            };
            walked.push(row.thread_id);
            last = row;
        }
        assert_eq!(
            walked,
            all.iter().map(|r| r.thread_id).collect::<Vec<_>>(),
            "ascending={ascending}: each once, in order"
        );
    }
}

/// Two messages of one conversation sent in the same second, by different
/// people: which of them the row names and which the walk sorts by were each
/// SQLite's choice, and could differ. The walk's own value, carried on the
/// row, is what the next page starts from, so a page that ends on that
/// conversation is followed by the rows after it, whatever the row shows.
#[test]
fn a_sender_page_follows_a_conversation_whose_newest_messages_tie() {
    use petrel_engine::store::{Sort, SortKey};
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let tied = T0 + 10 * DAY;
    store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_from("Bob <bob@example.com>", "tie-1", "Tie", &[], tied),
        )
        .unwrap();
    store
        .ingest_raw(
            &blobs,
            account,
            Some(inbox),
            None,
            &mail_from(
                "Carol <carol@example.com>",
                "tie-2",
                "Re: Tie",
                &["tie-1"],
                tied,
            ),
        )
        .unwrap();
    for (i, from) in ["Adam", "Bea", "Bob Two", "Cara", "Dan"].iter().enumerate() {
        store
            .ingest_raw(
                &blobs,
                account,
                Some(inbox),
                None,
                &mail_from(
                    &format!("{from} <p{i}@example.com>"),
                    &format!("other-{i}"),
                    &format!("Other {i}"),
                    &[],
                    T0 + i as i64 * DAY,
                ),
            )
            .unwrap();
    }
    let sort = Sort {
        key: SortKey::Sender,
        ascending: true,
    };
    let all = store.list_threads(&ListView::Inbox, 0, 50, sort).unwrap();
    assert_eq!(all.len(), 6);
    assert!(
        all.iter().all(|r| r.sort_value.is_some()),
        "every row says what it sorts under"
    );
    // Page by page, one row each, from the first row to the last.
    let mut walked = vec![all[0].thread_id];
    let mut last = all[0].clone();
    while let Some(row) = store
        .list_threads_after_shown(
            &ListView::Inbox,
            1,
            sort,
            last.date_ms,
            last.thread_id,
            Some(&shown_for(SortKey::Sender, &last)),
        )
        .unwrap()
        .into_iter()
        .next()
    {
        walked.push(row.thread_id);
        last = row;
    }
    assert_eq!(walked, all.iter().map(|r| r.thread_id).collect::<Vec<_>>());
    // By date there is nothing to carry.
    assert!(
        store
            .list_threads(&ListView::Inbox, 0, 50, Sort::default())
            .unwrap()
            .iter()
            .all(|r| r.sort_value.is_none())
    );
}

#[test]
fn thread_detail_page_returns_the_newest_slice_then_older() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let mut parent = String::from("p0@x");
    let mut ids = Vec::new();
    for i in 0..5 {
        let msgid = format!("p{i}@x");
        let refs: Vec<&str> = if i == 0 {
            vec![]
        } else {
            vec![parent.as_str()]
        };
        let raw = mail(&msgid, "Paged thread", &refs, T0 + i * DAY, "body");
        let ing = store
            .ingest_raw(&blobs, account, Some(inbox), None, &raw)
            .unwrap();
        ids.push(ing.message_id);
        parent = msgid;
    }
    let tid = store.thread_of(ids[0]).unwrap().unwrap();
    let newest = store.thread_detail_page(tid, Some(2), None).unwrap();
    assert_eq!(newest.len(), 2);
    assert_eq!(newest[0].id, ids[3]);
    assert_eq!(newest[1].id, ids[4]);
    let older = store
        .thread_detail_page(tid, Some(2), Some((newest[0].date_ms, newest[0].id)))
        .unwrap();
    assert_eq!(older.len(), 2);
    assert_eq!(older[0].id, ids[1]);
    assert_eq!(older[1].id, ids[2]);
    let rest = store
        .thread_detail_page(tid, Some(2), Some((older[0].date_ms, older[0].id)))
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].id, ids[0]);
}

#[test]
fn thread_index_lists_every_row_without_hydrating() {
    let (_d, mut store, blobs, account) = setup();
    let inbox = inboxed(&store, account);
    let mut parent = String::from("idx0@x");
    let mut ids = Vec::new();
    for i in 0..5 {
        let msgid = format!("idx{i}@x");
        let refs: Vec<&str> = if i == 0 {
            vec![]
        } else {
            vec![parent.as_str()]
        };
        let raw = mail(&msgid, "Indexed thread", &refs, T0 + i * DAY, "body");
        let ing = store
            .ingest_raw(&blobs, account, Some(inbox), None, &raw)
            .unwrap();
        ids.push(ing.message_id);
        parent = msgid;
    }
    let tid = store.thread_of(ids[0]).unwrap().unwrap();
    let index = store.thread_index(tid).unwrap();
    assert_eq!(
        index.iter().map(|r| r.id).collect::<Vec<_>>(),
        ids,
        "index is every surviving message, oldest first"
    );
    assert!(
        index
            .iter()
            .all(|r| !r.from_display.is_empty() || !r.from_addr.is_empty()),
        "each card has a sender"
    );
    assert!(
        store.thread_message(ids[2]).unwrap().is_some(),
        "opening one card hydrates that id"
    );
    let missing = ids[4] + 1_000_000;
    assert!(
        store.thread_message(missing).unwrap().is_none(),
        "an id that is not in the store is not an error"
    );
    let fat = store.thread_detail(tid).unwrap();
    assert_eq!(fat.len(), 5);
    assert_eq!(fat[0].to, ["me@example.com"]);
}
