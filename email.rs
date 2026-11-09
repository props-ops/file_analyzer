//! Types shared by every email format (`.msg`, `.eml`, `.mbox`).
//!
//! Each reader (msg.rs, eml.rs) turns its own file format into the same
//! `EmailInfo` struct, so validation and printing are written only once.

use std::io::Cursor;

use cfb::CompoundFile;

/// Every Compound File Binary file (.msg, .doc, .xls, .ppt) starts with these.
pub const CFB_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Everything we extract from one email.
#[derive(Debug, Default)]
pub struct EmailInfo {
    /// Human-readable format name, e.g. "Outlook message (.msg)".
    pub format: &'static str,
    pub subject: Option<String>,
    pub sender: Option<String>,
    pub recipients: Vec<Recipient>,
    pub sent: Option<String>,
    pub received: Option<String>,
    pub created: Option<String>,
    pub importance: Option<&'static str>,
    pub message_id: Option<String>,
    /// Outlook only, e.g. "IPM.Note".
    pub message_class: Option<String>,
    pub body_formats: Vec<&'static str>,
    pub has_internet_headers: bool,
    /// Outlook only: what Outlook's own "has attachments" flag says.
    pub has_attachments_flag: Option<bool>,
    /// Present when the email came from or through Gmail.
    pub gmail: Option<GmailInfo>,
    pub attachments: Vec<Attachment>,
}

/// Gmail-specific details. Google Takeout adds labels and thread IDs to
/// every message it exports; mail sent through Gmail has Google headers.
#[derive(Debug, Default)]
pub struct GmailInfo {
    pub labels: Vec<String>,
    pub thread_id: Option<String>,
    /// Google's own Received/X-Google-* headers were found.
    pub google_headers: bool,
}

#[derive(Debug)]
pub struct Recipient {
    pub kind: &'static str, // "To", "Cc" or "Bcc"
    pub name: String,
    pub email: Option<String>,
}

/// Where an attachment lives from the reader's point of view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// Shown inside the message body (e.g. a pasted image or a logo).
    Inline,
    /// Shown in the attachment bar, separate from the body.
    Regular,
}

/// Result of checking an attachment's real content against its file name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeCheck {
    /// The bytes match what the extension promises.
    Match,
    /// The extension says one thing, the bytes say another.
    Mismatch,
    /// We don't know what this extension should look like.
    Unverified,
    /// The attachment has no data at all.
    Empty,
}

#[derive(Debug)]
pub struct Attachment {
    pub file_name: String,
    pub mime_type: Option<String>,
    pub size: usize,
    /// "file", "embedded message" or "OLE object".
    pub method: &'static str,
    pub content_id: Option<String>,
    pub placement: Placement,
    /// Why we decided on `placement`, in plain words.
    pub notes: Vec<String>,
    pub expected: Option<FileType>,
    pub detected: FileType,
    pub check: TypeCheck,
}

impl Attachment {
    /// Work out `expected` and `check` from a file name (or, failing that,
    /// a MIME type) and the bytes. `data` is `None` for embedded messages
    /// and OLE objects, which aren't stored as plain bytes.
    pub fn type_check(
        file_name: &str,
        extension: Option<&str>,
        mime_type: Option<&str>,
        data: Option<&[u8]>,
        detected: FileType,
    ) -> (Option<FileType>, TypeCheck) {
        let extension = extension.or_else(|| file_name.rsplit_once('.').map(|(_, ext)| ext));
        let expected = extension
            .and_then(FileType::from_extension)
            .or_else(|| mime_type.and_then(FileType::from_mime));
        let check = match (data, expected) {
            (Some([]), _) => TypeCheck::Empty,
            (_, Some(exp)) if types_agree(exp, detected) => TypeCheck::Match,
            (_, Some(_)) => TypeCheck::Mismatch,
            (_, None) => TypeCheck::Unverified,
        };
        (expected, check)
    }
}

