//! Reading mbox files — the inverse of the store's writer, same dialect.
//!
//! The writer separates messages with a `From ` line, escapes body lines that
//! begin `From ` to `>From `, normalises line endings to LF, and leaves a
//! blank line after each message. This reader inverts exactly that. The
//! dialect (mboxo) has one known flaw, inherited knowingly: a body line that
//! was *originally* `>From ` is indistinguishable from an escaped one and
//! comes back unescaped. Every mbox tool that speaks this dialect shares it.

/// Splits an mbox file into raw RFC822 messages.
///
/// A separator is a line beginning `From ` at the start of the file or after
/// a blank line — both conditions, so an unescaped `From ` in the middle of a
/// paragraph (from a foreign writer) does not shear a message in two. Line
/// endings come back as CRLF, the canonical form the rest of the pipeline
/// stores.
pub fn split(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut messages: Vec<Vec<u8>> = Vec::new();
    let mut current: Option<Vec<u8>> = None;
    let mut previous_blank = true; // start of file counts

    for line in bytes.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if previous_blank && line.starts_with(b"From ") {
            if let Some(done) = current.take() {
                messages.push(trim_trailing_blank(done));
            }
            current = Some(Vec::new());
            previous_blank = false;
            continue;
        }
        if let Some(msg) = current.as_mut() {
            let unescaped = line.strip_prefix(b">").filter(|r| r.starts_with(b"From "));
            msg.extend_from_slice(unescaped.unwrap_or(line));
            msg.extend_from_slice(b"\r\n");
        }
        previous_blank = line.is_empty();
    }
    if let Some(done) = current.take() {
        messages.push(trim_trailing_blank(done));
    }
    messages
}

/// The writer puts a blank line after each message; it is the file's
/// punctuation, not the message's.
fn trim_trailing_blank(mut msg: Vec<u8>) -> Vec<u8> {
    while msg.ends_with(b"\r\n\r\n") {
        msg.truncate(msg.len() - 2);
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_unescapes_the_writers_own_dialect() {
        let file = b"From a@example.com Mon Jan  1 00:00:00 2024\n\
Subject: one\n\
\n\
body one\n\
>From the mountains I write\n\
\n\
From b@example.com Mon Jan  1 00:00:01 2024\n\
Subject: two\n\
\n\
>From a blank line this would shear the message without its escape\n\
still message two\n\
\n";
        let msgs = split(file);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        let one = String::from_utf8_lossy(&msgs[0]);
        assert!(one.contains("Subject: one"), "{one}");
        assert!(one.contains("From the mountains"), "{one}");
        assert!(!one.contains(">From"), "escaping undone: {one}");
        let two = String::from_utf8_lossy(&msgs[1]);
        assert!(two.contains("still message two"), "{two}");
        assert!(two.contains("From a blank line"), "{two}");
        assert!(!two.contains(">From"), "{two}");
    }

    #[test]
    fn an_unescaped_mid_paragraph_from_does_not_shear_the_message() {
        // A foreign writer that forgot to escape: the line is not after a
        // blank line, so it stays body.
        let file = b"From a@example.com Mon Jan  1 00:00:00 2024\n\
Subject: one\n\
\n\
first line\n\
From here on it gets interesting\n\
last line\n";
        let msgs = split(file);
        assert_eq!(msgs.len(), 1);
        assert!(String::from_utf8_lossy(&msgs[0]).contains("interesting"));
    }

    #[test]
    fn an_empty_file_is_no_messages() {
        assert!(split(b"").is_empty());
        assert!(split(b"\n\n").is_empty());
    }
}
