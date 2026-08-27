//! Deterministic synthetic mail for tests and benchmarks. No real addresses,
//! no real content, ever — see AGENTS.md. Seeded, so every run reproduces.

/// xorshift64* — tiny, deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
}

const FIRST: &[&str] = &[
    "avery", "blake", "casey", "devon", "ellis", "finley", "gale", "harper", "indra", "jules",
    "kai", "lark", "morgan", "noor", "oakes", "petra", "quinn", "rowan", "sage", "tatum",
];
const DOMAIN: &[&str] = &[
    "example.com",
    "example.org",
    "mail.example.net",
    "corp.example.io",
    "lists.example.dev",
];
const SUBJECT_WORDS: &[&str] = &[
    "meeting",
    "notes",
    "invoice",
    "project",
    "falcon",
    "launch",
    "review",
    "draft",
    "budget",
    "report",
    "schedule",
    "update",
    "planning",
    "contract",
    "release",
    "roadmap",
    "summary",
    "question",
    "reminder",
    "agenda",
    "quarterly",
    "status",
    "feedback",
    "proposal",
];
const BODY_WORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "have",
    "will",
    "team",
    "please",
    "thanks",
    "attached",
    "details",
    "week",
    "meeting",
    "notes",
    "before",
    "after",
    "review",
    "should",
    "could",
    "deadline",
    "morning",
    "afternoon",
    "confirm",
    "numbers",
    "final",
    "draft",
    "shipping",
    "vendor",
    "pricing",
    "context",
    "thread",
    "reply",
    "forward",
    "calendar",
    "invite",
    "document",
    "storage",
    "search",
    "message",
    "archive",
    "folder",
    "project",
    "falcon",
    "status",
    "update",
    "quarterly",
    "report",
    "release",
    "candidate",
];
const CJK_SENTENCES: &[&str] = &[
    "東京計画の会議は木曜日です",
    "予算報告書を添付します",
    "検索機能の設計を確認してください",
    "来週の予定を教えてください",
];

pub struct GenMessage {
    pub from_addr: String,
    pub from_display: String,
    pub to_addr: String,
    pub subject: String,
    pub body: String,
    pub date_ms: i64,
}

impl GenMessage {
    /// The message as bytes, the way it would have arrived off the wire.
    ///
    /// Synthetic mail is inserted in bulk for speed, which stores headers and
    /// an index but no body — and the reading pane renders from stored bytes,
    /// so every message opens blank. This is what gives it something to read.
    ///
    /// `seq` only has to be unique within a mailbox: it is the Message-ID, and
    /// the store dedupes on that, so a repeated one silently drops the message.
    pub fn to_rfc822(&self, seq: usize) -> Vec<u8> {
        let mut out = String::with_capacity(self.body.len() + 512);
        out.push_str(&format!(
            "From: {} <{}>\r\n",
            self.from_display, self.from_addr
        ));
        out.push_str(&format!("To: {}\r\n", self.to_addr));
        out.push_str(&format!("Subject: {}\r\n", self.subject));
        out.push_str(&format!("Date: {}\r\n", rfc2822_date(self.date_ms)));
        out.push_str(&format!("Message-ID: <demo-{seq}@petrel.example>\r\n"));
        out.push_str("MIME-Version: 1.0\r\n");
        out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        out.push_str("Content-Transfer-Encoding: 8bit\r\n\r\n");

        // The generated line, then enough around it that the reading pane has
        // the shape of a message rather than one orphaned sentence.
        out.push_str(&self.body);
        out.push_str("\r\n\r\n");
        out.push_str(FOLLOW_ONS[seq % FOLLOW_ONS.len()]);
        out.push_str("\r\n\r\n");
        // People sign their mail; billing systems and newsletters do not, and
        // "Thanks, Fastmail" under an invoice is the kind of detail that makes
        // synthetic mail read as synthetic.
        if let Some(name) = self.signs_off_as() {
            out.push_str(CLOSINGS[seq % CLOSINGS.len()]);
            out.push_str("\r\n");
            out.push_str(name);
            out.push_str("\r\n");
        }
        out.into_bytes()
    }
}

impl GenMessage {
    /// The sender's first name, when the sender is a person.
    ///
    /// Decided from the address rather than the display name: a person's
    /// mailbox is usually named after them (`dana@`, `sam.ortiz@`), and an
    /// organisation's is named after its function (`billing@`, `news@`,
    /// `orders@`). No list of company names to keep up to date.
    fn signs_off_as(&self) -> Option<&str> {
        let first = self.from_display.split(' ').next()?;
        if first.len() < 2 {
            return None;
        }
        let local = self.from_addr.split('@').next()?.to_lowercase();
        local.contains(&first.to_lowercase()).then_some(first)
    }
}

