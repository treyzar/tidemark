//! Editable text for the island's own input fields (notes, reminders,
//! search). Pure editing logic; drawing and keyboard plumbing live
//! elsewhere. Positions are char indices.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextField {
    pub text: String,
    /// Caret position, in chars.
    pub caret: usize,
    /// Other end of the selection, if any.
    pub anchor: Option<usize>,
    pub multiline: bool,
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

impl TextField {
    pub fn new(multiline: bool) -> Self {
        Self { multiline, ..Default::default() }
    }

    pub fn with_text(text: &str, multiline: bool) -> Self {
        let mut f = Self::new(multiline);
        f.text = text.to_string();
        f.caret = f.len();
        f
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Selected range (start, end) in chars, if non-empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.caret).then(|| (a.min(self.caret), a.max(self.caret)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        Some(self.text.chars().skip(s).take(e - s).collect())
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        let (bs, be) = (byte_at(&self.text, start), byte_at(&self.text, end));
        self.text.replace_range(bs..be, "");
        self.caret = start;
        self.anchor = None;
    }

    /// Removes the selection; true if there was one.
    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((s, e)) => {
                self.delete_range(s, e);
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    pub fn insert(&mut self, s: &str) {
        let s: String = if self.multiline {
            s.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            s.replace(['\r', '\n'], " ")
        };
        let s: String = s.chars().filter(|c| *c == '\n' || !c.is_control()).collect();
        self.delete_selection();
        let b = byte_at(&self.text, self.caret);
        self.text.insert_str(b, &s);
        self.caret += s.chars().count();
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let start = if word { self.word_start(self.caret) } else { self.caret - 1 };
        self.delete_range(start, self.caret);
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() || self.caret >= self.len() {
            return;
        }
        let end = if word { self.word_end(self.caret) } else { self.caret + 1 };
        let caret = self.caret;
        self.delete_range(caret, end);
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn word_start(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from;
        while i > 0 && c[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !c[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_end(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from;
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        while i < c.len() && !c[i].is_whitespace() {
            i += 1;
        }
        i
    }

    fn move_to(&mut self, pos: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = pos.min(self.len());
    }

    pub fn left(&mut self, word: bool, select: bool) {
        if !select {
            if let Some((s, _)) = self.selection() {
                return self.move_to(s, false);
            }
        }
        let to = if word { self.word_start(self.caret) } else { self.caret.saturating_sub(1) };
        self.move_to(to, select);
    }

    pub fn right(&mut self, word: bool, select: bool) {
        if !select {
            if let Some((_, e)) = self.selection() {
                return self.move_to(e, false);
            }
        }
        let to = if word { self.word_end(self.caret) } else { self.caret + 1 };
        self.move_to(to, select);
    }

    /// (line start, line end) around `pos`, in chars.
    fn line_bounds(&self, pos: usize) -> (usize, usize) {
        let c = self.chars();
        let mut s = pos.min(c.len());
        while s > 0 && c[s - 1] != '\n' {
            s -= 1;
        }
        let mut e = pos.min(c.len());
        while e < c.len() && c[e] != '\n' {
            e += 1;
        }
        (s, e)
    }

    pub fn home(&mut self, select: bool) {
        let (s, _) = self.line_bounds(self.caret);
        self.move_to(s, select);
    }

    pub fn end(&mut self, select: bool) {
        let (_, e) = self.line_bounds(self.caret);
        self.move_to(e, select);
    }

    /// Up/down by logical line, keeping the column where possible.
    pub fn vertical(&mut self, down: bool, select: bool) {
        let (s, e) = self.line_bounds(self.caret);
        let col = self.caret - s;
        let target = if down {
            if e >= self.len() {
                self.len()
            } else {
                let (ns, ne) = self.line_bounds(e + 1);
                (ns + col).min(ne)
            }
        } else if s == 0 {
            0
        } else {
            let (ps, pe) = self.line_bounds(s - 1);
            (ps + col).min(pe)
        };
        self.move_to(target, select);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
        self.anchor = None;
    }

    /// Replaces the selection (or all text when nothing is selected) with
    /// `f(part)` and selects the result. False if `f` declines.
    pub fn transform(&mut self, f: impl FnOnce(&str) -> Option<String>) -> bool {
        let (start, end) = self.selection().unwrap_or((0, self.len()));
        let part: String = self.text.chars().skip(start).take(end - start).collect();
        let Some(out) = f(&part) else { return false };
        let (bs, be) = (byte_at(&self.text, start), byte_at(&self.text, end));
        self.text.replace_range(bs..be, &out);
        self.anchor = Some(start);
        self.caret = start + out.chars().count();
        true
    }

    /// Caret position as a UTF-16 index (what DirectWrite hit-testing uses).
    pub fn caret_utf16(&self) -> u32 {
        self.text.chars().take(self.caret).map(char::len_utf16).sum::<usize>() as u32
    }

    /// Selection as UTF-16 (start, length).
    pub fn selection_utf16(&self) -> Option<(u32, u32)> {
        let (s, e) = self.selection()?;
        let start: usize = self.text.chars().take(s).map(char::len_utf16).sum();
        let len: usize = self.text.chars().skip(s).take(e - s).map(char::len_utf16).sum();
        Some((start as u32, len as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_backspace_with_cyrillic() {
        let mut f = TextField::new(false);
        f.insert("Привет");
        f.insert(", мир");
        assert_eq!(f.text, "Привет, мир");
        f.backspace(false);
        assert_eq!(f.text, "Привет, ми");
        f.backspace(true);
        assert_eq!(f.text, "Привет, ");
        assert_eq!(f.caret, 8);
    }

    #[test]
    fn single_line_flattens_newlines() {
        let mut f = TextField::new(false);
        f.insert("a\r\nb\tc");
        assert_eq!(f.text, "a  bc");
        let mut m = TextField::new(true);
        m.insert("a\r\nb");
        assert_eq!(m.text, "a\nb");
    }

    #[test]
    fn selection_replace_and_delete() {
        let mut f = TextField::with_text("hello world", false);
        f.home(false);
        f.right(true, true);
        assert_eq!(f.selected_text().as_deref(), Some("hello"));
        f.insert("bye");
        assert_eq!(f.text, "bye world");
        f.select_all();
        f.delete(false);
        assert!(f.is_empty());
    }

    #[test]
    fn arrows_collapse_selection() {
        let mut f = TextField::with_text("abcdef", false);
        f.select_all();
        f.left(false, false);
        assert_eq!((f.caret, f.anchor), (0, None));
        f.select_all();
        f.right(false, false);
        assert_eq!(f.caret, 6);
    }

    #[test]
    fn vertical_moves_keep_column() {
        let mut f = TextField::with_text("first line\nab\nthird line", true);
        f.caret = 5; // "first|"
        f.vertical(true, false);
        assert_eq!(f.caret, 13); // end of "ab"
        f.vertical(true, false);
        assert_eq!(f.caret, 16); // "th|ird" — column 2 carried from "ab"
        f.vertical(false, false);
        f.vertical(false, false);
        assert_eq!(f.caret, 2);
    }

    #[test]
    fn transform_whole_text_and_selection() {
        let mut f = TextField::with_text("ghbdtn vbh", true);
        assert!(f.transform(|t| Some(crate::launcher::swap_layout(t))));
        assert_eq!(f.text, "привет мир");
        assert_eq!(f.selection(), Some((0, 10)));
        assert!(f.transform(|t| Some(t.to_uppercase())));
        assert_eq!(f.text, "ПРИВЕТ МИР");
        // Only the selected word changes.
        let mut g = TextField::with_text("one two three", false);
        g.anchor = Some(4);
        g.caret = 7;
        assert!(g.transform(|t| Some(t.to_uppercase())));
        assert_eq!(g.text, "one TWO three");
        assert!(!g.transform(|_| None));
        assert_eq!(g.text, "one TWO three");
    }

    #[test]
    fn utf16_positions() {
        let mut f = TextField::with_text("a😀b", false);
        assert_eq!(f.caret_utf16(), 4);
        f.caret = 2;
        assert_eq!(f.caret_utf16(), 3);
        f.anchor = Some(0);
        assert_eq!(f.selection_utf16(), Some((0, 3)));
    }
}
