//! A scripted IMAP server that keeps state, for the sync tests: mailboxes
//! hold their messages, flags and UIDVALIDITY, and change as the client asks
//! (MOVE, COPY, STORE, EXPUNGE), so a test can see which message a command
//! reached. A mailbox can be renumbered under a new UIDVALIDITY, which is
//! what a change queued while the account could not reach the server has to
//! survive (fifth audit #168). `scripted_imap` counts sign-ins and answers
//! little else; the drain's own script knows one UIDVALIDITY.
//!
//! Plaintext, no literals, no IDLE: what the drain, the sync cycle, recovery
//! and the backfill say. Ported in part from the fifth audit's probe server
//! (docs/26-probes/p2-shell/src/p2/srv.rs).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub(crate) struct Msg {
    pub(crate) uid: u32,
    pub(crate) raw: Vec<u8>,
    pub(crate) flags: Vec<String>,
    date: String,
}

struct MBox {
    attrs: &'static str,
    msgs: Vec<Msg>,
    next: u32,
    validity: u32,
}

pub(crate) struct Imap {
    caps: &'static str,
    boxes: Mutex<BTreeMap<String, MBox>>,
    /// Every command line, in order.
    lines: Mutex<Vec<String>>,
    /// The next command containing the first part is answered with the
    /// second's bytes instead: a reply the client's parser cannot read.
    garble: Mutex<Option<(String, Vec<u8>)>>,
    arrivals: Mutex<u32>,
    /// A Message-ID search omits the mailbox's highest UID. APPENDUID still
    /// names it. A draft push that learned its UID from a later search used
    /// to miss the copy it had just appended and leave the previous one.
    pub(crate) hide_newest_message_id: AtomicBool,
    /// The next APPEND lands twice, the second copy at the next UID: another
    /// push of the same draft, finishing just after this one.
    pub(crate) twin_next_append: AtomicBool,
    /// APPENDUID names a UID that is never stored. The bytes are kept at the
    /// following UID: a server that accepts the append and then files a
    /// rewritten copy under a new number.
    pub(crate) rewrite_appended: AtomicBool,
    /// A UID FETCH that names no stored message is answered NO. RFC 3501 has
    /// it answer OK with nothing; a server that does not made a draft push
    /// that checked APPENDUID fail after its APPEND had already landed.
    pub(crate) refuse_missing_fetch: AtomicBool,
}

