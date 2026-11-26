#[cfg(test)]
mod tests {
    use super::super::{ChangeKind, FileDiff};

    #[test]
    fn finds_a_small_change_with_context() {
        let before = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        let after = "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n";
        let diff = FileDiff::new("test.txt", before, after, false);
        assert_eq!(diff.additions, 1);
        assert_eq!(diff.deletions, 1);
        assert_eq!(diff.hunks.len(), 1);
        let hunk = &diff.hunks[0];
        assert_eq!(diff.header(hunk), "@@ -1,6 +1,6 @@");
        assert!(
            hunk.lines
                .iter()
                .any(|line| line.kind == ChangeKind::Delete)
        );
        assert!(
            hunk.lines
                .iter()
                .any(|line| line.kind == ChangeKind::Insert)
        );
    }

    #[test]
    fn a_new_file_is_all_insertions() {
        let diff = FileDiff::new("new.txt", "", "one\ntwo\n", true);
        assert!(diff.created);
        assert_eq!(diff.additions, 2);
        assert_eq!(diff.deletions, 0);
        assert_eq!(diff.header(&diff.hunks[0]), "@@ -0,0 +1,2 @@");
    }

    #[test]
    fn separate_changes_become_separate_hunks() {
        let before = (1..=40)
            .map(|number| number.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let mut after = before.clone();
        after = after.replace("\n2\n", "\nTWO\n");
        after = after.replace("\n39\n", "\nTHIRTY-NINE\n");
        let diff = FileDiff::new("test.txt", &before, &after, false);
        assert_eq!(diff.hunks.len(), 2);
    }

    #[test]
    fn side_by_side_rows_pair_replacements() {
        let diff = FileDiff::new("test.txt", "one\ntwo\nthree\n", "one\nTWO\nthree\n", false);
        let rows = diff.hunks[0].side_rows();
        assert!(rows.iter().any(|row| {
            row.left.as_ref().is_some_and(|line| line.text == "two")
                && row.right.as_ref().is_some_and(|line| line.text == "TWO")
        }));
    }
}
