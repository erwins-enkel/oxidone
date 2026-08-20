//! The editable line behind every text overlay: a buffer and the caret that
//! edits it.
//!
//! Pure — no terminal, no `Model`, no `Overlay` — so the caret's one invariant
//! is stated and tested in one place instead of being re-derived at each of the
//! seven overlay buffers.

use std::ops::Deref;

/// A single-line text buffer with a caret.
///
/// The caret is a **byte offset into `text`, always on a `char` boundary** —
/// every mutator below either lands it on one or leaves it where it was, and
/// `text` is private so nothing outside can put it anywhere else. Byte rather
/// than character offset because that is what `String::insert`/`remove` and
/// slicing take; a character offset would pay for a scan on every keystroke to
/// hand them the same number.
///
/// Motion saturates at both ends rather than wrapping: `←` at column 0 is a
/// visible no-op, which is what a text input does everywhere else.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    text: String,
    caret: usize,
}

impl TextInput {
    /// A buffer holding `text`, with the caret at its end — so an overlay that
    /// opens on a prefill is ready to append, as it was before there was a
    /// caret to place anywhere else.
    pub fn new(text: String) -> Self {
        let caret = text.len();
        Self { text, caret }
    }

    /// The caret's byte offset, for the view: the point it puts the terminal's
    /// cursor, and the only reason anything outside this module needs the number.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The text, consuming the caret with it — what a submitted overlay hands
    /// its `finish_*` function.
    pub fn into_text(self) -> String {
        self.text
    }

    /// Insert `c` at the caret and step past it.
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Delete the character *before* the caret, and follow it back.
    pub fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.text.remove(prev);
            self.caret = prev;
        }
    }

    /// Delete the character *under* the caret, which stays where it is.
    pub fn delete(&mut self) {
        if self.caret < self.text.len() {
            self.text.remove(self.caret);
        }
    }

    /// Move one character left, or stay at 0.
    pub fn left(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.caret = prev;
        }
    }

    /// Move one character right, or stay at the end.
    pub fn right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.caret = next;
        }
    }

    /// Move to the start of the line.
    pub fn home(&mut self) {
        self.caret = 0;
    }

    /// Move to the end of the line.
    pub fn end(&mut self) {
        self.caret = self.text.len();
    }

    /// Empty the line, caret and all.
    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// Delete the word before the caret, leaving the tail after it untouched:
    /// [`kill_word`] applied to the head, then the two halves rejoined. The
    /// caret lands where the head now ends, which is where the killed word
    /// began.
    pub fn kill_word(&mut self) {
        let mut head = self.text[..self.caret].to_string();
        kill_word(&mut head);
        let caret = head.len();
        head.push_str(&self.text[self.caret..]);
        self.text = head;
        self.caret = caret;
    }

    /// The boundary one character behind the caret, or `None` at the start.
    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.caret]
            .chars()
            .next_back()
            .map(|c| self.caret - c.len_utf8())
    }

    /// The boundary one character ahead of the caret, or `None` at the end.
    fn next_boundary(&self) -> Option<usize> {
        self.text[self.caret..]
            .chars()
            .next()
            .map(|c| self.caret + c.len_utf8())
    }
}

