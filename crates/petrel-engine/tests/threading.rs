//! Conversation threading against the store.
//!
//! The interesting cases are not "a reply joins its parent" but the ones that
//! make threading wrong in real mailboxes: messages arriving out of order,
//! references stripped by mailing lists, generic subjects that must *not*
//! merge, and accounts that must never bleed into each other.

use petrel_engine::blob::BlobStore;
use petrel_engine::store::Store;

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
                .ingest_raw(&blobs, account, None, None, m)
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
    let listed = store.list_threads(0, 20).expect("list");
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
            None,
            None,
            &mail("m1@x", "Hi", &[], T0, "start"),
        )
        .expect("m1");
    // m3 references m2, which we have not seen yet — so it starts its own thread.
    let third = store
        .ingest_raw(
            &blobs,
            account,
            None,
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
            None,
            None,
            &mail("m2@x", "Re: Hi", &["m1@x"], T0 + DAY, "second"),
        )
        .expect("m2");

    let t1 = store.thread_of(first.message_id).unwrap().unwrap();
    let t2 = store.thread_of(second.message_id).unwrap().unwrap();
    let t3 = store.thread_of(third.message_id).unwrap().unwrap();
    assert_eq!(t1, t2, "middle joins the head");
    assert_eq!(t2, t3, "and drags the tail with it");

    let listed = store.list_threads(0, 20).expect("list");
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
            None,
            None,
            &mail("s1@x", "Quarterly logistics review", &[], T0, "one"),
        )
        .expect("a");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            None,
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

/// The failure mode that matters more than a missed merge: a *wrong* merge
/// hides mail inside an unrelated conversation, where nobody looks for it.
#[test]
fn generic_subjects_do_not_merge_strangers() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &mail("g1@x", "Hi", &[], T0, "from alice"),
        )
        .expect("a");
    let b = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &mail("g2@x", "Hi", &[], T0 + DAY, "from bob"),
        )
        .expect("b");

    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "\"Hi\" is not evidence of a conversation"
    );
    assert_eq!(store.list_threads(0, 20).expect("list").len(), 2);
}

#[test]
fn the_subject_window_stops_distant_mail_from_joining() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &mail("w1@x", "Annual budget planning", &[], T0, "this year"),
        )
        .expect("a");
    // Same subject, a year later — almost certainly a new conversation.
    let b = store
        .ingest_raw(
            &blobs,
            account,
            None,
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
        .ingest_raw(&blobs, account, None, None, &raw)
        .expect("a");
    let b = store
        .ingest_raw(&blobs, other, None, None, &raw)
        .expect("b");

    assert_ne!(
        store.thread_of(a.message_id).unwrap(),
        store.thread_of(b.message_id).unwrap(),
        "each account keeps its own conversation; actions must not cross"
    );
    assert_eq!(store.list_threads(0, 20).expect("list").len(), 2);
}

#[test]
fn deleted_messages_leave_the_conversation() {
    let (_d, mut store, blobs, account) = setup();
    let a = store
        .ingest_raw(
            &blobs,
            account,
            None,
            None,
            &mail("d1@x", "Vendor contract terms", &[], T0, "one"),
        )
        .expect("a");
    store
        .ingest_raw(
            &blobs,
            account,
            None,
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
    let listed = store.list_threads(0, 20).expect("list");
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
                .ingest_raw(&blobs, account, None, None, m)
                .expect("ingest");
        }
    }
    let listed = store.list_threads(0, 20).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].message_count, 2,
        "three syncs, still two messages"
    );
}
