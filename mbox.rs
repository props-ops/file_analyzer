//! Reading mailboxes (`.mbox`), the format Google Takeout uses for Gmail.
//!
//! An mbox file is many `.eml` messages glued together. Each one starts
//! with a "From " line (no colon), for example:
//!
//! ```text
//! From 1780123456789012345@xxx Fri Mar 15 14:30:00 +0000 2024
//! X-GM-THRID: 1780123456789012345
//! X-Gmail-Labels: Inbox,Important,Opened
//! From: Alex Rivera <alex.rivera@example.com>
//! ...
//! ```
//!
//! Takeout exports can be many gigabytes, so we *stream* the file: read one
//! message, analyze it, keep only the small summary, move on. We never hold
//! the whole file in memory.

use std::fs::File;
use std::io::{self, BufReader};

use mail_parser::mailbox::mbox::MessageIterator;

use crate::email::{EmailInfo, unix_to_string};
use crate::eml;

/// A small summary of one message: one line of the report. We keep only
/// this (not the full `EmailInfo`) so a mailbox with hundreds of thousands
/// of messages still fits comfortably in memory.
pub struct Summary {
    pub date: Option<String>,
    pub subject: Option<String>,
    pub inline: usize,
    pub regular: usize,
    pub type_problems: usize,
    pub has_gmail_labels: bool,
}

impl Summary {
    fn from_info(info: &EmailInfo) -> Summary {
        let v = info.validation();
        Summary {
            date: info.sent.clone(),
            subject: info.subject.clone(),
            inline: v.inline,
            regular: v.regular,
            type_problems: v.type_problems,
            has_gmail_labels: info.gmail.as_ref().is_some_and(|g| !g.labels.is_empty()),
        }
    }

    pub fn passed(&self) -> bool {
        self.inline > 0 && self.regular > 0
    }
}

/// Result of reading a mailbox.
pub struct Mailbox {
    /// One entry per message, or why it couldn't be read.
    pub messages: Vec<Result<Summary, String>>,
    /// The full details of the one message asked for with `--message`.
    pub detail: Option<Result<EmailInfo, String>>,
}

/// An mbox starts with a "From " envelope line such as
/// `From someone@example.com Fri Mar 15 14:30:00 2024`, normally followed
/// by email headers.
pub fn looks_like_mbox(head: &[u8]) -> bool {
    if !head.starts_with(b"From ") {
        return false;
    }
    let newline = head.iter().position(|&b| b == b'\n').unwrap_or(head.len());
    let first_line = String::from_utf8_lossy(&head[..newline]);

    // Envelope line: "From", a sender, then a weekday name.
    let mut words = first_line.split_whitespace().skip(2);
    let weekday = words.next().unwrap_or("");
    let is_envelope = matches!(
        weekday,
        "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun"
    );

    is_envelope || eml::looks_like_eml(head.get(newline + 1..).unwrap_or(&[]))
}

/// Analyze the mailbox at `path`. If `detail` is `Some(n)`, keep the full
/// details of message `n` (counting from 1) and stop reading there.
pub fn analyze_file(path: &str, detail: Option<usize>) -> io::Result<Mailbox> {
    let reader = BufReader::new(File::open(path)?);
    let mut mailbox = Mailbox {
        messages: Vec::new(),
        detail: None,
    };

    for item in MessageIterator::new(reader) {
        let message = item?;
        let mut result = eml::analyze(message.contents(), "Message in a mailbox (.mbox)");

        // Fall back to the "From " line's date if there's no Received header.
        if let Ok(info) = &mut result
            && info.received.is_none()
            && message.internal_date() > 0
        {
            info.received = Some(unix_to_string(message.internal_date() as i64));
        }

        mailbox.messages.push(
            result
                .as_ref()
                .map(Summary::from_info)
                .map_err(Clone::clone),
        );

        if detail == Some(mailbox.messages.len()) {
            mailbox.detail = Some(result);
            break; // no need to read the rest of the file
        }
    }
    Ok(mailbox)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_eml;

    #[test]
    fn recognises_mbox() {
        assert!(looks_like_mbox(
            b"From abc Fri Mar 15 14:30:00 2024\nFrom: a@b.com\nSubject: x\n\n"
        ));
        assert!(!looks_like_mbox(
            b"From here on, this is just text.\nMore text."
        ));
    }

    #[test]
    fn reads_gmail_takeout_sample() {
        let path = std::env::temp_dir().join("file_analyzer_test.mbox");
        std::fs::write(&path, sample_eml::build_mbox()).unwrap();
        let path = path.to_str().unwrap();

        let mailbox = analyze_file(path, None).unwrap();
        assert_eq!(mailbox.messages.len(), 3);
        let passed = mailbox
            .messages
            .iter()
            .filter(|m| m.as_ref().is_ok_and(Summary::passed))
            .count();
        assert_eq!(passed, 1);

        // Asking for message 1 stops reading after it.
        let mailbox = analyze_file(path, Some(1)).unwrap();
        std::fs::remove_file(path).ok();
        assert_eq!(mailbox.messages.len(), 1);
        let first = mailbox.detail.unwrap().unwrap();
        let gmail = first.gmail.as_ref().expect("Gmail info");
        assert!(gmail.labels.contains(&"Important".to_string()));
        assert!(gmail.thread_id.is_some());
        assert!(first.validation().passed());
    }
}