impl Imap {
    /// A server advertising `caps`, holding the mailboxes `(name, attributes)`,
    /// empty, each at UIDVALIDITY 1. Any password is taken.
    pub(crate) fn new(caps: &'static str, boxes: &[(&str, &'static str)]) -> Arc<Imap> {
        Arc::new(Imap {
            caps,
            boxes: Mutex::new(
                boxes
                    .iter()
                    .map(|(name, attrs)| {
                        let mailbox = MBox {
                            attrs,
                            msgs: Vec::new(),
                            next: 1,
                            validity: 1,
                        };
                        (name.to_string(), mailbox)
                    })
                    .collect(),
            ),
            lines: Mutex::new(Vec::new()),
            garble: Mutex::new(None),
            arrivals: Mutex::new(0),
            hide_newest_message_id: AtomicBool::new(false),
            twin_next_append: AtomicBool::new(false),
            rewrite_appended: AtomicBool::new(false),
            refuse_missing_fetch: AtomicBool::new(false),
        })
    }

    /// A message arriving in `mailbox`, at the next UID.
    pub(crate) fn put(&self, mailbox: &str, raw: Vec<u8>, flags: &[&str]) -> u32 {
        let date = {
            let mut n = self.arrivals.lock().unwrap();
            *n += 1;
            format!("01-Oct-2026 09:{:02}:{:02} +0000", (*n / 60) % 60, *n % 60)
        };
        let mut boxes = self.boxes.lock().unwrap();
        let b = boxes.get_mut(mailbox).expect("mailbox");
        let uid = b.next;
        b.next += 1;
        b.msgs.push(Msg {
            uid,
            raw,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            date,
        });
        uid
    }

    /// The server renumbers a mailbox, as a migration or a rebuilt index
    /// does: a new UIDVALIDITY, and UIDs from 1 again in the order held.
    pub(crate) fn renumber(&self, mailbox: &str, validity: u32) {
        let mut boxes = self.boxes.lock().unwrap();
        let b = boxes.get_mut(mailbox).unwrap();
        b.validity = validity;
        for (i, m) in b.msgs.iter_mut().enumerate() {
            m.uid = i as u32 + 1;
        }
        b.next = b.msgs.len() as u32 + 1;
    }

    /// Another client expunged the message with this subject.
    pub(crate) fn expunge_where(&self, mailbox: &str, subject: &str) {
        let mut boxes = self.boxes.lock().unwrap();
        let b = boxes.get_mut(mailbox).unwrap();
        b.msgs
            .retain(|m| header_value(&m.raw, "Subject").as_deref() != Some(subject));
    }

    /// The subjects a mailbox holds, by UID.
    pub(crate) fn subjects(&self, mailbox: &str) -> Vec<(u32, String)> {
        self.boxes.lock().unwrap()[mailbox]
            .msgs
            .iter()
            .map(|m| (m.uid, header_value(&m.raw, "Subject").unwrap_or_default()))
            .collect()
    }

    pub(crate) fn held(&self, mailbox: &str) -> Vec<Msg> {
        self.boxes.lock().unwrap()[mailbox].msgs.clone()
    }

    /// The commands received that contain `what`, without their tags.
    pub(crate) fn said(&self, what: &str) -> Vec<String> {
        let up = what.to_ascii_uppercase();
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| l.split_once(' ').map(|(_, command)| command.to_string()))
            .filter(|command| command.to_ascii_uppercase().contains(&up))
            .collect()
    }

    /// Answers the next command containing `command` with `bytes`.
    pub(crate) fn garble_next(&self, command: &str, bytes: &[u8]) {
        *self.garble.lock().unwrap() = Some((command.to_ascii_uppercase(), bytes.to_vec()));
    }
}

/// The tokens of an argument list: atoms, quoted strings and parenthesised
/// lists.
enum Tok {
    Atom(String),
    Str(String),
    List(String),
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Atom(s) | Tok::Str(s) | Tok::List(s) => s.clone(),
        }
    }
}

fn tokens(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        match b[i] {
            b' ' => i += 1,
            b'"' => {
                let mut v = Vec::new();
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                    }
                    v.push(b[i]);
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(String::from_utf8_lossy(&v).into_owned()));
            }
            b'(' => {
                let start = i;
                let mut depth = 0;
                while i < b.len() {
                    if b[i] == b'(' {
                        depth += 1;
                    } else if b[i] == b')' {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    i += 1;
                }
                out.push(Tok::List(s[start..i].to_string()));
            }
            _ => {
                let start = i;
                let mut depth = 0;
                while i < b.len() && (b[i] != b' ' || depth > 0) {
                    if b[i] == b'[' {
                        depth += 1;
                    } else if b[i] == b']' {
                        depth -= 1;
                    }
                    i += 1;
                }
                out.push(Tok::Atom(s[start..i].to_string()));
            }
        }
    }
    out
}

/// Whether `n` is in a sequence set, `*` standing for `max`.
fn in_set(spec: &str, n: u32, max: u32) -> bool {
    spec.split(',').any(|part| {
        let val = |x: &str| {
            if x == "*" {
                max
            } else {
                x.parse::<u32>().unwrap_or(0)
            }
        };
        match part.split_once(':') {
            Some((a, b)) => {
                let (a, b) = (val(a), val(b));
                n >= a.min(b) && n <= a.max(b)
            }
            None => n == val(part),
        }
    })
}

fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let want = format!("{}:", name.to_ascii_lowercase());
    head.split("\r\n")
        .find(|line| line.to_ascii_lowercase().starts_with(&want))
        .map(|line| line[want.len()..].trim().to_string())
}