/// Reading a `TextInput` reads its text: `trim`, `is_empty`, `chars` and every
/// other `str` method work on it directly, and it coerces to `&str` at a call
/// site that wants one. The caret is reached through [`TextInput::caret`], so
/// the two can never be read from different places.
impl Deref for TextInput {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

/// Comparing against a string literal compares the text — the caret is where
/// the edit is, not part of what was typed.
impl PartialEq<str> for TextInput {
    fn eq(&self, other: &str) -> bool {
        self.text == *other
    }
}

/// Delete the word at the end of `buffer`, readline's `unix-word-rubout`: drop
/// any trailing whitespace, then the run of non-whitespace behind it.
/// `"call Bob re: the invoice"` → `"call Bob re: the "`.
///
/// The whole-buffer form, for the two append-only surfaces that have no caret —
/// the filter input and the Omnibox. [`TextInput::kill_word`] is this same
/// function applied to the text before the caret.
pub fn kill_word(buffer: &mut String) {
    buffer.truncate(buffer.trim_end().len());
    let keep = buffer.rfind(char::is_whitespace).map_or(0, |i| {
        i + buffer[i..].chars().next().map_or(1, char::len_utf8)
    });
    buffer.truncate(keep);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer with the caret placed `chars` characters from the left, the way
    /// a run of `←` from the opening position would leave it.
    fn at(text: &str, chars: usize) -> TextInput {
        let mut input = TextInput::new(text.to_string());
        input.home();
        for _ in 0..chars {
            input.right();
        }
        input
    }

    #[test]
    fn a_new_buffer_opens_with_the_caret_at_the_end() {
        let input = TextInput::new("call Bob".to_string());
        assert_eq!(input.caret(), 8);
        assert_eq!(&*input, "call Bob");
    }

    #[test]
    fn typing_at_the_caret_inserts_rather_than_appends() {
        let mut input = at("invoice", 0);
        for c in "Draft ".chars() {
            input.insert(c);
        }
        assert_eq!(&*input, "Draft invoice");
        assert_eq!(input.caret(), 6, "the caret follows what was typed");
    }

    #[test]
    fn backspace_takes_the_character_behind_the_caret_and_delete_the_one_under_it() {
        let mut input = at("abcd", 2);
        input.backspace();
        assert_eq!(&*input, "acd");
        assert_eq!(input.caret(), 1);

        input.delete();
        assert_eq!(&*input, "ad");
        assert_eq!(input.caret(), 1, "delete leaves the caret where it is");
    }

    #[test]
    fn deleting_at_either_end_is_a_no_op_rather_than_a_panic() {
        let mut start = at("abc", 0);
        start.backspace();
        assert_eq!(&*start, "abc");
        assert_eq!(start.caret(), 0);

        let mut end = TextInput::new("abc".to_string());
        end.delete();
        assert_eq!(&*end, "abc");
        assert_eq!(end.caret(), 3);
    }

    #[test]
    fn motion_saturates_at_both_ends() {
        let mut input = TextInput::new("ab".to_string());
        input.right();
        assert_eq!(input.caret(), 2, "already at the end");
        input.home();
        input.left();
        assert_eq!(input.caret(), 0, "already at the start");
        input.end();
        assert_eq!(input.caret(), 2);
    }

    /// The invariant the type exists to hold: every offset it produces is a
    /// `char` boundary, so no slice or `remove` can split a multi-byte
    /// character. `ü` and `ß` are two bytes each.
    #[test]
    fn the_caret_steps_whole_characters_over_multi_byte_text() {
        let mut input = TextInput::new("Grüße".to_string());
        assert_eq!(input.caret(), 7, "five characters, seven bytes");

        input.left();
        input.left();
        assert_eq!(input.caret(), 4, "back over `e` and `ß`");

        input.insert('s');
        assert_eq!(&*input, "Grüsße");

        input.backspace();
        input.backspace();
        assert_eq!(&*input, "Grße", "the `ü` came off whole");
        assert_eq!(input.caret(), 2);
    }

    #[test]
    fn kill_word_spares_the_tail_after_the_caret() {
        let mut input = at("call Bob re: the invoice", 17);
        input.kill_word();
        assert_eq!(&*input, "call Bob re: invoice");
        assert_eq!(input.caret(), 13, "where the killed word began");
    }

    #[test]
    fn kill_word_on_the_whole_buffer_matches_the_caret_form_at_the_end() {
        let mut buffer = "call Bob re: the invoice".to_string();
        kill_word(&mut buffer);
        assert_eq!(buffer, "call Bob re: the ");

        let mut input = TextInput::new("call Bob re: the invoice".to_string());
        input.kill_word();
        assert_eq!(&*input, buffer);
        assert_eq!(input.caret(), buffer.len());
    }

    #[test]
    fn clearing_takes_the_caret_home_with_the_text() {
        let mut input = TextInput::new("call Bob".to_string());
        input.clear();
        assert!(input.is_empty(), "`str::is_empty`, through `Deref`");
        assert_eq!(input.caret(), 0);
    }
}
