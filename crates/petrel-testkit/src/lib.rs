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
