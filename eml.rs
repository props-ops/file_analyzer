//! Reading standard internet email (`.eml`, RFC 5322 / MIME).
//!
//! An `.eml` file is plain text: headers at the top (`From:`, `Subject:` ...),
//! a blank line, then the body. Emails with attachments use MIME: the body
//! is split into *parts* separated by a boundary line, and each part has its
//! own headers. A typical layout:
//!
//! ```text
//! multipart/mixed
//! ├── multipart/related            <- body + things the body displays
//! │   ├── multipart/alternative
//! │   │   ├── text/plain
//! │   │   └── text/html            <- contains <img src="cid:logo@x">
//! │   └── image/png                <- Content-ID: <logo@x>  => INLINE
//! └── application/pdf              <- Content-Disposition: attachment => REGULAR
//! ```
//!
//! The `mail-parser` crate does the MIME decoding for us.

use mail_parser::{Address, Message, MessageParser, MimeHeaders, PartType};

use crate::email::{
    Attachment, EmailInfo, FileType, GmailInfo, Placement, Recipient, contains, unix_to_string,
};

/// Does the start of this file look like email headers?
///
/// Every line up to the first blank line must be `Name: value` (or a
/// continuation line starting with a space), and at least one of the names
/// must be a standard email or MIME header.
pub fn looks_like_eml(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let mut header_count = 0;
    let mut has_key_header = false;

    for line in text.lines() {
        if line.is_empty() {
            break; // end of the header block
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if header_count == 0 {
                return false; // a continuation line with nothing to continue
            }
            continue;
        }
        let Some((name, _value)) = line.split_once(':') else {
            return false;
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic()) {
            return false;
        }
        header_count += 1;
        let name = name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "from"
                | "to"
                | "cc"
                | "subject"
                | "date"
                | "message-id"
                | "received"
                | "mime-version"
                | "return-path"
                | "delivered-to"
                | "content-type"
                | "content-transfer-encoding"
        ) {
            has_key_header = true;
        }
    }
    header_count >= 1 && has_key_header
}

/// Parse one email from raw bytes.
pub fn analyze(bytes: &[u8], format: &'static str) -> Result<EmailInfo, String> {
    let message = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| "could not parse this email".to_string())?;
    Ok(from_message(&message, format))
}

fn from_message(message: &Message, format: &'static str) -> EmailInfo {
    // --- Bodies -----------------------------------------------------------
    // mail-parser lists a plain-text part as the "HTML body" when there is no
    // real HTML, so we check each part's actual type.
    let mut html = String::new();
    for part in message.html_bodies() {
        if let PartType::Html(text) = &part.body {
            html.push_str(text);
        }
    }
    let has_text = message
        .text_bodies()
        .any(|part| matches!(part.body, PartType::Text(_)));

    let mut body_formats = Vec::new();
    if !html.is_empty() {
        body_formats.push("HTML");
    }
    if has_text {
        body_formats.push("Plain text");
    }
    let body_search = html.to_lowercase();

    // --- People -----------------------------------------------------------
    let sender = message
        .from()
        .and_then(|a| a.first())
        .map(|a| match (a.name(), a.address()) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name.to_string(),
            (None, Some(email)) => email.to_string(),
            (None, None) => "(unknown)".to_string(),
        });

    let mut recipients = Vec::new();
    push_recipients(&mut recipients, "To", message.to());
    push_recipients(&mut recipients, "Cc", message.cc());
    push_recipients(&mut recipients, "Bcc", message.bcc());

    // --- Attachments ------------------------------------------------------
    let attachments = message
        .attachments()
        .filter_map(|part| read_attachment(part, &body_search))
        .collect();

    EmailInfo {
        format,
        subject: message.subject().map(str::to_string),
        sender,
        recipients,
        sent: message.date().map(|d| unix_to_string(d.to_timestamp())),
        // The top Received header is written by the last server, i.e. when
        // the email was delivered to this mailbox.
        received: message
            .received()
            .and_then(|r| r.date())
            .map(|d| unix_to_string(d.to_timestamp())),
        created: None,
        importance: importance(message),
        message_id: message.message_id().map(|id| format!("<{id}>")),
        message_class: None,
        body_formats,
        has_internet_headers: true, // an .eml file *is* its headers + body
        has_attachments_flag: None,
        gmail: gmail_info(message),
        attachments,
    }
}

