//! A one-line text box's text and caret, for the launcher's search box.

/// Text with a caret, edited like a readline prompt. The caret is a byte offset, always on a
/// character boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    text: String,
    caret: usize,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text before the caret.
    pub fn before_caret(&self) -> &str {
        &self.text[..self.caret]
    }

    /// Replaces the text, with the caret at the end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.len();
    }

    pub fn clear(&mut self) {
        self.set(String::new());
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    fn prev(&self, at: usize) -> Option<usize> {
        self.text[..at].char_indices().next_back().map(|(i, _)| i)
    }

    fn next(&self, at: usize) -> Option<usize> {
        self.text[at..].chars().next().map(|c| at + c.len_utf8())
    }

    /// Where the word before `at` starts: back over spaces and punctuation, then the word.
    fn word_start(&self, at: usize) -> usize {
        let mut i = at;
        while let Some(p) = self
            .prev(i)
            .filter(|&p| !self.text[p..].starts_with(is_word))
        {
            i = p;
        }
        while let Some(p) = self
            .prev(i)
            .filter(|&p| self.text[p..].starts_with(is_word))
        {
            i = p;
        }
        i
    }

    /// Where the word after `at` ends: over spaces and punctuation, then the word.
    fn word_end(&self, at: usize) -> usize {
        let mut i = at;
        while !self.text[i..].starts_with(is_word) {
            match self.next(i) {
                Some(n) => i = n,
                None => return i,
            }
        }
        while self.text[i..].starts_with(is_word) {
            match self.next(i) {
                Some(n) => i = n,
                None => return i,
            }
        }
        i
    }

    /// Moves the caret a character left. Returns whether it moved.
    pub fn left(&mut self) -> bool {
        self.prev(self.caret).map(|p| self.caret = p).is_some()
    }

    pub fn right(&mut self) -> bool {
        self.next(self.caret).map(|n| self.caret = n).is_some()
    }

    pub fn word_left(&mut self) -> bool {
        let to = self.word_start(self.caret);
        let moved = to != self.caret;
        self.caret = to;
        moved
    }

    pub fn word_right(&mut self) -> bool {
        let to = self.word_end(self.caret);
        let moved = to != self.caret;
        self.caret = to;
        moved
    }

    pub fn home(&mut self) -> bool {
        let moved = self.caret != 0;
        self.caret = 0;
        moved
    }

    pub fn end(&mut self) -> bool {
        let moved = self.caret != self.text.len();
        self.caret = self.text.len();
        moved
    }

    /// Deletes the character before the caret. Returns whether the text changed.
    pub fn backspace(&mut self) -> bool {
        let Some(p) = self.prev(self.caret) else {
            return false;
        };
        self.text.replace_range(p..self.caret, "");
        self.caret = p;
        true
    }

    /// Deletes the character after the caret.
    pub fn delete(&mut self) -> bool {
        let Some(n) = self.next(self.caret) else {
            return false;
        };
        self.text.replace_range(self.caret..n, "");
        true
    }

    /// Deletes the word before the caret.
    pub fn delete_word(&mut self) -> bool {
        let start = self.word_start(self.caret);
        self.text.replace_range(start..self.caret, "");
        let changed = start != self.caret;
        self.caret = start;
        changed
    }

    /// Deletes everything before the caret.
    pub fn delete_to_start(&mut self) -> bool {
        let changed = self.caret != 0;
        self.text.replace_range(..self.caret, "");
        self.caret = 0;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text with `|` at the caret.
    fn show(input: &Input) -> String {
        format!(
            "{}|{}",
            input.before_caret(),
            &input.text()[input.before_caret().len()..]
        )
    }

    fn input(text: &str) -> Input {
        let mut input = Input::default();
        input.set(text);
        input
    }

    #[test]
    fn types_and_moves_by_character() {
        let mut i = input("ac");
        assert!(i.left());
        i.insert('b');
        assert_eq!(show(&i), "ab|c");
        assert!(i.right());
        assert!(!i.right());
        assert!(i.home());
        assert!(!i.left());
        i.insert('>');
        assert_eq!(show(&i), ">|abc");
        assert!(i.end());
        assert_eq!(show(&i), ">abc|");
    }

    #[test]
    fn handles_characters_wider_than_a_byte() {
        let mut i = input("aé😀");
        assert!(i.left());
        assert_eq!(show(&i), "aé|😀");
        assert!(i.backspace());
        assert_eq!(show(&i), "a|😀");
        assert!(i.delete());
        assert_eq!(show(&i), "a|");
    }

    #[test]
    fn moves_and_deletes_by_word() {
        let mut i = input("!movefocus left  ");
        assert!(i.word_left());
        assert_eq!(show(&i), "!movefocus |left  ");
        assert!(i.word_left());
        assert_eq!(show(&i), "!|movefocus left  ");
        assert!(i.word_left());
        assert_eq!(show(&i), "|!movefocus left  ");
        assert!(!i.word_left());
        assert!(i.word_right());
        assert_eq!(show(&i), "!movefocus| left  ");
        assert!(i.word_right());
        assert!(i.word_right());
        assert_eq!(show(&i), "!movefocus left  |");

        let mut i = input("exec wt.exe");
        assert!(i.delete_word());
        assert_eq!(show(&i), "exec wt.|");
        assert!(i.delete_word());
        assert_eq!(show(&i), "exec |");
        assert!(i.delete_word());
        assert_eq!(show(&i), "|");
        assert!(!i.delete_word());
    }

    #[test]
    fn deletes_around_the_caret() {
        let mut i = input("hello world");
        for _ in 0..5 {
            i.left();
        }
        assert!(i.delete());
        assert_eq!(show(&i), "hello |orld");
        assert!(i.delete_to_start());
        assert_eq!(show(&i), "|orld");
        assert!(!i.backspace());
        assert!(!i.delete_to_start());
    }
}
