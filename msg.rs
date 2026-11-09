//! Reading Outlook `.msg` files.
//!
//! A `.msg` file is a *Compound File Binary* (CFB) container: a tiny file
//! system packed into one file. Its "folders" are called *storages* and its
//! "files" are called *streams*. Outlook stores every piece of an email as a
//! numbered *property* inside that container:
//!
//! ```text
//! /__properties_version1.0           fixed-size values: dates, flags, numbers
//! /__substg1.0_0037001F              property 0x0037 (subject), type 0x001F (UTF-16 text)
//! /__recip_version1.0_#00000000/     one storage per recipient
//! /__attach_version1.0_#00000000/    one storage per attachment
//!     __substg1.0_3707001F           attachment file name
//!     __substg1.0_37010102           attachment bytes (type 0x0102 = binary)
//! ```
//!
//! Specs: Microsoft [MS-OXMSG] (file layout) and [MS-OXPROPS] (property IDs).

use std::collections::HashMap;
use std::io::{Cursor, Read};

use cfb::CompoundFile;
use encoding_rs::{Encoding, WINDOWS_1252};

use crate::email::{
    Attachment, CFB_SIGNATURE, EmailInfo, FileType, Placement, Recipient, contains, unix_to_string,
};
use crate::rtf;

/// A compound file read from an in-memory byte slice.
type Cfb<'a> = CompoundFile<Cursor<&'a [u8]>>;

// --- Property types (the last 4 hex digits of a stream name) ---------------
const PT_STRING8: u16 = 0x001E; // 8-bit text in the message's code page
const PT_UNICODE: u16 = 0x001F; // UTF-16LE text
const PT_BINARY: u16 = 0x0102; // raw bytes

// --- Property IDs (the first 4 hex digits) ---------------------------------
const PR_IMPORTANCE: u16 = 0x0017;
const PR_MESSAGE_CLASS: u16 = 0x001A;
const PR_SUBJECT: u16 = 0x0037;
const PR_CLIENT_SUBMIT_TIME: u16 = 0x0039; // when it was sent
const PR_TRANSPORT_MESSAGE_HEADERS: u16 = 0x007D;
const PR_RECIPIENT_TYPE: u16 = 0x0C15;
const PR_SENDER_NAME: u16 = 0x0C1A;
const PR_SENDER_EMAIL_ADDRESS: u16 = 0x0C1F;
const PR_MESSAGE_DELIVERY_TIME: u16 = 0x0E06; // when it was received
const PR_MESSAGE_FLAGS: u16 = 0x0E07;
const PR_ATTACH_SIZE: u16 = 0x0E20;
const PR_BODY: u16 = 0x1000;
const PR_RTF_COMPRESSED: u16 = 0x1009;
const PR_HTML: u16 = 0x1013;
const PR_INTERNET_MESSAGE_ID: u16 = 0x1035;
const PR_DISPLAY_NAME: u16 = 0x3001;
const PR_EMAIL_ADDRESS: u16 = 0x3003;
const PR_CREATION_TIME: u16 = 0x3007;
const PR_ATTACH_DATA: u16 = 0x3701;
const PR_ATTACH_EXTENSION: u16 = 0x3703;
const PR_ATTACH_FILENAME: u16 = 0x3704;
const PR_ATTACH_METHOD: u16 = 0x3705;
const PR_ATTACH_LONG_FILENAME: u16 = 0x3707;
const PR_ATTACH_MIME_TAG: u16 = 0x370E;
const PR_ATTACH_CONTENT_ID: u16 = 0x3712;
const PR_ATTACH_FLAGS: u16 = 0x3714;
const PR_SMTP_ADDRESS: u16 = 0x39FE;
const PR_SENDER_SMTP_ADDRESS: u16 = 0x5D01;
const PR_INTERNET_CPID: u16 = 0x3FDE;
const PR_MESSAGE_CODEPAGE: u16 = 0x3FFD;
const PR_ATTACHMENT_HIDDEN: u16 = 0x7FFE;

// --- Flag values -----------------------------------------------------------
const MSGFLAG_HASATTACH: u32 = 0x10;
const ATT_MHTML_REF: u32 = 0x4; // attachment is referenced by the HTML body
const ATTACH_BY_VALUE: u32 = 1;
const ATTACH_EMBEDDED_MSG: u32 = 5;
const ATTACH_OLE: u32 = 6;

/// Header sizes of the `__properties_version1.0` stream, which differ by
/// where the stream sits ([MS-OXMSG] section 2.4).
const TOP_LEVEL_HEADER: usize = 32;
const CHILD_HEADER: usize = 8; // recipients and attachments

