//! Builds sample `.eml` and Gmail-style `.mbox` files for testing.
//!
//! - `build_eml()`: the same email as the sample `.msg`: an inline logo
//!   plus a regular PDF, XLSX and a mislabelled "notes.docx".
//! - `build_mbox()`: a Google Takeout-style mailbox with three messages:
//!   one that passes validation, one with only a regular attachment, and
//!   one with no attachments. Each carries Gmail's X-GM-THRID and
//!   X-Gmail-Labels headers, as Takeout exports do.

const LOGO_CID: &str = "logo.png@01DA76F1.2C5B8E40";
const PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";
const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
const XLSX: &[u8] =
    b"PK\x03\x04\x14\0\0\0\0\0[Content_Types].xml...PK\x03\x04\x14\0\0\0\0\0xl/workbook.xml...";
const FAKE_DOCX: &[u8] = b"These are just plain-text notes, not a real Word file.";

/// Standard base64 (RFC 4648), wrapped at 76 characters as email requires.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        // Pack up to 3 bytes into 24 bits, then emit 4 six-bit characters.
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    let mut wrapped = String::new();
    for (i, c) in out.chars().enumerate() {
        if i > 0 && i % 76 == 0 {
            wrapped.push_str("\r\n");
        }
        wrapped.push(c);
    }
    wrapped
}

/// One MIME attachment part.
fn attachment_part(boundary: &str, mime: &str, name: &str, data: &[u8]) -> String {
    format!(
        "--{boundary}\r\n\
         Content-Type: {mime}; name=\"{name}\"\r\n\
         Content-Disposition: attachment; filename=\"{name}\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n\
         {}\r\n",
        base64(data)
    )
}

const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// The full sample email: 1 inline image + 3 regular attachments.
pub fn build_eml() -> String {
    build_full_email("")
}

fn build_full_email(extra_headers: &str) -> String {
    let mut eml = format!(
        "{extra_headers}\
         From: Alex Rivera <alex.rivera@example.com>\r\n\
         To: Sam Lee <sam.lee@example.com>\r\n\
         Cc: Jordan Kim <jordan.kim@example.com>\r\n\
         Subject: Q3 report and budget\r\n\
         Date: Fri, 15 Mar 2024 07:30:00 -0700\r\n\
         Message-ID: <CAB12345@mail.example.com>\r\n\
         Importance: High\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"mixed\"\r\n\r\n\
         --mixed\r\n\
         Content-Type: multipart/related; boundary=\"related\"\r\n\r\n\
         --related\r\n\
         Content-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
         --alt\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Hi team, the Q3 report and budget are attached.\r\n\
         --alt\r\n\
         Content-Type: text/html; charset=utf-8\r\n\r\n\
         <html><body><p><img src=\"cid:{LOGO_CID}\" alt=\"Logo\"></p>\
         <p>Hi team, the Q3 report and budget are attached.</p></body></html>\r\n\
         --alt--\r\n\
         --related\r\n\
         Content-Type: image/png; name=\"logo.png\"\r\n\
         Content-Disposition: inline; filename=\"logo.png\"\r\n\
         Content-ID: <{LOGO_CID}>\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n\
         {}\r\n\
         --related--\r\n",
        base64(PNG)
    );
    eml.push_str(&attachment_part(
        "mixed",
        "application/pdf",
        "Q3 Report.pdf",
        PDF,
    ));
    eml.push_str(&attachment_part("mixed", XLSX_MIME, "Budget.xlsx", XLSX));
    eml.push_str(&attachment_part(
        "mixed",
        DOCX_MIME,
        "notes.docx",
        FAKE_DOCX,
    ));
    eml.push_str("--mixed--\r\n");
    eml
}

/// Headers Google Takeout adds to each exported message, plus the kind of
/// Received header Gmail's servers write.
fn gmail_headers(thread_id: &str, labels: &str, received: &str) -> String {
    format!(
        "X-GM-THRID: {thread_id}\r\n\
         X-Gmail-Labels: {labels}\r\n\
         Received: by 2002:a05:6402:1234 with SMTP id abc123;\r\n        \
         {received}\r\n\
         X-Google-Smtp-Source: AGHT+IExampleSourceValue==\r\n"
    )
}

/// A Google Takeout-style mailbox with three messages.
pub fn build_mbox() -> String {
    let first = build_full_email(&gmail_headers(
        "1793512345678901234",
        "Inbox,Important,Opened,\"Projects, Q3\"",
        "Fri, 15 Mar 2024 07:30:01 -0700",
    ));

    let second = format!(
        "{}From: Sam Lee <sam.lee@example.com>\r\n\
         To: Alex Rivera <alex.rivera@example.com>\r\n\
         Subject: Re: Q3 report and budget\r\n\
         Date: Fri, 15 Mar 2024 09:05:00 -0700\r\n\
         Message-ID: <CAB67890@mail.example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"mixed\"\r\n\r\n\
         --mixed\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Thanks! Signed copy attached.\r\n\
         {}--mixed--\r\n",
        gmail_headers(
            "1793512345678901234",
            "Sent,Opened",
            "Fri, 15 Mar 2024 09:05:01 -0700"
        ),
        attachment_part("mixed", "application/pdf", "Q3 Report (signed).pdf", PDF)
    );

    let third = format!(
        "{}From: Calendar <calendar-notification@google.com>\r\n\
         To: Alex Rivera <alex.rivera@example.com>\r\n\
         Subject: Reminder: Q3 review @ 10am\r\n\
         Date: Mon, 18 Mar 2024 08:00:00 -0700\r\n\
         Message-ID: <reminder-42@google.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\n\
         Your event starts in 2 hours.\r\n",
        gmail_headers(
            "1793599999999999999",
            "Inbox,Category Updates",
            "Mon, 18 Mar 2024 08:00:02 -0700"
        )
    );

    // Each message starts with a "From " envelope line; a blank line ends it.
    format!(
        "From 1793512345678901234@xxx Fri Mar 15 14:30:00 +0000 2024\n{first}\n\
         From 1793518765432109876@xxx Fri Mar 15 16:05:00 +0000 2024\n{second}\n\
         From 1793599999999999999@xxx Mon Mar 18 15:00:00 +0000 2024\n{third}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_examples() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
