//! The composer's text buffer: insertion, cursor motion, selection, and word edits,
//! all UTF-8-safe and grapheme-aware: an emoji is one user-facing character, so
//! motion and edits never split one in half.

use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Input {
    pub(crate) value: String,
    pub(crate) cursor: usize,
    pub(crate) selection_anchor: Option<usize>,
}

/// The byte offset just before the previous grapheme, so the cursor never
/// steps inside an emoji: a grapheme can be several chars (ZWJ sequences,
/// flags, skin tones), and char-level motion would land mid-emoji where
/// backspace then breaks it in half.
fn previous_grapheme(text: &str, from: usize) -> Option<usize> {
    text[..from]
        .grapheme_indices(true)
        .next_back()
        .map(|(index, _)| index)
}

/// The byte offset just after the next grapheme.
fn next_grapheme(text: &str, from: usize) -> Option<usize> {
    text[from..]
        .graphemes(true)
        .next()
        .map(|grapheme| from + grapheme.len())
}

impl Input {
    pub fn before_cursor(&self) -> &str {
        &self.value[..self.cursor]
    }

    pub fn with_cursor(&self) -> String {
        let mut value = self.value.clone();
        value.insert(self.cursor, '▌');
        value
    }

    pub(crate) fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            None
        } else if anchor < self.cursor {
            Some(anchor..self.cursor)
        } else {
            Some(self.cursor..anchor)
        }
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.value[range])
    }

    pub(crate) fn select_all(&mut self) {
        if !self.value.is_empty() {
            self.selection_anchor = Some(0);
            self.cursor = self.value.len();
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    pub(crate) fn delete_selection(&mut self) -> bool {
        if let Some(range) = self.selection() {
            self.value.replace_range(range.clone(), "");
            self.cursor = range.start;
            self.selection_anchor = None;
            true
        } else {
            self.selection_anchor = None;
            false
        }
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
        self.selection_anchor = None;
    }

    pub(crate) fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
        self.selection_anchor = None;
    }

    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        self.selection_anchor = None;
        std::mem::take(&mut self.value)
    }

    pub(crate) fn insert(&mut self, character: char) {
        self.delete_selection();
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn insert_str(&mut self, value: &str) {
        self.delete_selection();
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let clean = value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        self.value.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        // One whole grapheme goes, not one char: a backspace over 👩‍💻 or 🇺🇸
        // removes the emoji itself instead of leaving a broken half behind.
        if let Some(index) = previous_grapheme(&self.value, self.cursor) {
            self.value.replace_range(index..self.cursor, "");
            self.cursor = index;
        }
    }

    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(end) = next_grapheme(&self.value, self.cursor) {
            self.value.replace_range(self.cursor..end, "");
        }
    }

    pub(crate) fn move_cursor(&mut self, target: usize, select: bool) {
        let target = target.min(self.value.len());
        if select {
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
        } else {
            self.selection_anchor = None;
        }
        self.cursor = target;
    }

    pub(crate) fn left(&mut self, select: bool) {
        let target = previous_grapheme(&self.value, self.cursor).unwrap_or(0);
        self.move_cursor(target, select);
    }

    pub(crate) fn right(&mut self, select: bool) {
        let target = next_grapheme(&self.value, self.cursor).unwrap_or(self.value.len());
        self.move_cursor(target, select);
    }

    pub(crate) fn home(&mut self, select: bool) {
        self.move_cursor(0, select);
    }

    pub(crate) fn end(&mut self, select: bool) {
        self.move_cursor(self.value.len(), select);
    }

    /// Move vertically while preserving the cursor's grapheme column: an
    /// emoji counts as one column regardless of how many chars encode it, so
    /// up and down never land mid-emoji.
    pub(crate) fn move_vertical(&mut self, down: bool, select: bool) {
        let before = &self.value[..self.cursor];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let column = before[line_start..].graphemes(true).count();
        let line_end = self.value[line_start..]
            .find('\n')
            .map_or(self.value.len(), |n| line_start + n);
        let target_start = if down {
            if line_end == self.value.len() {
                return;
            }
            line_end + 1
        } else {
            if line_start == 0 {
                return;
            }
            self.value[..line_start - 1]
                .rfind('\n')
                .map_or(0, |n| n + 1)
        };
        let target_end = self.value[target_start..]
            .find('\n')
            .map_or(self.value.len(), |n| target_start + n);
        let target = self.value[target_start..target_end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset);
        self.move_cursor(target, select);
    }

    pub(crate) fn delete_word(&mut self) {
        if self.delete_selection() {
            return;
        }
        let before = &self.value[..self.cursor];
        let without_space = before.trim_end();
        let start = without_space
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, character)| index + character.len_utf8())
            .unwrap_or(0);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub(crate) fn pos_at_coords(&self, rel_x: usize, rel_y: usize, width: usize) -> usize {
        let width = width.max(1);
        let mut current_row = 0;
        let mut byte_offset = 0;

        // Graphemes, not chars: the click map must match how the composer
        // renders, and one emoji spans several chars but one display column.
        for line in self.value.split('\n') {
            let mut col = 0;
            let mut line_bytes = 0;
            for grapheme in line.graphemes(true) {
                if current_row == rel_y && col >= rel_x {
                    return byte_offset + line_bytes;
                }
                let w = unicode_width::UnicodeWidthStr::width(grapheme);
                if col + w > width {
                    current_row += 1;
                    col = 0;
                    if current_row == rel_y && col >= rel_x {
                        return byte_offset + line_bytes;
                    }
                }
                col += w;
                line_bytes += grapheme.len();
            }
            if current_row == rel_y {
                return byte_offset + line_bytes;
            }
            current_row += 1;
            byte_offset += line_bytes + 1;
        }
        self.value.len()
    }
}

