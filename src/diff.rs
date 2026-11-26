const CONTEXT_LINES: usize = 3;
const MAX_FILE_LINES: usize = 6_000;
const MAX_MIDDLE_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Context,
    Insert,
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub kind: ChangeKind,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub old_start: usize,
    pub new_start: usize,
    pub old_len: usize,
    pub new_len: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub created: bool,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<Hunk>,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SideRow {
    pub left: Option<DiffLine>,
    pub right: Option<DiffLine>,
}

impl FileDiff {
    pub fn new(path: impl Into<String>, before: &str, after: &str, created: bool) -> Self {
        let path = path.into();
        let before_lines = before.lines().collect::<Vec<_>>();
        let after_lines = after.lines().collect::<Vec<_>>();
        if before_lines.len().saturating_add(after_lines.len()) > MAX_FILE_LINES {
            return Self {
                path,
                created,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
                note: Some(format!(
                    "Diff hidden because the files are too large (limit: {MAX_FILE_LINES} lines)."
                )),
            };
        }

        let entries = line_entries(&before_lines, &after_lines);
        let additions = entries
            .iter()
            .filter(|entry| entry.kind == ChangeKind::Insert)
            .count();
        let deletions = entries
            .iter()
            .filter(|entry| entry.kind == ChangeKind::Delete)
            .count();
        let hunks = build_hunks(&entries);
        Self {
            path,
            created,
            additions,
            deletions,
            hunks,
            note: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    pub fn header(&self, hunk: &Hunk) -> String {
        format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
        )
    }
}

impl Hunk {
    pub fn side_rows(&self) -> Vec<SideRow> {
        let mut rows = Vec::new();
        let mut index = 0;
        while index < self.lines.len() {
            if self.lines[index].kind == ChangeKind::Context {
                let line = self.lines[index].clone();
                rows.push(SideRow {
                    left: Some(line.clone()),
                    right: Some(line),
                });
                index += 1;
                continue;
            }

            let mut deletes = Vec::new();
            let mut inserts = Vec::new();
            while index < self.lines.len() {
                let line = &self.lines[index];
                match line.kind {
                    ChangeKind::Delete => deletes.push(line.clone()),
                    ChangeKind::Insert => inserts.push(line.clone()),
                    ChangeKind::Context => break,
                }
                index += 1;
            }
            for row in 0..deletes.len().max(inserts.len()) {
                rows.push(SideRow {
                    left: deletes.get(row).cloned(),
                    right: inserts.get(row).cloned(),
                });
            }
        }
        rows
    }
}

#[derive(Clone, Copy)]
struct RawEntry<'a> {
    kind: ChangeKind,
    text: &'a str,
}

fn line_entries<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<RawEntry<'a>> {
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }

    let mut before_end = before.len();
    let mut after_end = after.len();
    while before_end > prefix
        && after_end > prefix
        && before[before_end - 1] == after[after_end - 1]
    {
        before_end -= 1;
        after_end -= 1;
    }

    let mut entries = Vec::with_capacity(before.len().max(after.len()));
    for line in &before[..prefix] {
        entries.push(RawEntry {
            kind: ChangeKind::Context,
            text: line,
        });
    }
    let old_middle = &before[prefix..before_end];
    let new_middle = &after[prefix..after_end];
    diff_middle(old_middle, new_middle, &mut entries);
    for line in &before[before_end..] {
        entries.push(RawEntry {
            kind: ChangeKind::Context,
            text: line,
        });
    }
    entries
}