// ===========================================================================
// Reading the container
// ===========================================================================

/// Quick check: is this byte buffer an Outlook message?
pub fn is_msg(bytes: &[u8]) -> bool {
    bytes.starts_with(&CFB_SIGNATURE)
        && CompoundFile::open(Cursor::new(bytes))
            .map(|cfb| cfb.is_stream("/__properties_version1.0"))
            .unwrap_or(false)
}

/// Parse a `.msg` file that has already been read into memory.
pub fn analyze(bytes: &[u8]) -> Result<EmailInfo, String> {
    let mut cfb = CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| format!("not a valid .msg file: {e}"))?;

    let props = read_props(&mut cfb, "", TOP_LEVEL_HEADER);

    // Older messages store text as 8-bit bytes in a Windows code page
    // (1252 = Western European, 1251 = Cyrillic, ...). Find out which.
    let enc = props
        .long(PR_INTERNET_CPID)
        .or_else(|| props.long(PR_MESSAGE_CODEPAGE))
        .and_then(|cp| codepage::to_encoding(cp as u16))
        .unwrap_or(WINDOWS_1252);

    // --- Bodies: needed to find inline-image references like "cid:..." ---
    let html = get_binary(&mut cfb, "", PR_HTML)
        .or_else(|| get_string(&mut cfb, enc, "", PR_HTML).map(String::into_bytes));
    let rtf = get_binary(&mut cfb, "", PR_RTF_COMPRESSED).and_then(|c| rtf::decompress(&c));
    let plain = get_string(&mut cfb, enc, "", PR_BODY);

    let mut body_formats = Vec::new();
    if html.is_some() {
        body_formats.push("HTML");
    }
    if let Some(r) = &rtf {
        // Outlook often stores HTML mail *inside* RTF, marked by \fromhtml1.
        body_formats.push(if contains(r, b"\\fromhtml1") {
            "RTF (wrapping HTML)"
        } else {
            "RTF"
        });
    }
    if plain.is_some() {
        body_formats.push("Plain text");
    }

    // One lowercase blob of all body text to search for cid: references.
    let mut body_search = Vec::new();
    for part in [&html, &rtf].into_iter().flatten() {
        body_search.extend_from_slice(part);
    }
    let body_search = String::from_utf8_lossy(&body_search).to_lowercase();

    // --- Sender ---
    let sender_name = get_string(&mut cfb, enc, "", PR_SENDER_NAME);
    let sender_email = get_string(&mut cfb, enc, "", PR_SENDER_SMTP_ADDRESS)
        .or_else(|| get_string(&mut cfb, enc, "", PR_SENDER_EMAIL_ADDRESS));
    let sender = match (sender_name, sender_email) {
        (Some(n), Some(e)) if n != e => Some(format!("{n} <{e}>")),
        (Some(n), _) => Some(n),
        (None, Some(e)) => Some(e),
        (None, None) => None,
    };

    let importance = props.long(PR_IMPORTANCE).map(|v| match v {
        0 => "Low",
        2 => "High",
        _ => "Normal",
    });

    // --- Child storages: recipients and attachments ---
    // Collect the names first: `read_root_storage` borrows `cfb`, and we
    // need to borrow it mutably again to open streams (chapter 4 rules!).
    let mut recip_dirs = Vec::new();
    let mut attach_dirs = Vec::new();
    for entry in cfb.read_root_storage() {
        let name = entry.name();
        if name.starts_with("__recip_version1.0_#") {
            recip_dirs.push(format!("/{name}"));
        } else if name.starts_with("__attach_version1.0_#") {
            attach_dirs.push(format!("/{name}"));
        }
    }
    recip_dirs.sort();
    attach_dirs.sort();

    let recipients = recip_dirs
        .iter()
        .map(|dir| read_recipient(&mut cfb, enc, dir))
        .collect();

    let attachments = attach_dirs
        .iter()
        .map(|dir| read_attachment(&mut cfb, enc, dir, &body_search))
        .collect();

    Ok(EmailInfo {
        format: "Outlook message (.msg)",
        message_class: get_string(&mut cfb, enc, "", PR_MESSAGE_CLASS),
        subject: get_string(&mut cfb, enc, "", PR_SUBJECT),
        sender,
        recipients,
        sent: props.time(PR_CLIENT_SUBMIT_TIME).map(filetime_to_string),
        received: props.time(PR_MESSAGE_DELIVERY_TIME).map(filetime_to_string),
        created: props.time(PR_CREATION_TIME).map(filetime_to_string),
        importance,
        message_id: get_string(&mut cfb, enc, "", PR_INTERNET_MESSAGE_ID),
        body_formats,
        has_internet_headers: get_string(&mut cfb, enc, "", PR_TRANSPORT_MESSAGE_HEADERS).is_some(),
        has_attachments_flag: Some(
            props.long(PR_MESSAGE_FLAGS).unwrap_or(0) & MSGFLAG_HASATTACH != 0,
        ),
        gmail: None,
        attachments,
    })
}

