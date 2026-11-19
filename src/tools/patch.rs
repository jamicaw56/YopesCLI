//! `apply_patch`: one envelope that adds, deletes, updates and moves files.
//!
//! The format is the `*** Begin Patch` envelope many models are trained on:
//!
//! ```text
//! *** Begin Patch
//! *** Add File: src/new.rs
//! +fn main() {}
//! *** Update File: src/lib.rs
//! *** Move to: src/core.rs
//! @@ fn old_name
//! -fn old_name() {}
//! +fn new_name() {}
//! *** Delete File: src/dead.rs
//! *** End Patch
//! ```
//!
//! Every hunk is matched against in-memory copies first: a patch that does
//! not apply cleanly touches no file.

use std::{
    collections::HashMap,
    fs,
    sync::{Mutex, OnceLock},
};

use super::*;

#[derive(Deserialize)]
struct ApplyPatchArgs {
    patch: String,
}

#[derive(Debug, PartialEq)]
enum FileOp {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

#[derive(Debug, Default, PartialEq)]
struct Hunk {
    /// Text after `@@`: a line to seek before matching, when present.
    anchor: Option<String>,
    old: Vec<String>,
    new: Vec<String>,
    /// `*** End of File` followed the hunk: it must match at the file's end.
    at_eof: bool,
}

/// One planned file change: `before` is `None` for a new file and `after` is
/// `None` for a deletion.
struct Planned {
    path: String,
    before: Option<String>,
    after: Option<String>,
}

const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

/// Diffs of finished patches, keyed by call id, picked up once by
/// `applied_diffs` for the transcript. The tool runs on the blocking pool,
/// so the hand-off is a process-wide map.
fn applied() -> &'static Mutex<HashMap<String, Vec<FileDiff>>> {
    static APPLIED: OnceLock<Mutex<HashMap<String, Vec<FileDiff>>>> = OnceLock::new();
    APPLIED.get_or_init(Default::default)
}

pub(super) fn apply_patch(call_id: &str, arguments: &str) -> Result<String> {
    let plan = plan(arguments)?;
    for change in &plan {
        let target = resolve(&change.path);
        match &change.after {
            Some(after) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("creating the folder for {}", change.path))?;
                }
                fs::write(&target, after.as_bytes())
                    .with_context(|| format!("writing {}", change.path))?;
            }
            None => {
                fs::remove_file(&target).with_context(|| format!("deleting {}", change.path))?
            }
        }
    }
    let summary = summarize(&plan);
    applied()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(call_id.to_owned(), diffs(&plan));
    Ok(summary)
}

/// The diffs a finished `apply_patch` call made, taken once.
pub(super) fn applied_diffs(call_id: &str) -> Vec<FileDiff> {
    applied()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(call_id)
        .unwrap_or_default()
}

pub(super) fn apply_patch_preview(arguments: &str) -> ChangePreview {
    match plan(arguments) {
        Ok(plan) => ChangePreview {
            diffs: diffs(&plan),
            before: None,
            note: None,
        },
        Err(error) => super::preview::preview_error(error),
    }
}

/// Every path a patch names, for the approval policy.
pub(super) fn patch_paths(arguments: &str) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<ApplyPatchArgs>(arguments) else {
        return Vec::new();
    };
    let Ok(ops) = parse_patch(&args.patch) else {
        return Vec::new();
    };
    ops.into_iter()
        .flat_map(|op| match op {
            FileOp::Add { path, .. } | FileOp::Delete { path } => vec![path],
            FileOp::Update { path, move_to, .. } => std::iter::once(path).chain(move_to).collect(),
        })
        .collect()
}

/// A short count line, also used by the transcript preview.
pub(super) fn patch_summary(arguments: &str) -> String {
    let Ok(args) = serde_json::from_str::<ApplyPatchArgs>(arguments) else {
        return "patch".to_owned();
    };
    match parse_patch(&args.patch) {
        Ok(ops) if ops.len() == 1 => match &ops[0] {
            FileOp::Add { path, .. } => format!("add {path}"),
            FileOp::Delete { path } => format!("delete {path}"),
            FileOp::Update {
                path,
                move_to: Some(to),
                ..
            } => format!("{path} → {to}"),
            FileOp::Update { path, .. } => path.clone(),
        },
        Ok(ops) => format!("{} files", ops.len()),
        Err(_) => "patch".to_owned(),
    }
}