fn diff_middle<'a>(before: &[&'a str], after: &[&'a str], entries: &mut Vec<RawEntry<'a>>) {
    if before.is_empty() {
        for line in after {
            entries.push(RawEntry {
                kind: ChangeKind::Insert,
                text: line,
            });
        }
        return;
    }
    if after.is_empty() {
        for line in before {
            entries.push(RawEntry {
                kind: ChangeKind::Delete,
                text: line,
            });
        }
        return;
    }
    if before.len().saturating_mul(after.len()) > MAX_MIDDLE_CELLS {
        for line in before {
            entries.push(RawEntry {
                kind: ChangeKind::Delete,
                text: line,
            });
        }
        for line in after {
            entries.push(RawEntry {
                kind: ChangeKind::Insert,
                text: line,
            });
        }
        return;
    }

    let width = after.len() + 1;
    let mut table = vec![0_u32; (before.len() + 1) * width];
    for old in (0..before.len()).rev() {
        for new in (0..after.len()).rev() {
            let value = if before[old] == after[new] {
                table[(old + 1) * width + new + 1] + 1
            } else {
                table[(old + 1) * width + new].max(table[old * width + new + 1])
            };
            table[old * width + new] = value;
        }
    }

    let mut old = 0;
    let mut new = 0;
    while old < before.len() && new < after.len() {
        if before[old] == after[new] {
            entries.push(RawEntry {
                kind: ChangeKind::Context,
                text: before[old],
            });
            old += 1;
            new += 1;
        } else if table[(old + 1) * width + new] >= table[old * width + new + 1] {
            entries.push(RawEntry {
                kind: ChangeKind::Delete,
                text: before[old],
            });
            old += 1;
        } else {
            entries.push(RawEntry {
                kind: ChangeKind::Insert,
                text: after[new],
            });
            new += 1;
        }
    }
    while old < before.len() {
        entries.push(RawEntry {
            kind: ChangeKind::Delete,
            text: before[old],
        });
        old += 1;
    }
    while new < after.len() {
        entries.push(RawEntry {
            kind: ChangeKind::Insert,
            text: after[new],
        });
        new += 1;
    }
}

fn build_hunks(raw: &[RawEntry<'_>]) -> Vec<Hunk> {
    let mut old_line = 1;
    let mut new_line = 1;
    let mut lines = Vec::with_capacity(raw.len());
    for entry in raw {
        let (old, new) = match entry.kind {
            ChangeKind::Context => {
                let numbers = (Some(old_line), Some(new_line));
                old_line += 1;
                new_line += 1;
                numbers
            }
            ChangeKind::Insert => {
                let numbers = (None, Some(new_line));
                new_line += 1;
                numbers
            }
            ChangeKind::Delete => {
                let numbers = (Some(old_line), None);
                old_line += 1;
                numbers
            }
        };
        lines.push(DiffLine {
            kind: entry.kind,
            text: entry.text.to_owned(),
            old_line: old,
            new_line: new,
        });
    }

    let changed = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != ChangeKind::Context).then_some(index))
        .collect::<Vec<_>>();
    let mut groups = Vec::new();
    let mut cursor = 0;
    while cursor < changed.len() {
        let start = changed[cursor];
        let mut end = start;
        cursor += 1;
        while cursor < changed.len() && changed[cursor] <= end + CONTEXT_LINES * 2 + 1 {
            end = changed[cursor];
            cursor += 1;
        }
        groups.push((start, end));
    }

    groups
        .into_iter()
        .map(|(start_change, end_change)| {
            let start = start_change.saturating_sub(CONTEXT_LINES);
            let end = end_change
                .saturating_add(CONTEXT_LINES)
                .saturating_add(1)
                .min(lines.len());
            let slice = &lines[start..end];
            let old_before = lines[..start]
                .iter()
                .filter(|line| line.old_line.is_some())
                .count();
            let new_before = lines[..start]
                .iter()
                .filter(|line| line.new_line.is_some())
                .count();
            let old_start = slice[0].old_line.unwrap_or(old_before);
            let new_start = slice[0].new_line.unwrap_or(new_before);
            let old_len = slice.iter().filter(|line| line.old_line.is_some()).count();
            let new_len = slice.iter().filter(|line| line.new_line.is_some()).count();
            Hunk {
                old_start,
                new_start,
                old_len,
                new_len,
                lines: slice.to_vec(),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod extracted_tests;
