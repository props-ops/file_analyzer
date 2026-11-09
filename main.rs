//! file_analyzer: inspect a file and report on it.
//!
//! Usage:
//!     cargo run -- samples/sample.txt            text statistics
//!     cargo run -- samples/sample.msg            Outlook message
//!     cargo run -- email.eml                     standard email (incl. Gmail "Download message")
//!     cargo run -- "All mail.mbox"               mailbox, e.g. Gmail via Google Takeout
//!     cargo run -- "All mail.mbox" --message 3   one message from a mailbox, in full
//!     cargo run -- --make-sample sample.eml      write a test file (.msg, .eml or .mbox)
//!     cargo run                                  asks you for a path
//!
//! Exit codes: 0 = OK, 1 = error, 2 = attachment validation failed
//! (for a whole mailbox: no message passed).

// Each `mod` line includes a file from src/ as a module (Rust Book chapter 7).
mod email;
mod eml;
mod mbox;
mod msg;
mod report;
mod rtf;
mod sample_eml;
mod sample_msg;
mod stats;

use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::process;

use email::{CFB_SIGNATURE, EmailInfo};

/// How much of the file we read to decide what kind of file it is.
const SNIFF_BYTES: u64 = 64 * 1024;

/// What the user asked for on the command line.
struct Options {
    path: Option<String>,
    message: Option<usize>,
    make_sample: Option<String>,
}

fn main() {
    let options = parse_args();

    if let Some(out) = &options.make_sample {
        make_sample(out);
        return;
    }

    let path = options.path.clone().unwrap_or_else(ask_for_path);

    // Look at the start of the file to decide what it is. This is by
    // *content*, not file name, so a renamed file is still read correctly.
    let head =
        read_head(&path).unwrap_or_else(|error| fail(&format!("could not read '{path}': {error}")));

    if head.starts_with(&CFB_SIGNATURE) {
        let bytes = read_all(&path);
        if msg::is_msg(&bytes) {
            let info = msg::analyze(&bytes).unwrap_or_else(|e| fail(&e));
            finish_email(&path, &info);
        }
        // Some other CFB file (.doc, .xls ...): fall through to text stats.
        print_text(&path, &bytes);
    } else if mbox::looks_like_mbox(&head) {
        analyze_mailbox(&path, options.message);
    } else if eml::looks_like_eml(&head) {
        let bytes = read_all(&path);
        let info = eml::analyze(&bytes, "Email message (.eml)").unwrap_or_else(|e| fail(&e));
        finish_email(&path, &info);
    } else {
        print_text(&path, &read_all(&path));
    }
}

/// Print an email report and exit with its validation result.
fn finish_email(title: &str, info: &EmailInfo) -> ! {
    report::print_email_report(title, info);
    process::exit(if info.validation().passed() { 0 } else { 2 });
}

fn analyze_mailbox(path: &str, message: Option<usize>) {
    let mailbox = mbox::analyze_file(path, message)
        .unwrap_or_else(|e| fail(&format!("could not read '{path}': {e}")));

    if let Some(number) = message {
        match &mailbox.detail {
            Some(Ok(info)) => finish_email(&format!("{path}, message {number}"), info),
            Some(Err(error)) => fail(error),
            None => fail(&format!(
                "there is no message {number}; this mailbox has {}",
                mailbox.messages.len()
            )),
        }
    }

    report::print_mbox_summary(path, &mailbox);
    let any_passed = mailbox
        .messages
        .iter()
        .any(|m| m.as_ref().is_ok_and(|s| s.passed()));
    if !any_passed {
        process::exit(2);
    }
}

fn print_text(path: &str, bytes: &[u8]) {
    // `from_utf8_lossy` replaces invalid UTF-8 instead of failing, so
    // binary files won't crash the program.
    let text = String::from_utf8_lossy(bytes);
    report::print_text_report(path, &stats::analyze(&text));
}

fn parse_args() -> Options {
    let mut options = Options {
        path: None,
        message: None,
        make_sample: None,
    };
    let mut args = env::args().skip(1); // skip the program's own name

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--make-sample" => {
                options.make_sample = Some(args.next().unwrap_or_else(|| "sample.msg".to_string()));
            }
            "--message" => {
                let value = args.next().unwrap_or_default();
                match value.parse::<usize>() {
                    Ok(n) if n > 0 => options.message = Some(n),
                    _ => fail("--message needs a message number, e.g. --message 3"),
                }
            }
            "-h" | "--help" => {
                println!(
                    "Usage: file_analyzer [FILE] [--message N] | --make-sample FILE.(msg|eml|mbox)"
                );
                process::exit(0);
            }
            _ => options.path = Some(arg),
        }
    }
    options
}

fn make_sample(out: &str) {
    let lower = out.to_ascii_lowercase();
    let result = if lower.ends_with(".eml") {
        fs::write(out, sample_eml::build_eml())
    } else if lower.ends_with(".mbox") {
        fs::write(out, sample_eml::build_mbox())
    } else {
        sample_msg::build().and_then(|bytes| fs::write(out, bytes))
    };
    match result {
        Ok(()) => println!("Wrote sample to {out}"),
        Err(error) => fail(&format!("could not write '{out}': {error}")),
    }
}

/// Read just the first few kilobytes of a file.
fn read_head(path: &str) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    File::open(path)?.take(SNIFF_BYTES).read_to_end(&mut head)?;
    Ok(head)
}

fn read_all(path: &str) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| fail(&format!("could not read '{path}': {error}")))
}

/// Print an error and stop. The `!` return type means "never returns".
fn fail(message: &str) -> ! {
    eprintln!("Error: {message}");
    process::exit(1);
}

/// Prompt until the user types a path, just like the guessing game.
fn ask_for_path() -> String {
    loop {
        println!("Enter the path of a file to analyze:");

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read line");

        // Shadowing: replace the String with a trimmed copy of itself.
        let input = input.trim().to_string();

        if input.is_empty() {
            println!("Please type a path.");
            continue;
        }

        break input; // `break` with a value returns it from the loop
    }
}