/// Second paragraphs. Deliberately dull and specific: demo prose that tries to
/// be interesting reads as filler, and filler is what you notice in a
/// screenshot.
const FOLLOW_ONS: &[&str] = &[
    "I have put the numbers in the shared folder so you can check the working rather than take my word for it. The summary is on the first tab; everything else is the trail that produced it.",
    "No rush on this — end of week is fine, and if it slips to Monday nothing breaks. I would rather it were right than early.",
    "Two of us have read it and landed in the same place, which is either reassuring or a sign we have both missed the same thing. A third pair of eyes would settle it.",
    "The short version: it holds. The longer version is below, for whoever wants to see how we got there.",
    "I have left the old version in place for now so the two can be compared side by side. Say the word and I will take it down.",
];

/// Sign-offs, so a message ends the way people end them.
const CLOSINGS: &[&str] = &["Thanks,", "Best,", "Cheers,", "Regards,", "Talk soon,"];

/// RFC 2822 date, in UTC.
///
/// Civil-from-days (Howard Hinnant's algorithm): no date crate, no timezone
/// database, and the same arithmetic the store uses for mbox From lines.
pub fn rfc2822_date(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    let dow = (days + 4).rem_euclid(7) as usize;

    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        DAYS[dow],
        d,
        MONTHS[(month - 1) as usize],
        year,
        h,
        m,
        s
    )
}

pub struct MailboxGen {
    rng: Rng,
    n: usize,
    produced: usize,
    /// Every k-th message gets a unique rare token in its body (search-recall probes).
    pub rare_token_every: usize,
    /// Percent of messages carrying a CJK sentence.
    pub cjk_percent: u64,
}

impl MailboxGen {
    pub fn new(seed: u64, n: usize) -> Self {
        MailboxGen {
            rng: Rng::new(seed),
            n,
            produced: 0,
            rare_token_every: 1000,
            cjk_percent: 10,
        }
    }

    pub fn rare_token(index: usize) -> String {
        format!("zephyrite{index}")
    }

    fn words(&mut self, pool: &[&str], count: usize) -> String {
        let mut out = String::new();
        for i in 0..count {
            if i > 0 {
                out.push(' ');
            }
            // Mild bias toward the front of the pool: common words stay common.
            let idx = self.rng.below(pool.len()).min(self.rng.below(pool.len()));
            out.push_str(pool[idx]);
        }
        out
    }
}

impl Iterator for MailboxGen {
    type Item = GenMessage;

    fn next(&mut self) -> Option<GenMessage> {
        if self.produced >= self.n {
            return None;
        }
        let i = self.produced;
        self.produced += 1;

        let from_first = FIRST[self.rng.below(FIRST.len())];
        let from_last = FIRST[self.rng.below(FIRST.len())];
        let from_addr = format!(
            "{}.{}@{}",
            from_first,
            from_last,
            DOMAIN[self.rng.below(DOMAIN.len())]
        );
        let from_display = format!("{} {}", from_first, from_last);
        let to_addr = format!("{}@{}", FIRST[self.rng.below(FIRST.len())], DOMAIN[0]);

        let subject_len = 3 + self.rng.below(4);
        let subject = self.words(SUBJECT_WORDS, subject_len);
        let body_len = 60 + self.rng.below(190);
        let mut body = self.words(BODY_WORDS, body_len);
        if self.rare_token_every > 0 && i.is_multiple_of(self.rare_token_every) {
            body.push(' ');
            body.push_str(&Self::rare_token(i));
        }
        if self.rng.chance(self.cjk_percent) {
            body.push(' ');
            body.push_str(CJK_SENTENCES[self.rng.below(CJK_SENTENCES.len())]);
        }

        let date_ms = 1_600_000_000_000_i64 + (i as i64) * 60_000 + (self.rng.below(50_000) as i64);
        Some(GenMessage {
            from_addr,
            from_display,
            to_addr,
            subject,
            body,
            date_ms,
        })
    }
}

// ---------------------------------------------------------------- demo mail

/// Plausible-looking mail for looking at the app.
///
/// Distinct from [`MailboxGen`], which generates deliberate word-salad because
/// search-recall benchmarks need rare tokens and uniform distribution. This one
/// optimises for the opposite thing: a list that reads like somebody's actual
/// inbox, so design problems are visible instead of hidden behind noise.
pub struct DemoMailbox {
    rng: Rng,
    n: usize,
    produced: usize,
    now_ms: i64,
}