/// Counts used to decide pass/fail.
#[derive(Debug)]
pub struct Validation {
    pub inline: usize,
    pub regular: usize,
    pub type_problems: usize,
}

impl Validation {
    /// Passes when BOTH kinds of attachment are present.
    pub fn passed(&self) -> bool {
        self.inline > 0 && self.regular > 0
    }
}

impl EmailInfo {
    pub fn validation(&self) -> Validation {
        let count = |p: Placement| self.attachments.iter().filter(|a| a.placement == p).count();
        Validation {
            inline: count(Placement::Inline),
            regular: count(Placement::Regular),
            type_problems: self
                .attachments
                .iter()
                .filter(|a| matches!(a.check, TypeCheck::Mismatch | TypeCheck::Empty))
                .count(),
        }
    }
}

// ===========================================================================
// File-type detection ("magic bytes")
// ===========================================================================

/// The kinds of file we can recognise from their contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    Pdf,
    Word,
    Excel,
    PowerPoint,
    /// A password-protected Office file (any of Word/Excel/PowerPoint).
    EncryptedOffice,
    Zip,
    Png,
    Jpeg,
    Gif,
    EmbeddedEmail,
    OleObject,
    Unknown,
}

impl FileType {
    pub fn label(self) -> &'static str {
        match self {
            FileType::Pdf => "PDF document",
            FileType::Word => "Word document",
            FileType::Excel => "Excel workbook",
            FileType::PowerPoint => "PowerPoint presentation",
            FileType::EncryptedOffice => "Encrypted Office document",
            FileType::Zip => "ZIP archive",
            FileType::Png => "PNG image",
            FileType::Jpeg => "JPEG image",
            FileType::Gif => "GIF image",
            FileType::EmbeddedEmail => "email message",
            FileType::OleObject => "OLE object",
            FileType::Unknown => "unrecognised",
        }
    }

    /// What a file with this extension *should* contain.
    pub fn from_extension(ext: &str) -> Option<FileType> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        let file_type = match ext.as_str() {
            "pdf" => FileType::Pdf,
            "doc" | "docx" | "docm" | "dot" | "dotx" | "dotm" => FileType::Word,
            "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "xltm" => FileType::Excel,
            "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" => FileType::PowerPoint,
            "zip" => FileType::Zip,
            "png" => FileType::Png,
            "jpg" | "jpeg" | "jfif" => FileType::Jpeg,
            "gif" => FileType::Gif,
            "msg" | "eml" => FileType::EmbeddedEmail,
            _ => return None,
        };
        Some(file_type)
    }

    /// What a file with this MIME type *should* contain.
    pub fn from_mime(mime: &str) -> Option<FileType> {
        let mime = mime.to_ascii_lowercase();
        let file_type = match mime.as_str() {
            "application/pdf" => FileType::Pdf,
            "application/msword" => FileType::Word,
            "application/vnd.ms-excel" => FileType::Excel,
            "application/vnd.ms-powerpoint" => FileType::PowerPoint,
            "application/zip" | "application/x-zip-compressed" => FileType::Zip,
            "image/png" => FileType::Png,
            "image/jpeg" | "image/jpg" | "image/pjpeg" => FileType::Jpeg,
            "image/gif" => FileType::Gif,
            "message/rfc822" | "application/vnd.ms-outlook" => FileType::EmbeddedEmail,
            m if m.contains("wordprocessingml") => FileType::Word,
            m if m.contains("spreadsheetml") => FileType::Excel,
            m if m.contains("presentationml") => FileType::PowerPoint,
            _ => return None,
        };
        Some(file_type)
    }

    /// Look at the bytes themselves to work out what the file really is.
    pub fn detect(data: &[u8]) -> FileType {
        // PDFs may have a little junk before the header, so search the start.
        if contains(&data[..data.len().min(1024)], b"%PDF-") {
            return FileType::Pdf;
        }
        if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            return FileType::Png;
        }
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return FileType::Jpeg;
        }
        if data.starts_with(b"GIF8") {
            return FileType::Gif;
        }
        if data.starts_with(b"PK\x03\x04") {
            // Modern Office files (.docx, .xlsx, .pptx) are ZIP archives
            // with a [Content_Types].xml entry and a folder per app.
            // File names in a ZIP are stored uncompressed, so we can search.
            if contains(data, b"[Content_Types].xml") {
                if contains(data, b"word/") {
                    return FileType::Word;
                }
                if contains(data, b"xl/") {
                    return FileType::Excel;
                }
                if contains(data, b"ppt/") {
                    return FileType::PowerPoint;
                }
            }
            return FileType::Zip;
        }
        if data.starts_with(&CFB_SIGNATURE) {
            // Legacy Office files (.doc, .xls, .ppt) are CFB containers too;
            // the name of their main stream tells us which app made them.
            if let Ok(inner) = CompoundFile::open(Cursor::new(data)) {
                let has = |name: &str| inner.exists(name);
                if has("/WordDocument") {
                    return FileType::Word;
                }
                if has("/Workbook") || has("/Book") {
                    return FileType::Excel;
                }
                if has("/PowerPoint Document") {
                    return FileType::PowerPoint;
                }
                if has("/EncryptedPackage") {
                    return FileType::EncryptedOffice;
                }
                if has("/__properties_version1.0") {
                    return FileType::EmbeddedEmail;
                }
                return FileType::OleObject;
            }
        }
        FileType::Unknown
    }
}