fn push_recipients(list: &mut Vec<Recipient>, kind: &'static str, address: Option<&Address>) {
    let Some(address) = address else { return };
    for addr in address.iter() {
        let email = addr.address().map(str::to_string);
        let name = addr
            .name()
            .map(str::to_string)
            .or_else(|| email.clone())
            .unwrap_or_else(|| "(no name)".to_string());
        list.push(Recipient { kind, name, email });
    }
}

fn read_attachment(part: &mail_parser::MessagePart, body_search: &str) -> Option<Attachment> {
    let disposition = part
        .content_disposition()
        .map(|d| d.ctype().to_ascii_lowercase());

    // An unnamed text part that isn't marked as an attachment is a piece of
    // the body (a mailing-list footer, a text/watch-html variant ...), not a
    // file. Counting it would make validation pass when it shouldn't.
    let is_text = matches!(part.body, PartType::Text(_) | PartType::Html(_));
    if is_text && part.attachment_name().is_none() && disposition.as_deref() != Some("attachment") {
        return None;
    }

    // What's stored, and what is it really?
    let (method, data, detected, size): (&'static str, Option<&[u8]>, FileType, usize) =
        match &part.body {
            PartType::Binary(bytes) | PartType::InlineBinary(bytes) => (
                "file",
                Some(bytes.as_ref()),
                FileType::detect(bytes),
                bytes.len(),
            ),
            PartType::Text(text) | PartType::Html(text) => {
                let bytes = text.as_bytes();
                ("file", Some(bytes), FileType::detect(bytes), bytes.len())
            }
            PartType::Message(inner) => (
                "embedded message",
                None,
                FileType::EmbeddedEmail,
                inner.raw_message().len(),
            ),
            PartType::Multipart(_) => return None, // a container, not an attachment
        };

    let file_name = part
        .attachment_name()
        .map(str::to_string)
        .or_else(|| {
            part.message()
                .and_then(|m| m.subject())
                .map(|s| format!("{s}.eml"))
        })
        .unwrap_or_else(|| "(unnamed)".to_string());

    let mime_type = part.content_type().map(|ct| match ct.subtype() {
        Some(sub) => format!("{}/{sub}", ct.ctype()),
        None => ct.ctype().to_string(),
    });
    let content_id = part
        .content_id()
        .map(|cid| cid.trim_matches(|c| c == '<' || c == '>').to_string());
    let location = part.content_location().filter(|l| !l.is_empty());

    // --- Inline or regular? ---------------------------------------------
    // A part is inline when the HTML body actually displays it. That is
    // also how Gmail and Outlook decide what to show in the attachment bar.
    let mut notes = Vec::new();
    if let Some(cid) = &content_id
        && contains(
            body_search.as_bytes(),
            format!("cid:{}", cid.to_lowercase()).as_bytes(),
        )
    {
        notes.push(format!("body references it as cid:{cid}"));
    } else if let Some(loc) = location
        && contains(body_search.as_bytes(), loc.to_lowercase().as_bytes())
    {
        notes.push(format!("body references its Content-Location ({loc})"));
    }
    if notes.is_empty()
        && disposition.as_deref() == Some("inline")
        && part.attachment_name().is_none()
    {
        notes.push("marked Content-Disposition: inline and has no file name".to_string());
    }

    let placement = if notes.is_empty() {
        Placement::Regular
    } else {
        Placement::Inline
    };
    if placement == Placement::Regular {
        if disposition.as_deref() == Some("inline") {
            notes.push(
                "marked Content-Disposition: inline, but the body never uses it, \
                 so mail apps list it as an attachment"
                    .to_string(),
            );
        } else if content_id.is_some() {
            notes.push("has a Content-ID, but the body never uses it".to_string());
        }
    }

    let (expected, check) =
        Attachment::type_check(&file_name, None, mime_type.as_deref(), data, detected);

    Some(Attachment {
        file_name,
        mime_type,
        size,
        method,
        content_id,
        placement,
        notes,
        expected,
        detected,
        check,
    })
}

fn importance(message: &Message) -> Option<&'static str> {
    let header = |name: &str| {
        message
            .header_raw(name.to_string())
            .map(|v| v.trim().to_ascii_lowercase())
    };
    if let Some(value) = header("Importance") {
        return Some(match value.as_str() {
            "high" => "High",
            "low" => "Low",
            _ => "Normal",
        });
    }
    // X-Priority: 1 (Highest) ... 5 (Lowest)
    header("X-Priority").map(|value| match value.chars().next() {
        Some('1') | Some('2') => "High",
        Some('4') | Some('5') => "Low",
        _ => "Normal",
    })
}