#[cfg(test)]
mod tests {
    use super::Input;

    /// A multi-char emoji is one unit: motion never steps inside it and
    /// backspace never leaves a broken half behind.
    #[test]
    fn motion_and_backspace_stay_whole_graphemes() {
        let mut input = Input::default();
        // 👩‍💻 is one grapheme (ZWJ sequence) and 🇺🇸 is one (flag pair); both
        // move and delete as a unit.
        input.insert_str("a👩‍💻b🇺🇸");
        // A backspace at the end takes the whole flag, not one regional
        // indicator.
        input.backspace();
        assert_eq!(input.value, "a👩‍💻b", "backspace removes the whole flag");
        // Left walks whole graphemes: before 'b', then before the ZWJ emoji
        // — each stop is a boundary, never inside an emoji.
        input.left(false);
        input.left(false);
        assert_eq!(input.selected_text(), None);
        assert_eq!(input.value, "a👩‍💻b", "left over an emoji never splits it");
        assert_eq!(input.cursor, 1, "lefts land on grapheme boundaries");
        input.right(false);
        input.right(false);
        assert_eq!(
            input.cursor,
            input.value.len(),
            "right clears the whole ZWJ emoji"
        );
        input.backspace();
        assert_eq!(input.value, "a👩‍💻", "backspace removes 'b'");
        input.backspace();
        assert_eq!(input.value, "a", "backspace removes the whole ZWJ emoji");
        input.right(false);
        assert_eq!(
            input.cursor,
            input.value.len(),
            "right at the end stays put"
        );
        input.delete();
        assert_eq!(input.value, "a", "delete past the end is a no-op");
    }

    /// Vertical motion lands on grapheme boundaries, not inside an emoji.
    #[test]
    fn vertical_motion_lands_on_grapheme_boundaries() {
        let mut input = Input::default();
        input.insert_str("🦀🦀🦀🦀\nline");
        // The cursor ends before 'n' (column 2 of "line"); up must land after
        // the second crab — a grapheme boundary — not mid-emoji, and down
        // returns to the same column.
        input.left(false);
        input.left(false);
        let before_n = input.cursor;
        input.move_vertical(false, false);
        assert_eq!(input.cursor, 8, "up lands after the second crab");
        input.move_vertical(true, false);
        assert_eq!(input.cursor, before_n, "down restores the grapheme column");
        assert_eq!(input.value, "🦀🦀🦀🦀\nline");
    }

    #[test]
    fn editing_respects_utf8_boundaries() {
        let mut input = Input::default();
        input.insert('é');
        input.insert('🦀');
        input.left(false);
        input.backspace();
        assert_eq!(input.value, "🦀");
        input.right(false);
        input.insert_str("\r\nhello");
        assert_eq!(input.value, "🦀\nhello");
    }

    #[test]
    fn deletes_the_previous_word_and_space() {
        let mut input = Input::default();
        input.insert_str("hello world  ");
        input.delete_word();
        assert_eq!(input.value, "hello ");
        input.delete_word();
        assert_eq!(input.value, "");
    }

    #[test]
    fn selection_and_replacement() {
        let mut input = Input::default();
        input.insert_str("abcdef");
        input.select_all();
        assert_eq!(input.selected_text(), Some("abcdef"));
        input.insert('x');
        assert_eq!(input.value, "x");
        assert_eq!(input.cursor, 1);
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn shift_navigation_selects_range() {
        let mut input = Input::default();
        input.insert_str("hello");
        input.left(true);
        input.left(true);
        assert_eq!(input.selected_text(), Some("lo"));
        input.backspace();
        assert_eq!(input.value, "hel");
        assert_eq!(input.cursor, 3);
    }
}