fn read_recipient(cfb: &mut Cfb, enc: &'static Encoding, dir: &str) -> Recipient {
    let props = read_props(cfb, dir, CHILD_HEADER);
    let kind = match props.long(PR_RECIPIENT_TYPE) {
        Some(2) => "Cc",
        Some(3) => "Bcc",
        _ => "To",
    };
    let email = get_string(cfb, enc, dir, PR_SMTP_ADDRESS)
        .or_else(|| get_string(cfb, enc, dir, PR_EMAIL_ADDRESS));
    let name = get_string(cfb, enc, dir, PR_DISPLAY_NAME)
        .or_else(|| email.clone())
        .unwrap_or_else(|| "(no name)".to_string());
    Recipient { kind, name, email }
}

fn read_attachment(
    cfb: &mut Cfb,
    enc: &'static Encoding,
    dir: &str,
    body_search: &str,
) -> Attachment {
    let props = read_props(cfb, dir, CHILD_HEADER);
    let method_code = props.long(PR_ATTACH_METHOD).unwrap_or(ATTACH_BY_VALUE);

    let file_name = get_string(cfb, enc, dir, PR_ATTACH_LONG_FILENAME)
        .or_else(|| get_string(cfb, enc, dir, PR_ATTACH_FILENAME))
        .or_else(|| get_string(cfb, enc, dir, PR_DISPLAY_NAME))
        .unwrap_or_else(|| "(unnamed)".to_string());

    let extension = get_string(cfb, enc, dir, PR_ATTACH_EXTENSION)
        .or_else(|| file_name.rsplit_once('.').map(|(_, ext)| ext.to_string()));

    // Embedded emails and OLE objects are stored as a sub-storage, not bytes.
    let data_storage = format!("{dir}/__substg1.0_{PR_ATTACH_DATA:04X}000D");
    let (method, data, detected) = match method_code {
        ATTACH_EMBEDDED_MSG if cfb.is_storage(&data_storage) => {
            ("embedded message", Vec::new(), FileType::EmbeddedEmail)
        }
        ATTACH_OLE if cfb.is_storage(&data_storage) => {
            ("OLE object", Vec::new(), FileType::OleObject)
        }
        _ => {
            let data = get_binary(cfb, dir, PR_ATTACH_DATA).unwrap_or_default();
            let detected = FileType::detect(&data);
            ("file", data, detected)
        }
    };

    let size = if data.is_empty() {
        props.long(PR_ATTACH_SIZE).unwrap_or(0) as usize
    } else {
        data.len()
    };

    // --- Inline or regular? ---------------------------------------------
    // Outlook doesn't store a single "inline" switch, so we combine the
    // three signals the spec provides.
    let content_id = get_string(cfb, enc, dir, PR_ATTACH_CONTENT_ID)
        .map(|cid| cid.trim_matches(|c| c == '<' || c == '>').to_string());
    let flags = props.long(PR_ATTACH_FLAGS).unwrap_or(0);
    let hidden = props.boolean(PR_ATTACHMENT_HIDDEN);

    let mut notes = Vec::new();
    // `if let ... && ...` is a "let chain", available since Rust 1.88.
    if let Some(cid) = &content_id
        && body_search.contains(&format!("cid:{}", cid.to_lowercase()))
    {
        notes.push(format!("body references it as cid:{cid}"));
    }
    if flags & ATT_MHTML_REF != 0 {
        notes.push("flagged as referenced by the HTML body".to_string());
    }
    if hidden {
        notes.push("hidden from the attachment bar".to_string());
    }

    let placement = if notes.is_empty() {
        Placement::Regular
    } else {
        Placement::Inline
    };
    if placement == Placement::Regular && content_id.is_some() {
        notes.push("has a Content-ID, but the body never uses it".to_string());
    }

    // --- Does the content match the name? -------------------------------
    let stored_bytes = (method == "file").then_some(data.as_slice());
    let mime_type = get_string(cfb, enc, dir, PR_ATTACH_MIME_TAG);
    let (expected, check) = Attachment::type_check(
        &file_name,
        extension.as_deref(),
        mime_type.as_deref(),
        stored_bytes,
        detected,
    );

    Attachment {
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
    }
}

