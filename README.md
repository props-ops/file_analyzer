# file_analyzer

[![CI](https://github.com/<your-username>/file_analyzer/actions/workflows/ci.yml/badge.svg)](https://github.com/<your-username>/file_analyzer/actions/workflows/ci.yml)
![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange)
![License: MIT](https://img.shields.io/badge/license-MIT-blue)

A command-line tool that inspects emails and checks their attachments. It
reads Outlook `.msg`, standard `.eml` and `.mbox` mailboxes (including Gmail
Google Takeout exports) and reports:

- **Metadata:** subject, sender, recipients, dates, Message-ID, importance.
- **Every attachment, marked _inline_ or _regular_:** inline means the email
  body displays it, like a signature logo; regular means it's in the
  attachment bar, like a PDF.
- **A file-type check:** each attachment's real bytes are compared with its
  name, so a `.docx` that's actually plain text gets flagged.
- **A verdict:** `PASS` when an email has *both* inline and regular
  attachments.

Any other file gets line, word and character statistics. The format is
detected from the file's content, not its name.

```
$ file_analyzer samples/sample.msg
...
Attachments (4)
======================================
[1] logo.png  (INLINE)
    Content-ID        logo.png@01DA76F1.2C5B8E40
    Type check        OK, PNG image
    Why               body references it as cid:logo.png@01DA76F1.2C5B8E40
[2] Q3 Report.pdf  (REGULAR)
    Type check        OK, PDF document
[3] Budget.xlsx  (REGULAR)
    Type check        OK, Excel workbook
[4] notes.docx  (REGULAR)
    Type check        MISMATCH: named like Word document, but content is unrecognised

Validation
======================================
Inline attachments    1  present
Regular attachments   3  present
Type problems         1
Result                PASS: both inline and regular attachments are present
```

## Install and run

Requires [Rust](https://rustup.rs) 1.88 or newer.

```bash
git clone https://github.com/<your-username>/file_analyzer
cd file_analyzer
cargo build --release

./target/release/file_analyzer samples/sample.msg       # Outlook message
./target/release/file_analyzer samples/sample.eml       # standard email
./target/release/file_analyzer samples/sample.mbox      # one line per message
./target/release/file_analyzer samples/sample.mbox --message 2   # one message in full
./target/release/file_analyzer --make-sample test.eml   # write a test .msg/.eml/.mbox
```

On Windows the binary is `target\release\file_analyzer.exe`. Paths with spaces
need quotes.

| Exit code | Meaning |
| --- | --- |
| 0 | Success. Email: validation passed. Mailbox: at least one message passed |
| 1 | Error: file missing or unreadable, or bad arguments |
| 2 | Read fine but failed validation. Mailbox: no message passed |

For a mailbox the output is a summary table:

```
    #  Date        Inline  Regular  Result  Subject
    1  2024-03-15       1        3  PASS    Q3 report and budget
    2  2024-03-15       0        1  FAIL    Re: Q3 report and budget
    3  2024-03-18       0        0  FAIL    Reminder: Q3 review @ 10am
```

Mailboxes are streamed one message at a time: a 30,000-message mailbox takes
about 0.4 s and 13 MB of memory.

## How it works

Each reader turns its format into the same `EmailInfo` struct, so validation
and printing are written once:

```
                ┌─ msg.rs  ── Outlook Compound File (CFB) + MAPI properties + compressed RTF
file ── sniff ──┼─ eml.rs  ── MIME parsing (mail-parser) + Gmail headers
                ├─ mbox.rs ── streaming mailbox reader → eml.rs per message
                └─ stats.rs ─ text statistics
                       │
                  email.rs ── EmailInfo, inline/regular rules, magic-byte detection
                       │
                  report.rs ─ output
```

- **Inline or regular:** an attachment is inline when the body actually uses
  it. That means the HTML or RTF body references its Content-ID (`cid:...`) or
  Content-Location, Outlook flags it as referenced or hidden, or it's
  `Content-Disposition: inline` with no file name.
- **Type check:** the tool recognises:
  - PDF;
  - Word, Excel and PowerPoint, both modern (ZIP-based) and legacy
    (Compound File);
  - password-protected Office files and ZIP;
  - PNG, JPEG and GIF;
  - attached emails.

## Testing

```bash
cargo test                                  # 20 unit tests + 7 end-to-end CLI tests
cargo clippy --all-targets -- -D warnings
```

CI runs formatting, linting and tests on Linux, Windows and macOS. Beyond the
test suite, the readers were checked against real-world files:

- **`.msg`:** 25 Outlook files from the
  [msg-extractor](https://github.com/TeamMsgExtractor/msg-extractor),
  [MsgReader](https://github.com/Sicos1977/MsgReader) and
  [MAPI](https://github.com/hfig/MAPI) projects. Results matched Python's
  `extract_msg`.
- **`.eml`:** 220 files from the
  [mail-parser](https://github.com/stalwartlabs/mail-parser),
  [SpamScope mail-parser](https://github.com/SpamScope/mail-parser) and
  [nodemailer mailparser](https://github.com/nodemailer/mailparser) test
  suites, many deliberately malformed. No crashes, and attachment results
  matched Python's `email` package on 181. The differences were mostly
  attached emails, which Python unpacks into their parts.

That testing found and fixed several bugs:
- Windows code-page text in old `.msg` files;
- unnamed body parts being counted as attachments;
- header-only MIME detection;
- Gmail Takeout dates.

## Power Automate version

[`power-automate/`](power-automate/) holds the same analysis as an
**Office Script**. It parses `.msg`/`.eml` files inside a Power Automate flow
using only standard Microsoft 365 connectors, with no third-party add-ons and
no Azure. It's tested under the exact TypeScript version Office Scripts uses.

## Project layout

```
src/
  main.rs        arguments, format detection, exit codes
  email.rs       shared types, inline/regular rules, file-type detection
  msg.rs         Outlook .msg reader
  rtf.rs         compressed-RTF decoder ([MS-OXRTFCP])
  eml.rs         .eml reader
  mbox.rs        streaming .mbox reader
  stats.rs       text statistics
  report.rs      output formatting
  sample_msg.rs, sample_eml.rs   generators for the files in samples/
tests/cli.rs     end-to-end tests of the binary
samples/         small test files (regenerate with --make-sample)
power-automate/  Office Script version
```

## Limits

- Attachments inside attached emails aren't checked.
- Rich Text `winmail.dat` packages and encrypted (S/MIME, IRM) messages aren't
  unpacked.
- The sample PDF/XLSX attachments are tiny stand-ins with the right
  signatures. They won't open in Acrobat or Excel.

## Roadmap

- `--extract <folder>` to save attachments to disk
- Analyze every email in a folder in one run
- JSON/CSV output
- Recursive checks of attached emails

## Background

Started as a learning project while working through *The Rust Programming
Language*. The early modules (`stats.rs`, `main.rs`) are commented with the
book chapters each concept comes from.

## License

[MIT](LICENSE)
