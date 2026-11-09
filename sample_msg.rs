//! Builds a small, valid `.msg` file for testing.
//!
//! Run `cargo run -- --make-sample sample.msg` to write one to disk. It holds:
//! - an inline logo image (referenced from the HTML body by `cid:`),
//! - a regular PDF and a regular Excel workbook,
//! - a regular "notes.docx" that is really plain text (to trigger a
//!   type-mismatch warning).
//!
//! The PDF and XLSX are tiny stand-ins with the right signatures. They are
//! enough for type detection, but Acrobat or Excel won't open them.

use std::io::{self, Cursor, Write};

use cfb::CompoundFile;

type Writer = CompoundFile<Cursor<Vec<u8>>>;

const PT_LONG: u16 = 0x0003;
const PT_BOOLEAN: u16 = 0x000B;
const PT_SYSTIME: u16 = 0x0040;
const PT_UNICODE: u16 = 0x001F;
const PT_BINARY: u16 = 0x0102;
const READ_WRITE: u32 = 0x6;

/// 2024-03-15 14:30:00 UTC as a Windows FILETIME.
const SENT: u64 = (1_710_513_000 + 11_644_473_600) * 10_000_000;
const RECEIVED: u64 = SENT + 42 * 10_000_000; // 42 seconds later

const LOGO_CID: &str = "logo.png@01DA76F1.2C5B8E40";

/// Collects 16-byte property entries for one `__properties_version1.0` stream.
struct PropList {
    entries: Vec<u8>,
}

impl PropList {
    fn new() -> Self {
        PropList {
            entries: Vec::new(),
        }
    }

    fn fixed(&mut self, id: u16, prop_type: u16, value: u64) {
        let tag = (u32::from(id) << 16) | u32::from(prop_type);
        self.entries.extend_from_slice(&tag.to_le_bytes());
        self.entries.extend_from_slice(&READ_WRITE.to_le_bytes());
        self.entries.extend_from_slice(&value.to_le_bytes());
    }

    fn write(&self, cfb: &mut Writer, dir: &str, header: &[u8]) -> io::Result<()> {
        let mut data = header.to_vec();
        data.extend_from_slice(&self.entries);
        put(cfb, &format!("{dir}/__properties_version1.0"), &data)
    }
}

fn put(cfb: &mut Writer, path: &str, data: &[u8]) -> io::Result<()> {
    cfb.create_stream(path)?.write_all(data)
}

/// Write a UTF-16 text property and record its size in the property list.
fn text(cfb: &mut Writer, props: &mut PropList, dir: &str, id: u16, value: &str) -> io::Result<()> {
    let mut bytes: Vec<u8> = value.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(&[0, 0]); // null terminator
    props.fixed(id, PT_UNICODE, bytes.len() as u64);
    put(
        cfb,
        &format!("{dir}/__substg1.0_{id:04X}{PT_UNICODE:04X}"),
        &bytes,
    )
}

fn binary(
    cfb: &mut Writer,
    props: &mut PropList,
    dir: &str,
    id: u16,
    value: &[u8],
) -> io::Result<()> {
    props.fixed(id, PT_BINARY, value.len() as u64);
    put(
        cfb,
        &format!("{dir}/__substg1.0_{id:04X}{PT_BINARY:04X}"),
        value,
    )
}

struct SampleAttachment<'a> {
    name: &'a str,
    mime: &'a str,
    data: Vec<u8>,
    content_id: Option<&'a str>,
    hidden: bool,
}