/// Does `expected` (from the name) agree with `detected` (from the bytes)?
fn types_agree(expected: FileType, detected: FileType) -> bool {
    let is_office = matches!(
        expected,
        FileType::Word | FileType::Excel | FileType::PowerPoint
    );
    expected == detected || (is_office && detected == FileType::EncryptedOffice)
}

/// True if `needle` appears anywhere in `haystack`.
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Seconds since 1970-01-01 UTC -> "YYYY-MM-DD HH:MM:SS UTC".
pub fn unix_to_string(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_file_types_from_bytes() {
        assert_eq!(FileType::detect(b"%PDF-1.7\n..."), FileType::Pdf);
        assert_eq!(FileType::detect(b"\x89PNG\r\n\x1a\n"), FileType::Png);
        assert_eq!(
            FileType::detect(b"PK\x03\x04....[Content_Types].xml....word/document.xml"),
            FileType::Word
        );
        assert_eq!(FileType::detect(b"PK\x03\x04 just a zip"), FileType::Zip);
        assert_eq!(FileType::detect(b"hello"), FileType::Unknown);
    }

    #[test]
    fn type_check_flags_mismatch_and_empty() {
        let (_, check) =
            Attachment::type_check("a.pdf", None, None, Some(b"%PDF-1.4"), FileType::Pdf);
        assert_eq!(check, TypeCheck::Match);
        let (_, check) =
            Attachment::type_check("a.pdf", None, None, Some(b"hi"), FileType::Unknown);
        assert_eq!(check, TypeCheck::Mismatch);
        let (_, check) = Attachment::type_check("a.pdf", None, None, Some(b""), FileType::Unknown);
        assert_eq!(check, TypeCheck::Empty);
        let (_, check) =
            Attachment::type_check("a.txt", None, None, Some(b"hi"), FileType::Unknown);
        assert_eq!(check, TypeCheck::Unverified);
        // No file name: fall back to the MIME type.
        let (exp, check) = Attachment::type_check(
            "(unnamed)",
            None,
            Some("image/png"),
            Some(b"\x89PNG"),
            FileType::Png,
        );
        assert_eq!((exp, check), (Some(FileType::Png), TypeCheck::Match));
    }

    #[test]
    fn formats_unix_time() {
        assert_eq!(unix_to_string(1_710_513_000), "2024-03-15 14:30:00 UTC");
    }
}
