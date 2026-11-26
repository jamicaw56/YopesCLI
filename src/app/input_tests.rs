#[cfg(test)]
mod tests {
    use super::super::Input;

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