pub fn build() -> io::Result<Vec<u8>> {
    let mut cfb = CompoundFile::create(Cursor::new(Vec::new()))?;

    let html = format!(
        "<html><body><p><img src=\"cid:{LOGO_CID}\" alt=\"Logo\"></p>\
         <p>Hi team, the Q3 report and budget are attached.</p></body></html>"
    );
    let plain = "Hi team, the Q3 report and budget are attached.";

    // Compressed-RTF stream using the "MELA" (stored, not compressed) type.
    let rtf_text =
        format!("{{\\rtf1\\ansi\\fromhtml1 {{\\*\\htmltag <img src=\"cid:{LOGO_CID}\">}}}}");
    let mut rtf = Vec::new();
    rtf.extend_from_slice(&((rtf_text.len() + 12) as u32).to_le_bytes());
    rtf.extend_from_slice(&(rtf_text.len() as u32).to_le_bytes());
    rtf.extend_from_slice(b"MELA");
    rtf.extend_from_slice(&0u32.to_le_bytes());
    rtf.extend_from_slice(rtf_text.as_bytes());

    // --- Top-level message ---
    let mut props = PropList::new();
    props.fixed(0x0039, PT_SYSTIME, SENT);
    props.fixed(0x0E06, PT_SYSTIME, RECEIVED);
    props.fixed(0x3007, PT_SYSTIME, SENT);
    props.fixed(0x0E07, PT_LONG, 0x10 | 0x1); // has attachments, read
    props.fixed(0x0017, PT_LONG, 2); // high importance
    text(&mut cfb, &mut props, "", 0x001A, "IPM.Note")?;
    text(&mut cfb, &mut props, "", 0x0037, "Q3 report and budget")?;
    text(&mut cfb, &mut props, "", 0x0C1A, "Alex Rivera")?;
    text(&mut cfb, &mut props, "", 0x5D01, "alex.rivera@example.com")?;
    text(&mut cfb, &mut props, "", 0x0E04, "Sam Lee")?;
    text(&mut cfb, &mut props, "", 0x0E03, "Jordan Kim")?;
    text(
        &mut cfb,
        &mut props,
        "",
        0x1035,
        "<CAB12345@mail.example.com>",
    )?;
    text(
        &mut cfb,
        &mut props,
        "",
        0x007D,
        "Received: from mail.example.com\r\nMIME-Version: 1.0\r\n",
    )?;
    text(&mut cfb, &mut props, "", 0x1000, plain)?;
    binary(&mut cfb, &mut props, "", 0x1013, html.as_bytes())?;
    binary(&mut cfb, &mut props, "", 0x1009, &rtf)?;

    // Named-property map: required by Outlook, empty is fine here.
    cfb.create_storage("/__nameid_version1.0")?;
    for stream in ["00020102", "00030102", "00040102"] {
        put(
            &mut cfb,
            &format!("/__nameid_version1.0/__substg1.0_{stream}"),
            &[],
        )?;
    }

    // --- Recipients ---
    let recipients = [
        ("Sam Lee", "sam.lee@example.com", 1),
        ("Jordan Kim", "jordan.kim@example.com", 2),
    ];
    for (i, (name, email, kind)) in recipients.iter().enumerate() {
        let dir = format!("/__recip_version1.0_#{i:08X}");
        cfb.create_storage(&dir)?;
        let mut rp = PropList::new();
        rp.fixed(0x0C15, PT_LONG, *kind);
        rp.fixed(0x3000, PT_LONG, i as u64); // row id
        text(&mut cfb, &mut rp, &dir, 0x3001, name)?;
        text(&mut cfb, &mut rp, &dir, 0x3002, "SMTP")?;
        text(&mut cfb, &mut rp, &dir, 0x3003, email)?;
        text(&mut cfb, &mut rp, &dir, 0x39FE, email)?;
        rp.write(&mut cfb, &dir, &[0u8; 8])?;
    }

    // --- Attachments ---
    let attachments = [
        SampleAttachment {
            name: "logo.png",
            mime: "image/png",
            data: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89".to_vec(),
            content_id: Some(LOGO_CID),
            hidden: true,
        },
        SampleAttachment {
            name: "Q3 Report.pdf",
            mime: "application/pdf",
            data: b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n".to_vec(),
            content_id: None,
            hidden: false,
        },
        SampleAttachment {
            name: "Budget.xlsx",
            mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            data: b"PK\x03\x04\x14\0\0\0\0\0[Content_Types].xml...PK\x03\x04\x14\0\0\0\0\0xl/workbook.xml...".to_vec(),
            content_id: None,
            hidden: false,
        },
        SampleAttachment {
            name: "notes.docx",
            mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            data: b"These are just plain-text notes, not a real Word file.".to_vec(),
            content_id: None,
            hidden: false,
        },
    ];

    for (i, att) in attachments.iter().enumerate() {
        let dir = format!("/__attach_version1.0_#{i:08X}");
        cfb.create_storage(&dir)?;
        let mut ap = PropList::new();
        ap.fixed(0x3705, PT_LONG, 1); // attach by value
        ap.fixed(0x0E21, PT_LONG, i as u64); // attachment number
        ap.fixed(0x0E20, PT_LONG, att.data.len() as u64);
        ap.fixed(0x7FFE, PT_BOOLEAN, u64::from(att.hidden));
        let ext = att
            .name
            .rsplit_once('.')
            .map(|(_, e)| format!(".{e}"))
            .unwrap_or_default();
        text(&mut cfb, &mut ap, &dir, 0x3001, att.name)?;
        text(&mut cfb, &mut ap, &dir, 0x3707, att.name)?;
        text(&mut cfb, &mut ap, &dir, 0x3704, att.name)?;
        text(&mut cfb, &mut ap, &dir, 0x3703, &ext)?;
        text(&mut cfb, &mut ap, &dir, 0x370E, att.mime)?;
        if let Some(cid) = att.content_id {
            text(&mut cfb, &mut ap, &dir, 0x3712, cid)?;
        }
        binary(&mut cfb, &mut ap, &dir, 0x3701, &att.data)?;
        ap.write(&mut cfb, &dir, &[0u8; 8])?;
    }

    // Top-level header: 8 reserved, next recipient id, next attachment id,
    // recipient count, attachment count, 8 reserved.
    let mut header = vec![0u8; 8];
    for count in [
        recipients.len(),
        attachments.len(),
        recipients.len(),
        attachments.len(),
    ] {
        header.extend_from_slice(&(count as u32).to_le_bytes());
    }
    header.extend_from_slice(&[0u8; 8]);
    props.write(&mut cfb, "", &header)?;

    cfb.flush()?;
    Ok(cfb.into_inner().into_inner())
}