/// One header field as `BODY[HEADER.FIELDS (...)]` returns it.
fn header_block(raw: &[u8], name: &str) -> Vec<u8> {
    match header_value(raw, name) {
        Some(value) => format!("{name}: {value}\r\n\r\n").into_bytes(),
        None => b"\r\n".to_vec(),
    }
}

/// A refusal for a mailbox the server does not have, in Dovecot's words,
/// which quote the name as Gmail's do: what sync.log must never repeat.
fn missing(name: &str) -> String {
    format!("NO [NONEXISTENT] Mailbox doesn't exist: {name} (0.001 + 0.000 secs).")
}

/// `{n}` at the end of a command: the next `n` bytes are a literal.
fn literal_len(line: &str) -> Option<usize> {
    let start = line.rfind('{')?;
    let end = line.rfind('}')?;
    if end <= start || !line[end + 1..].trim().is_empty() {
        return None;
    }
    line[start + 1..end].parse().ok()
}

/// Stores an APPEND literal and names the UID it was given.
fn append_literal(srv: &Imap, line: &str, raw: &[u8]) -> (Vec<u8>, bool) {
    let tag = line.split_whitespace().next().unwrap_or("*");
    let name = mailbox_of_append(line);
    let mut boxes = srv.boxes.lock().unwrap();
    let Some(name) = name else {
        return (format!("{tag} BAD no mailbox\r\n").into_bytes(), false);
    };
    let Some(b) = boxes.get_mut(&name) else {
        return (
            format!("{tag} NO [NONEXISTENT] Unknown Mailbox: {name}\r\n").into_bytes(),
            false,
        );
    };
    let uid = b.next;
    b.next += 1;
    let validity = b.validity;
    let flags = line
        .split_once('(')
        .and_then(|(head, rest)| rest.split_once(')').map(|pair| (head, pair.0)))
        .filter(|(head, _)| head.contains("APPEND"))
        .map(|(_, inside)| {
            inside
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if srv.rewrite_appended.load(Ordering::Relaxed) {
        // The number APPENDUID will name. Nothing is stored under it.
        let real = b.next;
        b.next += 1;
        b.msgs.push(Msg {
            uid: real,
            raw: raw.to_vec(),
            flags,
            date: "01-Oct-2026 09:00:00 +0000".into(),
        });
    } else if srv.twin_next_append.swap(false, Ordering::Relaxed) {
        b.msgs.push(Msg {
            uid,
            raw: raw.to_vec(),
            flags: flags.clone(),
            date: "01-Oct-2026 09:00:00 +0000".into(),
        });
        let twin = b.next;
        b.next += 1;
        b.msgs.push(Msg {
            uid: twin,
            raw: raw.to_vec(),
            flags,
            date: "01-Oct-2026 09:00:01 +0000".into(),
        });
    } else {
        b.msgs.push(Msg {
            uid,
            raw: raw.to_vec(),
            flags,
            date: "01-Oct-2026 09:00:00 +0000".into(),
        });
    }
    // APPENDUID is UIDPLUS's (RFC 4315); a server without it names nothing.
    let reply = if srv.caps.contains("UIDPLUS") {
        format!("{tag} OK [APPENDUID {validity} {uid}] Appended\r\n")
    } else {
        format!("{tag} OK Appended\r\n")
    };
    (reply.into_bytes(), false)
}

fn mailbox_of_append(line: &str) -> Option<String> {
    let toks = tokens(line);
    let at = toks
        .iter()
        .position(|tok| tok.text().eq_ignore_ascii_case("APPEND"))?;
    Some(toks.get(at + 1)?.text())
}

/// The reply to one command line, and whether to close afterwards.
fn answer(srv: &Imap, selected: &mut Option<String>, line: &str) -> (Vec<u8>, bool) {
    let tag = line.split_whitespace().next().unwrap_or("*").to_string();
    let rest = line[tag.len()..].trim_start();
    let (uid, body) = match rest.get(..4) {
        Some(prefix) if prefix.eq_ignore_ascii_case("UID ") => (true, &rest[4..]),
        _ => (false, rest),
    };
    let cmd = body
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    let args = body[cmd.len().min(body.len())..].trim_start();
    let ok = |s: &str| format!("{tag} {s}\r\n").into_bytes();
    let mut boxes = srv.boxes.lock().unwrap();
    match cmd.as_str() {
        "CAPABILITY" => (
            format!("* CAPABILITY IMAP4rev1 {}\r\n{tag} OK done\r\n", srv.caps).into_bytes(),
            false,
        ),
        "LOGIN" | "NOOP" | "CHECK" | "ENABLE" => (ok("OK done"), false),
        "ID" => (format!("* ID NIL\r\n{tag} OK done\r\n").into_bytes(), false),
        "LOGOUT" => (format!("* BYE bye\r\n{tag} OK bye\r\n").into_bytes(), true),
        "CLOSE" | "UNSELECT" => {
            *selected = None;
            (ok("OK done"), false)
        }
        "LIST" | "LSUB" => {
            let mut out = String::new();
            for (name, b) in boxes.iter() {
                let attrs = format!("\\HasNoChildren {}", b.attrs);
                out.push_str(&format!("* {cmd} ({}) \"/\" \"{name}\"\r\n", attrs.trim()));
            }
            out.push_str(&format!("{tag} OK done\r\n"));
            (out.into_bytes(), false)
        }
        "SELECT" | "EXAMINE" => {
            let name = tokens(args).first().map(Tok::text).unwrap_or_default();
            match boxes.get(&name) {
                Some(b) => {
                    *selected = Some(name);
                    let out = format!(
                        "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] ok\r\n* OK [UIDNEXT {}] ok\r\n{tag} OK done\r\n",
                        b.msgs.len(),
                        b.validity,
                        b.next
                    );
                    (out.into_bytes(), false)
                }
                None => (ok(&missing(&name)), false),
            }
        }
        "CREATE" => {
            let name = tokens(args).first().map(Tok::text).unwrap_or_default();
            if boxes.contains_key(&name) {
                return (ok("NO [ALREADYEXISTS] Mailbox exists"), false);
            }
            let mailbox = MBox {
                attrs: "",
                msgs: Vec::new(),
                next: 1,
                validity: 1,
            };
            boxes.insert(name, mailbox);
            (ok("OK created"), false)
        }
        "SUBSCRIBE" => (ok("OK done"), false),
        "STATUS" => {
            let name = tokens(args).first().map(Tok::text).unwrap_or_default();
            match boxes.get(&name) {
                Some(b) => (
                    format!(
                        "* STATUS \"{name}\" (MESSAGES {} UIDNEXT {} UIDVALIDITY {})\r\n{tag} OK done\r\n",
                        b.msgs.len(),
                        b.next,
                        b.validity
                    )
                    .into_bytes(),
                    false,
                ),
                None => (ok(&missing(&name)), false),
            }
        }
        _ => {
            let Some(sel) = selected.clone() else {
                return (ok("BAD no mailbox selected"), false);
            };
            let Some(b) = boxes.get_mut(&sel) else {
                return (ok(&missing(&sel)), false);
            };
            let max_uid = b.next.saturating_sub(1);
            let max_seq = b.msgs.len() as u32;
            let hit = |set: &str, seq: u32, m: &Msg| {
                if uid {
                    in_set(set, m.uid, max_uid)
                } else {
                    in_set(set, seq, max_seq)
                }
            };
            match cmd.as_str() {
                "SEARCH" => {
                    let crit = args.to_ascii_uppercase();
                    let term = tokens(args)
                        .last()
                        .map(Tok::text)
                        .unwrap_or_default()
                        .to_lowercase();
                    let mut found = Vec::new();
                    for (i, m) in b.msgs.iter().enumerate() {
                        if crit.contains("UNDELETED") && m.flags.iter().any(|f| f == "\\Deleted") {
                            continue;
                        }
                        let pick = if crit.contains("HEADER MESSAGE-ID") {
                            let id = header_value(&m.raw, "Message-ID").unwrap_or_default();
                            !term.is_empty() && id.to_lowercase().contains(&term)
                        } else if let Some(range) = crit.strip_prefix("UID ") {
                            in_set(range.trim(), m.uid, max_uid)
                        } else {
                            true
                        };
                        if pick {
                            found.push(if uid { m.uid } else { i as u32 + 1 }.to_string());
                        }
                    }
                    if crit.contains("HEADER MESSAGE-ID")
                        && srv.hide_newest_message_id.load(Ordering::Relaxed)
                        && let Some((idx, _)) = found
                            .iter()
                            .enumerate()
                            .max_by_key(|(_, n)| n.parse::<u32>().unwrap_or(0))
                    {
                        found.remove(idx);
                    }
                    let sp = if found.is_empty() { "" } else { " " };
                    let out = format!("* SEARCH{sp}{}\r\n{tag} OK searched\r\n", found.join(" "));
                    (out.into_bytes(), false)
                }
                "FETCH" => {
                    let (set, items) = args.split_once(' ').unwrap_or((args, ""));
                    let items = items.to_ascii_uppercase();
                    let mut out = Vec::new();
                    for (i, m) in b.msgs.iter().enumerate() {
                        let seq = i as u32 + 1;
                        if !hit(set, seq, m) {
                            continue;
                        }
                        let mut item = format!("* {seq} FETCH (UID {}", m.uid).into_bytes();
                        if items.contains("FLAGS") {
                            item.extend(format!(" FLAGS ({})", m.flags.join(" ")).bytes());
                        }
                        if items.contains("RFC822.SIZE") {
                            item.extend(format!(" RFC822.SIZE {}", m.raw.len()).bytes());
                        }
                        if items.contains("INTERNALDATE") {
                            item.extend(format!(" INTERNALDATE \"{}\"", m.date).bytes());
                        }
                        if items.contains("HEADER.FIELDS (MESSAGE-ID)") {
                            let h = header_block(&m.raw, "Message-ID");
                            item.extend(
                                format!(" BODY[HEADER.FIELDS (MESSAGE-ID)] {{{}}}\r\n", h.len())
                                    .bytes(),
                            );
                            item.extend(h);
                        } else if items.contains("BODY.PEEK[]") || items.contains("BODY[]") {
                            item.extend(format!(" BODY[] {{{}}}\r\n", m.raw.len()).bytes());
                            item.extend(m.raw.iter());
                        }
                        item.extend(b")\r\n");
                        out.extend(item);
                    }
                    if uid && out.is_empty() && srv.refuse_missing_fetch.load(Ordering::Relaxed) {
                        (ok("NO no such message"), false)
                    } else {
                        out.extend(ok("OK fetched"));
                        (out, false)
                    }
                }
                "STORE" => {
                    let mut parts = args.splitn(3, ' ');
                    let set = parts.next().unwrap_or("").to_string();
                    let op = parts.next().unwrap_or("").to_ascii_uppercase();
                    let values: Vec<String> = parts
                        .next()
                        .unwrap_or("")
                        .trim_matches(['(', ')'])
                        .split_whitespace()
                        .map(str::to_string)
                        .collect();
                    let mut out = Vec::new();
                    for (i, m) in b.msgs.iter_mut().enumerate() {
                        let seq = i as u32 + 1;
                        let picked = if uid {
                            in_set(&set, m.uid, max_uid)
                        } else {
                            in_set(&set, seq, max_seq)
                        };
                        if !picked {
                            continue;
                        }
                        if op.starts_with('+') {
                            for v in &values {
                                if !m.flags.contains(v) {
                                    m.flags.push(v.clone());
                                }
                            }
                        } else if op.starts_with('-') {
                            m.flags.retain(|f| !values.contains(f));
                        } else {
                            m.flags = values.clone();
                        }
                        if !op.contains(".SILENT") {
                            let line = format!(
                                "* {seq} FETCH (UID {} FLAGS ({}))\r\n",
                                m.uid,
                                m.flags.join(" ")
                            );
                            out.extend(line.bytes());
                        }
                    }
                    out.extend(ok("OK stored"));
                    (out, false)
                }
                "COPY" | "MOVE" => {
                    let t = tokens(args);
                    let set = t.first().map(Tok::text).unwrap_or_default();
                    let to = t.get(1).map(Tok::text).unwrap_or_default();
                    let picked: Vec<Msg> = b
                        .msgs
                        .iter()
                        .enumerate()
                        .filter(|(i, m)| hit(&set, *i as u32 + 1, m))
                        .map(|(_, m)| m.clone())
                        .collect();
                    if !boxes.contains_key(&to) {
                        return (
                            ok(&format!(
                                "NO [TRYCREATE] Mailbox doesn't exist: {to} (0.001 + 0.000 secs)."
                            )),
                            false,
                        );
                    }
                    if cmd == "MOVE"
                        && let Some(source) = boxes.get_mut(&sel)
                    {
                        source
                            .msgs
                            .retain(|m| !picked.iter().any(|p| p.uid == m.uid));
                    }
                    let dest = boxes.get_mut(&to).expect("checked above");
                    for mut copy in picked {
                        copy.uid = dest.next;
                        copy.flags.retain(|f| f != "\\Deleted");
                        dest.next += 1;
                        dest.msgs.push(copy);
                    }
                    (ok("OK done"), false)
                }
                "EXPUNGE" => {
                    let set = uid.then(|| args.trim().to_string());
                    let mut out = Vec::new();
                    while let Some(pos) = b.msgs.iter().position(|m| {
                        m.flags.iter().any(|f| f == "\\Deleted")
                            && set.as_deref().is_none_or(|s| in_set(s, m.uid, max_uid))
                    }) {
                        b.msgs.remove(pos);
                        out.extend(format!("* {} EXPUNGE\r\n", pos + 1).bytes());
                    }
                    out.extend(ok("OK expunged"));
                    (out, false)
                }
                _ => (ok("BAD unknown command"), false),
            }
        }
    }
}

/// Serves `srv` on a loopback port, and returns the port.
pub(crate) async fn serve(srv: Arc<Imap>) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            let srv = Arc::clone(&srv);
            tokio::spawn(async move {
                let (rx, mut tx) = sock.into_split();
                let mut reader = BufReader::new(rx);
                if tx
                    .write_all(b"* OK stateful scripted IMAP ready\r\n")
                    .await
                    .is_err()
                {
                    return;
                }
                let mut selected = None;
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let command = line.trim_end().to_string();
                    srv.lines.lock().unwrap().push(command.clone());
                    if let Some(n) = literal_len(&command) {
                        if tx.write_all(b"+ OK\r\n").await.is_err() {
                            return;
                        }
                        let mut buf = vec![0u8; n];
                        if reader.read_exact(&mut buf).await.is_err() {
                            return;
                        }
                        let mut trail = String::new();
                        if reader.read_line(&mut trail).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let (reply, close) = append_literal(&srv, &command, &buf);
                        if tx.write_all(&reply).await.is_err() || close {
                            let _ = tx.shutdown().await;
                            return;
                        }
                        continue;
                    }
                    let garbled = {
                        let mut garble = srv.garble.lock().unwrap();
                        let hit = garble
                            .as_ref()
                            .is_some_and(|(cmd, _)| command.to_ascii_uppercase().contains(cmd));
                        if hit {
                            garble.take().map(|(_, bytes)| bytes)
                        } else {
                            None
                        }
                    };
                    let (reply, close) = match garbled {
                        Some(bytes) => (bytes, false),
                        None => answer(&srv, &mut selected, &command),
                    };
                    if tx.write_all(&reply).await.is_err() || close {
                        let _ = tx.shutdown().await;
                        return;
                    }
                }
            });
        }
    });
    port
}

/// A plain message with a Message-ID.
pub(crate) fn raw_msg(id: &str, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Dana Wu <dana@example.com>\r\nTo: Sam Ortiz <sam@example.com>\r\nSubject: {subject}\r\n\
         Date: Wed, 01 Oct 2026 09:00:00 +0000\r\nMessage-ID: <{id}>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}