fn plan(arguments: &str) -> Result<Vec<Planned>> {
    let args: ApplyPatchArgs = parse(arguments)?;
    if args.patch.len() > MAX_PATCH_BYTES {
        bail!("patch is too large (the limit is {MAX_PATCH_BYTES} bytes)");
    }
    let ops = parse_patch(&args.patch)?;
    // Later operations on the same path see earlier ones.
    let mut state: HashMap<String, Option<String>> = HashMap::new();
    let mut originals: HashMap<String, Option<String>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut current = |path: &str,
                       state: &mut HashMap<String, Option<String>>,
                       order: &mut Vec<String>|
     -> Result<Option<String>> {
        if let Some(text) = state.get(path) {
            return Ok(text.clone());
        }
        let resolved = resolve(path);
        let text = if resolved.exists() {
            if !resolved.is_file() {
                bail!("{path} is not a file");
            }
            Some(fs::read_to_string(&resolved).with_context(|| format!("reading {path}"))?)
        } else {
            None
        };
        originals.insert(path.to_owned(), text.clone());
        state.insert(path.to_owned(), text.clone());
        order.push(path.to_owned());
        Ok(text)
    };
    for op in ops {
        match op {
            FileOp::Add { path, content } => {
                if current(&path, &mut state, &mut order)?.is_some() {
                    bail!("{path} already exists; use *** Update File to change it");
                }
                state.insert(path, Some(content));
            }
            FileOp::Delete { path } => {
                if current(&path, &mut state, &mut order)?.is_none() {
                    bail!("{path} does not exist, so it cannot be deleted");
                }
                state.insert(path, None);
            }
            FileOp::Update {
                path,
                move_to,
                hunks,
            } => {
                let Some(source) = current(&path, &mut state, &mut order)? else {
                    bail!("{path} does not exist; use *** Add File to create it");
                };
                let updated = apply_hunks(&source, &hunks).with_context(|| format!("in {path}"))?;
                match move_to.filter(|to| *to != path) {
                    Some(to) => {
                        if current(&to, &mut state, &mut order)?.is_some() {
                            bail!("cannot move {path} to {to}: {to} already exists");
                        }
                        state.insert(path, None);
                        state.insert(to, Some(updated));
                    }
                    None => {
                        state.insert(path, Some(updated));
                    }
                }
            }
        }
    }
    let mut plan = Vec::new();
    for path in order {
        let before = originals.remove(&path).flatten();
        let after = state.remove(&path).flatten();
        if before != after {
            plan.push(Planned {
                path,
                before,
                after,
            });
        }
    }
    if plan.is_empty() {
        bail!("the patch changes nothing");
    }
    Ok(plan)
}

fn diffs(plan: &[Planned]) -> Vec<FileDiff> {
    plan.iter()
        .map(|change| {
            FileDiff::new(
                change.path.as_str(),
                change.before.as_deref().unwrap_or_default(),
                change.after.as_deref().unwrap_or_default(),
                change.before.is_none(),
            )
        })
        .collect()
}