const DEMO_PEOPLE: &[(&str, &str)] = &[
    ("Sam Ortiz", "sam.ortiz@vendorco.example"),
    ("Dana Wu", "dana@northbay.example"),
    ("Priya Raman", "priya.raman@clientco.example"),
    ("Marcus Bell", "m.bell@northbay.example"),
    ("Yuki Tanaka", "y.tanaka@example.jp"),
    ("Ana Sousa", "ana@sousa-design.example"),
    ("Tom Fenwick", "tom.fenwick@lawpartners.example"),
    ("Rachel Kim", "rachel@kimaccounting.example"),
    ("The Weekly Ledger", "news@ledger.example"),
    ("Depot Supply", "orders@depot.example"),
    ("GitHub", "notifications@github.example"),
    ("Fastmail Billing", "billing@fastmail.example"),
    ("会議事務局", "kaigi@example.jp"),
];

const DEMO_THREADS: &[(&str, &str)] = &[
    (
        "Q3 vendor contracts — pricing before Friday",
        "The twelve-month term works. The volume tier resets annually, not quarterly, so I've added that to page 3.",
    ),
    (
        "Vendor shortlist",
        "Narrowed it to three. Two came in under budget; the third is better on support but 18% more.",
    ),
    (
        "Self-hosted Dovecot — cert pinning question",
        "The fingerprint changed after their renewal. Is that flow documented anywhere I can point the team at?",
    ),
    (
        "Receipt for August",
        "Your subscription renewed. No action needed — the invoice is attached for your records.",
    ),
    (
        "Issue 214 — what the rate cut means",
        "Three things worth your attention this week, and one that isn't but is funny anyway.",
    ),
    (
        "Your order has shipped",
        "Tracking is below. Expected Thursday, and no signature is required on delivery.",
    ),
    (
        "[petrel-mail] Sanitizer profile ready for review (#142)",
        "17 tests including CSS exfiltration and attribute-selector attacks. Two edge cases still open.",
    ),
    (
        "Notes from Tuesday",
        "Rough notes while they're fresh. The bit about retention needs a decision before we go further.",
    ),
    (
        "東京支社の会議について",
        "明日の午前中に詳細を確認してください。資料は添付のとおりです。",
    ),
    (
        "Annex review",
        "Marked up section 4 in track changes. Everything else reads fine to me.",
    ),
    (
        "Invoice 2214 — overdue",
        "This one slipped past its terms. Could you confirm it's in the run for this week?",
    ),
    (
        "Lunch Thursday?",
        "There's a new place near the office that isn't terrible. 12:30 if that works.",
    ),
    (
        "Board pack v4",
        "Final version, incorporating Friday's comments. Slides 8-11 changed the most.",
    ),
    (
        "Re: Contract terms",
        "Agreed on all but the indemnity cap. Can we talk it through before signing?",
    ),
];

impl DemoMailbox {
    /// `now_ms` anchors the newest message; the rest spread backwards.
    pub fn new(seed: u64, n: usize, now_ms: i64) -> Self {
        Self {
            rng: Rng::new(seed),
            n,
            produced: 0,
            now_ms,
        }
    }
}

impl Iterator for DemoMailbox {
    type Item = GenMessage;

    fn next(&mut self) -> Option<GenMessage> {
        if self.produced >= self.n {
            return None;
        }
        let i = self.produced;
        self.produced += 1;

        let (display, addr) = DEMO_PEOPLE[self.rng.below(DEMO_PEOPLE.len())];
        let (subject, body) = DEMO_THREADS[self.rng.below(DEMO_THREADS.len())];

        // Roughly a third are replies, so the list shows conversations rather
        // than a flat wall of first-contact mail.
        let is_reply = self.rng.chance(33);
        let subject = if is_reply && !subject.starts_with("Re:") {
            format!("Re: {subject}")
        } else {
            subject.to_string()
        };

        // Newest first, thinning out into the past: dense over recent days,
        // sparser further back, the way a real mailbox looks.
        let minutes_back = (i as i64) * (7 + self.rng.below(120) as i64);
        Some(GenMessage {
            date_ms: self.now_ms - minutes_back * 60_000,
            from_addr: addr.to_string(),
            from_display: display.to_string(),
            to_addr: "you@example.com".to_string(),
            subject,
            body: body.to_string(),
        })
    }
}