// ===========================================================================
// Low-level helpers
// ===========================================================================

/// Fixed-size properties from a `__properties_version1.0` stream.
struct Props {
    /// property ID -> the raw 8-byte value
    values: HashMap<u16, u64>,
}

impl Props {
    fn long(&self, id: u16) -> Option<u32> {
        self.values.get(&id).map(|v| *v as u32)
    }

    fn boolean(&self, id: u16) -> bool {
        self.values
            .get(&id)
            .map(|v| *v as u16 != 0)
            .unwrap_or(false)
    }

    /// A FILETIME: 100-nanosecond ticks since 1601-01-01 UTC.
    fn time(&self, id: u16) -> Option<u64> {
        self.values.get(&id).copied().filter(|t| *t != 0)
    }
}

/// Each entry is 16 bytes: tag (4), flags (4), value (8), after a header.
fn read_props(cfb: &mut Cfb, dir: &str, header_len: usize) -> Props {
    let mut values = HashMap::new();
    if let Some(data) = read_stream(cfb, &format!("{dir}/__properties_version1.0")) {
        for entry in data.get(header_len..).unwrap_or(&[]).chunks_exact(16) {
            let tag = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let id = (tag >> 16) as u16;
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&entry[8..16]);
            values.insert(id, u64::from_le_bytes(raw));
        }
    }
    Props { values }
}

fn read_stream(cfb: &mut Cfb, path: &str) -> Option<Vec<u8>> {
    if !cfb.is_stream(path) {
        return None;
    }
    let mut stream = cfb.open_stream(path).ok()?;
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

/// Read a text property, trying UTF-16 first and then 8-bit text in `enc`.
fn get_string(cfb: &mut Cfb, enc: &'static Encoding, dir: &str, id: u16) -> Option<String> {
    let text = if let Some(raw) =
        read_stream(cfb, &format!("{dir}/__substg1.0_{id:04X}{PT_UNICODE:04X}"))
    {
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        let raw = read_stream(cfb, &format!("{dir}/__substg1.0_{id:04X}{PT_STRING8:04X}"))?;
        enc.decode(&raw).0.into_owned()
    };
    let text = text.trim_end_matches('\0').trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn get_binary(cfb: &mut Cfb, dir: &str, id: u16) -> Option<Vec<u8>> {
    read_stream(cfb, &format!("{dir}/__substg1.0_{id:04X}{PT_BINARY:04X}"))
}

/// Convert a Windows FILETIME (100 ns ticks since 1601) to a UTC string.
pub fn filetime_to_string(filetime: u64) -> String {
    const SECONDS_1601_TO_1970: i64 = 11_644_473_600;
    unix_to_string((filetime / 10_000_000) as i64 - SECONDS_1601_TO_1970)
}

// ===========================================================================
// Tests
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::TypeCheck;
    use crate::sample_msg;

    #[test]
    fn converts_filetime() {
        // 2024-03-15 14:30:00 UTC
        let ft = (1_710_513_000 + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_to_string(ft), "2024-03-15 14:30:00 UTC");
    }

    #[test]
    fn reads_sample_message() {
        let bytes = sample_msg::build().unwrap();
        assert!(is_msg(&bytes));
        let info = analyze(&bytes).unwrap();

        assert_eq!(info.subject.as_deref(), Some("Q3 report and budget"));
        assert_eq!(info.recipients.len(), 2);
        assert_eq!(info.recipients[1].kind, "Cc");
        assert_eq!(info.attachments.len(), 4);

        let logo = &info.attachments[0];
        assert_eq!(logo.placement, Placement::Inline);
        assert_eq!(logo.check, TypeCheck::Match);

        let pdf = &info.attachments[1];
        assert_eq!(pdf.placement, Placement::Regular);
        assert_eq!(pdf.detected, FileType::Pdf);

        let fake = &info.attachments[3];
        assert_eq!(fake.check, TypeCheck::Mismatch);

        let v = info.validation();
        assert_eq!((v.inline, v.regular, v.type_problems), (1, 3, 1));
        assert!(v.passed());
    }

    #[test]
    fn plain_text_files_are_not_msg() {
        assert!(!is_msg(b"just some text"));
    }
}