fn summarize(plan: &[Planned]) -> String {
    let count = |pick: fn(&Planned) -> bool| plan.iter().filter(|change| pick(change)).count();
    let added = count(|change| change.before.is_none());
    let deleted = count(|change| change.after.is_none());
    let updated = plan.len() - added - deleted;
    let mut parts = Vec::new();
    for (n, verb) in [(added, "added"), (updated, "updated"), (deleted, "deleted")] {
        if n > 0 {
            parts.push(format!("{n} {verb}"));
        }
    }
    let paths = plan
        .iter()
        .map(|change| change.path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!("Patch applied: {} ({paths}).", parts.join(", "))
}

fn parse_patch(patch: &str) -> Result<Vec<FileOp>> {
    let lines: Vec<&str> = patch
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .context("the patch must start with `*** Begin Patch`")?;
    let end = lines
        .iter()
        .rposition(|line| line.trim() == "*** End Patch")
        .context("the patch must end with `*** End Patch`")?;
    if end <= start {
        bail!("`*** End Patch` comes before `*** Begin Patch`");
    }
    let body = &lines[start + 1..end];
    let mut ops = Vec::new();
    let mut index = 0;
    while index < body.len() {
        let line = body[index];
        index += 1;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(path) = header(line, "*** Add File:")? {
            let mut content = String::new();
            while index < body.len() && !body[index].starts_with("*** ") {
                let Some(text) = body[index].strip_prefix('+') else {
                    bail!(
                        "line {:?} in added file {path} must start with `+`",
                        body[index]
                    );
                };
                content.push_str(text);
                content.push('\n');
                index += 1;
            }
            ops.push(FileOp::Add { path, content });
        } else if let Some(path) = header(line, "*** Delete File:")? {
            ops.push(FileOp::Delete { path });
        } else if let Some(path) = header(line, "*** Update File:")? {
            let mut move_to = None;
            if index < body.len() {
                if let Some(to) = header(body[index], "*** Move to:")? {
                    move_to = Some(to);
                    index += 1;
                }
            }
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut hunk = Hunk::default();
            while index < body.len() {
                let line = body[index];
                if line.trim() == "*** End of File" {
                    hunk.at_eof = true;
                    index += 1;
                    continue;
                }
                if line.starts_with("*** ") {
                    break;
                }
                index += 1;
                if let Some(anchor) = line.strip_prefix("@@") {
                    if !hunk.old.is_empty() || !hunk.new.is_empty() {
                        hunks.push(std::mem::take(&mut hunk));
                    }
                    let anchor = anchor.trim();
                    hunk.anchor = (!anchor.is_empty()).then(|| anchor.to_owned());
                    continue;
                }
                match line.chars().next() {
                    Some('+') => hunk.new.push(line[1..].to_owned()),
                    Some('-') => hunk.old.push(line[1..].to_owned()),
                    Some(' ') => {
                        hunk.old.push(line[1..].to_owned());
                        hunk.new.push(line[1..].to_owned());
                    }
                    // A blank context line often loses its leading space.
                    None => {
                        hunk.old.push(String::new());
                        hunk.new.push(String::new());
                    }
                    Some(_) => {
                        bail!("line {line:?} in {path} must start with ` `, `-`, `+` or `@@`")
                    }
                }
            }
            if !hunk.old.is_empty() || !hunk.new.is_empty() {
                hunks.push(hunk);
            }
            if hunks.is_empty() && move_to.is_none() {
                bail!("*** Update File: {path} has no hunks");
            }
            ops.push(FileOp::Update {
                path,
                move_to,
                hunks,
            });
        } else {
            bail!("unexpected line {line:?}; expected a `*** Add/Update/Delete File:` header");
        }
    }
    if ops.is_empty() {
        bail!("the patch has no file operations");
    }
    Ok(ops)
}

fn header(line: &str, prefix: &str) -> Result<Option<String>> {
    let Some(rest) = line.strip_prefix(prefix) else {
        return Ok(None);
    };
    let path = rest.trim();
    if path.is_empty() {
        bail!("`{prefix}` needs a path");
    }
    Ok(Some(path.to_owned()))
}

/// Apply hunks in order; each one searches from where the previous ended.
fn apply_hunks(source: &str, hunks: &[Hunk]) -> Result<String> {
    let crlf = source.contains("\r\n");
    let trailing_newline = source.is_empty() || source.ends_with('\n');
    let mut lines: Vec<String> = source
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect();
    let mut cursor = 0;
    for (number, hunk) in hunks.iter().enumerate() {
        let mut from = cursor;
        if let Some(anchor) = &hunk.anchor {
            from = find_line(&lines, anchor, from)
                .with_context(|| format!("hunk {}: anchor {anchor:?} not found", number + 1))?
                + 1;
        }
        let at = if hunk.old.is_empty() {
            // A pure insertion lands after the anchor, or at the end.
            if hunk.anchor.is_some() && !hunk.at_eof {
                from
            } else {
                lines.len()
            }
        } else {
            find_block(&lines, &hunk.old, from, hunk.at_eof).with_context(|| {
                format!(
                    "hunk {}: these lines were not found:\n{}",
                    number + 1,
                    clip(&hunk.old.join("\n"), 600)
                )
            })?
        };
        lines.splice(at..at + hunk.old.len(), hunk.new.iter().cloned());
        cursor = at + hunk.new.len();
    }
    let ending = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(ending);
    if trailing_newline && !lines.is_empty() {
        out.push_str(ending);
    }
    Ok(out)
}

fn find_line(lines: &[String], needle: &str, from: usize) -> Option<usize> {
    let needle = needle.trim();
    (from..lines.len())
        .find(|&index| lines[index].trim() == needle)
        .or_else(|| (from..lines.len()).find(|&index| lines[index].contains(needle)))
}

/// Find `block` at or after `from`: exact first, then ignoring trailing
/// whitespace, then ignoring all surrounding whitespace.
fn find_block(lines: &[String], block: &[String], from: usize, at_eof: bool) -> Option<usize> {
    if block.len() > lines.len() {
        return None;
    }
    let last = lines.len() - block.len();
    let starts: Vec<usize> = if at_eof {
        vec![last]
    } else {
        (from.min(last + 1)..=last).collect()
    };
    let passes: [fn(&str, &str) -> bool; 3] = [
        |a, b| a == b,
        |a, b| a.trim_end() == b.trim_end(),
        |a, b| a.trim() == b.trim(),
    ];
    passes.iter().find_map(|same| {
        starts.iter().copied().find(|&start| {
            block
                .iter()
                .zip(&lines[start..])
                .all(|(want, have)| same(want, have))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old: &[&str], new: &[&str]) -> Hunk {
        Hunk {
            old: old.iter().map(|s| s.to_string()).collect(),
            new: new.iter().map(|s| s.to_string()).collect(),
            ..Hunk::default()
        }
    }

    #[test]
    fn parses_every_operation() {
        let ops = parse_patch(
            "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** Update File: b.txt\n*** Move to: c.txt\n@@ fn x\n-old\n+new\n ctx\n*** Delete File: d.txt\n*** End Patch",
        )
        .unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(
            ops[0],
            FileOp::Add {
                path: "a.txt".into(),
                content: "hi\n".into()
            }
        );
        let FileOp::Update { move_to, hunks, .. } = &ops[1] else {
            panic!("expected update");
        };
        assert_eq!(move_to.as_deref(), Some("c.txt"));
        assert_eq!(hunks[0].anchor.as_deref(), Some("fn x"));
        assert_eq!(hunks[0].old, vec!["old", "ctx"]);
        assert_eq!(hunks[0].new, vec!["new", "ctx"]);
    }

    #[test]
    fn rejects_missing_envelope() {
        assert!(parse_patch("*** Add File: a\n+x").is_err());
    }

    #[test]
    fn hunks_keep_crlf_and_tolerate_whitespace() {
        let source = "a\r\nb  \r\nc\r\n";
        let out = apply_hunks(source, &[hunk(&["b", "c"], &["B", "c"])]).unwrap();
        assert_eq!(out, "a\r\nB\r\nc\r\n");
    }

    #[test]
    fn hunk_that_does_not_match_fails() {
        assert!(apply_hunks("a\nb\n", &[hunk(&["zzz"], &["y"])]).is_err());
    }

    #[test]
    fn anchor_picks_the_right_duplicate() {
        let source = "fn a\n  x\nfn b\n  x\n";
        let mut h = hunk(&["  x"], &["  y"]);
        h.anchor = Some("fn b".into());
        assert_eq!(apply_hunks(source, &[h]).unwrap(), "fn a\n  x\nfn b\n  y\n");
    }

    #[test]
    fn applies_and_refuses_whole_patches_on_disk() {
        let dir = std::env::temp_dir().join(format!("yopes-patch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("keep.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        let p = |name: &str| dir.join(name).display().to_string();
        let good = json!({
            "patch": format!(
                "*** Begin Patch\n*** Update File: {}\n-two\n+TWO\n*** Add File: {}\n+new\n*** Delete File: {}\n*** End Patch",
                p("keep.txt"), p("new.txt"), p("gone.txt")
            )
        });
        let result = apply_patch("t1", &good.to_string()).unwrap();
        assert!(result.contains("1 added, 1 updated, 1 deleted"), "{result}");
        assert_eq!(
            fs::read_to_string(dir.join("keep.txt")).unwrap(),
            "one\nTWO\n"
        );
        assert_eq!(fs::read_to_string(dir.join("new.txt")).unwrap(), "new\n");
        assert!(!dir.join("gone.txt").exists());
        assert_eq!(applied_diffs("t1").len(), 3);
        assert!(applied_diffs("t1").is_empty());

        // The second hunk fails, so the first file must stay untouched.
        let bad = json!({
            "patch": format!(
                "*** Begin Patch\n*** Update File: {}\n-one\n+ONE\n*** Update File: {}\n-missing\n+x\n*** End Patch",
                p("keep.txt"), p("new.txt")
            )
        });
        assert!(apply_patch("t2", &bad.to_string()).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("keep.txt")).unwrap(),
            "one\nTWO\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
