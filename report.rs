//! Printing results. Nothing in here computes anything; it only formats.

use crate::email::{EmailInfo, Placement, TypeCheck};
use crate::mbox::{Mailbox, Summary};
use crate::stats::FileStats;

/// Width of the label column. A `const` is fixed at compile time (chapter 3).
const LABEL_WIDTH: usize = 22;

fn heading(title: &str) {
    println!();
    println!("{title}");
    println!("{}", "=".repeat(LABEL_WIDTH + 16));
}

fn row(label: &str, value: impl std::fmt::Display) {
    // {:<width$} left-aligns the label in a column `width` characters wide.
    println!("{label:<width$}{value}", width = LABEL_WIDTH);
}

/// Print an optional value, or "(none)" if it's missing.
fn opt_row(label: &str, value: &Option<String>) {
    row(label, value.as_deref().unwrap_or("(none)"));
}

/// Shorten text to `max` characters, adding "..." if it was cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

pub fn print_text_report(path: &str, stats: &FileStats) {
    heading(&format!("File analysis: {path}"));

    // An array of (label, value) tuples: a compound type from chapter 3.
    let rows = [
        ("Size (bytes)", stats.bytes.to_string()),
        ("Lines", stats.lines.to_string()),
        ("  Non-blank", stats.non_blank_lines().to_string()),
        ("  Blank", stats.blank_lines.to_string()),
        ("Words", stats.words.to_string()),
        ("Characters", stats.chars.to_string()),
        ("  Letters", stats.letters.to_string()),
        ("  Digits", stats.digits.to_string()),
        ("  Whitespace", stats.whitespace.to_string()),
        ("  Punctuation", stats.punctuation.to_string()),
        (
            "Avg word length",
            format!("{:.2} chars", stats.average_word_length()),
        ),
        (
            "Longest line",
            format!(
                "line {} ({} chars)",
                stats.longest_line.0, stats.longest_line.1
            ),
        ),
    ];

    // Destructure each tuple directly in the `for` pattern.
    for (label, value) in rows {
        row(label, value);
    }
}

pub fn print_email_report(title: &str, info: &EmailInfo) {
    heading(&format!("{}: {title}", info.format));

    opt_row("Subject", &info.subject);
    opt_row("From", &info.sender);
    for r in &info.recipients {
        let who = match &r.email {
            Some(email) if *email != r.name => format!("{} <{email}>", r.name),
            _ => r.name.clone(),
        };
        row(r.kind, who);
    }
    opt_row("Sent", &info.sent);
    if info.received.is_some() {
        opt_row("Received", &info.received);
    }
    if info.created.is_some() {
        opt_row("Created", &info.created);
    }
    row("Importance", info.importance.unwrap_or("Normal"));
    if info.message_class.is_some() {
        opt_row("Message class", &info.message_class);
    }
    opt_row("Message-ID", &info.message_id);
    let formats = if info.body_formats.is_empty() {
        "(none)".to_string()
    } else {
        info.body_formats.join(", ")
    };
    row("Body formats", formats);
    row(
        "Internet headers",
        if info.has_internet_headers {
            "present"
        } else {
            "absent"
        },
    );
    if let Some(flag) = info.has_attachments_flag {
        row("Has-attachments flag", if flag { "set" } else { "not set" });
    }

    if let Some(gmail) = &info.gmail {
        let labels = if gmail.labels.is_empty() {
            "(none)".to_string()
        } else {
            gmail.labels.join(", ")
        };
        row("Gmail labels", labels);
        opt_row("Gmail thread ID", &gmail.thread_id);
        if gmail.google_headers {
            row("Gmail servers", "sent or received through Google");
        }
    }

    heading(&format!("Attachments ({})", info.attachments.len()));
    if info.attachments.is_empty() {
        println!("(none)");
    }
    for (i, a) in info.attachments.iter().enumerate() {
        let placement = match a.placement {
            Placement::Inline => "INLINE",
            Placement::Regular => "REGULAR",
        };
        println!("[{}] {}  ({placement})", i + 1, a.file_name);
        if a.size == 0 && a.method != "file" {
            row("    Size", "(not recorded)");
        } else {
            row("    Size", format!("{} bytes", a.size));
        }
        row("    Stored as", a.method);
        row("    MIME type", a.mime_type.as_deref().unwrap_or("(none)"));
        if let Some(cid) = &a.content_id {
            row("    Content-ID", cid);
        }
        let check = match a.check {
            TypeCheck::Match => format!("OK, {}", a.detected.label()),
            TypeCheck::Mismatch => format!(
                "MISMATCH: named like {}, but content is {}",
                a.expected.map(|t| t.label()).unwrap_or("?"),
                a.detected.label()
            ),
            TypeCheck::Unverified => {
                format!("not checked (content looks like {})", a.detected.label())
            }
            TypeCheck::Empty => "EMPTY: no data".to_string(),
        };
        row("    Type check", check);
        for note in &a.notes {
            row("    Why", note);
        }
    }

    let v = info.validation();
    let mark = |n: usize| if n > 0 { "present" } else { "MISSING" };
    heading("Validation");
    row(
        "Inline attachments",
        format!("{}  {}", v.inline, mark(v.inline)),
    );
    row(
        "Regular attachments",
        format!("{}  {}", v.regular, mark(v.regular)),
    );
    row("Type problems", v.type_problems);
    if v.passed() {
        row(
            "Result",
            "PASS: both inline and regular attachments are present",
        );
    } else {
        row(
            "Result",
            "FAIL: inline and regular attachments are not both present",
        );
    }
}

/// One line per message, then totals.
pub fn print_mbox_summary(path: &str, mailbox: &Mailbox) {
    heading(&format!("Mailbox (.mbox): {path}"));

    let ok: Vec<&Summary> = mailbox
        .messages
        .iter()
        .filter_map(|m| m.as_ref().ok())
        .collect();
    let passed = ok.iter().filter(|m| m.passed()).count();
    let with_attachments = ok.iter().filter(|m| m.inline + m.regular > 0).count();
    let type_problems = ok.iter().filter(|m| m.type_problems > 0).count();
    let gmail = ok.iter().filter(|m| m.has_gmail_labels).count();

    row("Messages", mailbox.messages.len());
    row("Unreadable", mailbox.messages.len() - ok.len());
    row("With any attachment", with_attachments);
    row(
        "Passed validation",
        format!("{passed}  (both inline and regular attachments)"),
    );
    row("With type problems", type_problems);
    row(
        "Gmail Takeout export",
        if gmail > 0 {
            format!("yes ({gmail} messages have Gmail labels)")
        } else {
            "no".to_string()
        },
    );

    println!();
    println!(
        "{:>5}  {:<10}  {:>6}  {:>7}  {:<6}  Subject",
        "#", "Date", "Inline", "Regular", "Result"
    );
    for (i, message) in mailbox.messages.iter().enumerate() {
        match message {
            Ok(m) => {
                let date: String = m.date.as_deref().unwrap_or("").chars().take(10).collect();
                let subject = truncate(m.subject.as_deref().unwrap_or("(no subject)"), 50);
                let result = if m.passed() { "PASS" } else { "FAIL" };
                println!(
                    "{:>5}  {date:<10}  {:>6}  {:>7}  {result:<6}  {subject}",
                    i + 1,
                    m.inline,
                    m.regular
                );
            }
            Err(error) => println!("{:>5}  could not read this message: {error}", i + 1),
        }
    }
    println!();
    println!("Show one message in full with: --message <#>");
}
