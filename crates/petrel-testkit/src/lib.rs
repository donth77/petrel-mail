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
    ("Q3 vendor contracts — pricing before Friday",
     "The twelve-month term works. The volume tier resets annually, not quarterly, so I've added that to page 3."),
    ("Vendor shortlist",
     "Narrowed it to three. Two came in under budget; the third is better on support but 18% more."),
    ("Self-hosted Dovecot — cert pinning question",
     "The fingerprint changed after their renewal. Is that flow documented anywhere I can point the team at?"),
    ("Receipt for August",
     "Your subscription renewed. No action needed — the invoice is attached for your records."),
    ("Issue 214 — what the rate cut means",
     "Three things worth your attention this week, and one that isn't but is funny anyway."),
    ("Your order has shipped",
     "Tracking is below. Expected Thursday, and no signature is required on delivery."),
    ("[petrel-mail] Sanitizer profile ready for review (#142)",
     "17 tests including CSS exfiltration and attribute-selector attacks. Two edge cases still open."),
    ("Notes from Tuesday",
     "Rough notes while they're fresh. The bit about retention needs a decision before we go further."),
    ("東京支社の会議について",
     "明日の午前中に詳細を確認してください。資料は添付のとおりです。"),
    ("Annex review",
     "Marked up section 4 in track changes. Everything else reads fine to me."),
    ("Invoice 2214 — overdue",
     "This one slipped past its terms. Could you confirm it's in the run for this week?"),
    ("Lunch Thursday?",
     "There's a new place near the office that isn't terrible. 12:30 if that works."),
    ("Board pack v4",
     "Final version, incorporating Friday's comments. Slides 8-11 changed the most."),
    ("Re: Contract terms",
     "Agreed on all but the indemnity cap. Can we talk it through before signing?"),
];

impl DemoMailbox {
    /// `now_ms` anchors the newest message; the rest spread backwards.
    pub fn new(seed: u64, n: usize, now_ms: i64) -> Self {
        Self { rng: Rng::new(seed), n, produced: 0, now_ms }
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
            to_addr: "me@example.com".to_string(),
            subject,
            body: body.to_string(),
        })
    }
}
