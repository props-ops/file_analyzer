//! The analysis engine.
//!
//! Everything that *computes* statistics lives here, separate from `main.rs`,
//! which only handles input and output. Keeping the two apart means you can
//! later reuse `analyze` from a GUI, a web server, or a directory walker
//! without touching this file.

/// All the numbers we collect about one piece of text.
///
/// A `struct` groups related values under one name (Rust Book chapter 5).
/// `#[derive(Debug, Default)]` lets us print it with `{:?}` and create an
/// "all zeros" version with `FileStats::default()`.
#[derive(Debug, Default)]
pub struct FileStats {
    pub bytes: usize,
    pub lines: usize,
    pub blank_lines: usize,
    pub words: usize,
    pub chars: usize,
    pub letters: usize,
    pub digits: usize,
    pub whitespace: usize,
    pub punctuation: usize,
    /// (line number, length in chars) of the longest line, 1-based.
    pub longest_line: (usize, usize),
    /// Total number of characters across all words (for the average).
    pub word_chars: usize,
}

impl FileStats {
    /// Average word length in characters. Returns 0.0 for empty files
    /// instead of dividing by zero.
    pub fn average_word_length(&self) -> f64 {
        if self.words == 0 {
            0.0
        } else {
            // `as f64` converts integers to floats; Rust never does this
            // automatically (chapter 3: no implicit type conversion).
            self.word_chars as f64 / self.words as f64
        }
    }

    /// Lines that contain something other than whitespace.
    pub fn non_blank_lines(&self) -> usize {
        self.lines - self.blank_lines
    }
}

/// Analyze a block of text and return its statistics.
///
/// `text: &str` is a *borrowed* string slice: we only read the text, so we
/// don't need to own it (chapter 4).
pub fn analyze(text: &str) -> FileStats {
    let mut stats = FileStats {
        bytes: text.len(), // .len() on a str counts bytes, not characters
        ..FileStats::default()
    };

    // `.lines()` splits on \n (and \r\n), and `.enumerate()` pairs each line
    // with its index, starting at 0.
    for (index, line) in text.lines().enumerate() {
        stats.lines += 1;

        if line.trim().is_empty() {
            stats.blank_lines += 1;
        }

        let line_length = line.chars().count();
        if line_length > stats.longest_line.1 {
            stats.longest_line = (index + 1, line_length);
        }

        for word in line.split_whitespace() {
            stats.words += 1;
            stats.word_chars += word.chars().count();
        }
    }

    // Character classification: one pass over every char.
    // `char` is a Unicode scalar value, so this handles é, ß, 日本 and emoji.
    for c in text.chars() {
        stats.chars += 1;

        if c.is_alphabetic() {
            stats.letters += 1;
        } else if c.is_numeric() {
            stats.digits += 1;
        } else if c.is_whitespace() {
            stats.whitespace += 1;
        } else if c.is_ascii_punctuation() {
            stats.punctuation += 1;
        }
    }

    stats
}

// ---------------------------------------------------------------------------
// Tests: run them with `cargo test`. Add a new test every time you add a
// feature; it's the fastest way to know you didn't break something.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_all_zeros() {
        let stats = analyze("");
        assert_eq!(stats.lines, 0);
        assert_eq!(stats.words, 0);
        assert_eq!(stats.average_word_length(), 0.0);
    }

    #[test]
    fn counts_lines_words_and_blanks() {
        let stats = analyze("hello world\n\n  \nthe end\n");
        assert_eq!(stats.lines, 4);
        assert_eq!(stats.blank_lines, 2);
        assert_eq!(stats.non_blank_lines(), 2);
        assert_eq!(stats.words, 4);
    }

    #[test]
    fn finds_longest_line() {
        let stats = analyze("ab\nabcdef\nabc");
        assert_eq!(stats.longest_line, (2, 6));
    }

    #[test]
    fn classifies_characters() {
        let stats = analyze("Hi 42!");
        assert_eq!(stats.letters, 2);
        assert_eq!(stats.digits, 2);
        assert_eq!(stats.whitespace, 1);
        assert_eq!(stats.punctuation, 1);
    }

    #[test]
    fn counts_unicode_chars_not_bytes() {
        let stats = analyze("café");
        assert_eq!(stats.chars, 4);
        assert_eq!(stats.bytes, 5); // 'é' takes 2 bytes in UTF-8
    }
}