/// Gmail details: Takeout labels/thread IDs, or Google's sending headers.
fn gmail_info(message: &Message) -> Option<GmailInfo> {
    let unfold = |s: &str| s.replace("\r\n", "").replace('\n', "").trim().to_string();

    let labels = message
        .header_raw("X-Gmail-Labels".to_string())
        .map(|raw| split_labels(&unfold(raw)))
        .unwrap_or_default();
    let thread_id = message
        .header_raw("X-GM-THRID".to_string())
        .map(unfold)
        .filter(|s| !s.is_empty());

    // Look only at the header block (everything before the first blank line).
    let raw = message.raw_message();
    let header_end = find(raw, b"\r\n\r\n")
        .or_else(|| find(raw, b"\n\n"))
        .unwrap_or(raw.len());
    let headers = String::from_utf8_lossy(&raw[..header_end]).to_lowercase();
    let google_headers = [
        "x-google-smtp-source:",
        "x-gm-message-state:",
        "x-gm-gg:",
        ".google.com with smtp",
        "by mail.gmail.com",
    ]
    .iter()
    .any(|marker| headers.contains(marker));

    if labels.is_empty() && thread_id.is_none() && !google_headers {
        return None;
    }
    Some(GmailInfo {
        labels,
        thread_id,
        google_headers,
    })
}

/// Split `Inbox,Important,"Label, with comma"` into separate labels.
fn split_labels(raw: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in raw.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                labels.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    labels.push(current.trim().to_string());
    labels.retain(|label| !label.is_empty());
    labels
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::TypeCheck;
    use crate::sample_eml;

    #[test]
    fn recognises_email_headers() {
        assert!(looks_like_eml(b"From: a@b.com\r\nSubject: Hi\r\n\r\nbody"));
        assert!(!looks_like_eml(b"Hello, Rust world!\n\nNot an email."));
        assert!(!looks_like_eml(b"Note: just one line"));
    }

    #[test]
    fn splits_gmail_labels() {
        assert_eq!(
            split_labels("Inbox,Important,\"Work, 2024\",Opened"),
            vec!["Inbox", "Important", "Work, 2024", "Opened"]
        );
    }

    #[test]
    fn reads_sample_eml() {
        let raw = sample_eml::build_eml();
        assert!(looks_like_eml(raw.as_bytes()));
        let info = analyze(raw.as_bytes(), "test").unwrap();

        assert_eq!(info.subject.as_deref(), Some("Q3 report and budget"));
        assert_eq!(info.sent.as_deref(), Some("2024-03-15 14:30:00 UTC"));
        assert_eq!(info.recipients.len(), 2);
        assert_eq!(info.importance, Some("High"));

        let names: Vec<&str> = info
            .attachments
            .iter()
            .map(|a| a.file_name.as_str())
            .collect();
        assert_eq!(
            names,
            ["logo.png", "Q3 Report.pdf", "Budget.xlsx", "notes.docx"]
        );
        assert_eq!(info.attachments[0].placement, Placement::Inline);
        assert_eq!(info.attachments[1].placement, Placement::Regular);
        assert_eq!(info.attachments[1].check, TypeCheck::Match);
        assert_eq!(info.attachments[3].check, TypeCheck::Mismatch);

        let v = info.validation();
        assert_eq!((v.inline, v.regular, v.type_problems), (1, 3, 1));
        assert!(v.passed());
        assert!(info.gmail.is_none());
    }
}
