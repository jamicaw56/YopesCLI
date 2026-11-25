//! `@path` mentions: path completion in the composer, and the file contents
//! attached to the message when it is sent.
//!
//! The user typed the path, so it is read as given: absolute paths work, and a
//! `..` outside the workspace is allowed. Only regular files that read as text
//! are attached.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

/// How many mentioned files one message attaches.
const MAX_MENTION_FILES: usize = 8;
/// The largest part of one file a mention attaches.
const MAX_MENTION_BYTES: usize = 48_000;
/// Entries one `@` completion directory scan lists.
const MAX_MENTION_CANDIDATES: usize = 8;

/// A `@path` in the input: the path after the `@`.
struct Mention {
    path: String,
}

/// Every `@path` token in `text`, in order. A token is a whitespace-delimited
/// word starting with `@`; a trailing `.`, `,`, `;`, `:` or `)` is dropped when
/// the path as typed does not exist, so `@src/main.rs.` works.
fn mentions(text: &str, root: &Path) -> Vec<Mention> {
    let mut found = Vec::new();
    for word in text.split_whitespace() {
        let Some(path) = word.strip_prefix('@') else {
            continue;
        };
        // Sentence punctuation is not part of the path unless the path as
        // typed really exists (a file may end in `)`).
        let trimmed = path.trim_end_matches(['.', ',', ';', ':', ')']);
        let path = if resolve(root, path).is_some() || trimmed.is_empty() {
            path
        } else {
            trimmed
        };
        if path.is_empty() {
            continue;
        }
        found.push(Mention {
            path: path.to_owned(),
        });
    }
    found
}

/// The path a mention names, resolved against the workspace: an absolute path
/// as given, a relative one under `root`. `None` when it holds no such file.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    joined.canonicalize().ok().filter(|path| path.is_file())
}

/// The message with every mentioned file appended in a fenced block, plus the
/// paths that were attached. A mention naming no file stays as typed text.
pub(crate) fn expand(text: &str, root: &Path) -> (String, Vec<String>) {
    let mut attached: Vec<String> = Vec::new();
    let mut blocks = String::new();
    for mention in mentions(text, root) {
        if attached.len() >= MAX_MENTION_FILES {
            break;
        }
        if attached.iter().any(|seen| seen == &mention.path) {
            continue;
        }
        let Some(path) = resolve(root, &mention.path) else {
            continue;
        };
        let Some(contents) = read_text(&path) else {
            continue;
        };
        // The fence is longer than any backtick run in the file, so a file
        // that itself contains ``` closes nothing.
        let fence = "`".repeat(longest_backticks(&contents).max(2) + 1);
        blocks.push_str(&format!(
            "\n\nfile {}\n{fence}\n{contents}\n{fence}\n",
            mention.path
        ));
        attached.push(mention.path);
    }
    if attached.is_empty() {
        return (text.to_owned(), attached);
    }
    (format!("{text}{blocks}"), attached)
}

/// The files in a message that `expand` would attach, without reading them.
#[cfg(test)]
pub(crate) fn mentioned(text: &str, root: &Path) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for mention in mentions(text, root) {
        if resolve(root, &mention.path).is_none() || paths.contains(&mention.path) {
            continue;
        }
        paths.push(mention.path);
    }
    paths
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    let cut = bytes.len().min(MAX_MENTION_BYTES);
    let text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
    if text.contains('\0') || text.trim().is_empty() {
        return None;
    }
    Some(text)
}

fn longest_backticks(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// Palette rows for the `@path` being typed at the end of the input: the
/// directory it names, filtered by the text before the last separator.
pub(crate) fn suggestions(value: &str, root: &Path) -> Vec<Suggestion> {
    let (head, word) = match value.rsplit_once(char::is_whitespace) {
        Some((head, word)) => (format!("{head} "), word),
        None => (String::new(), value),
    };
    let Some(partial) = word.strip_prefix('@') else {
        return Vec::new();
    };
    // The separator stays in the directory part, so a completed level keeps
    // its slash and the next one completes straight away.
    let (directory, prefix) = match partial.rfind(['/', '\\']) {
        Some(at) => (partial[..=at].to_owned(), partial[at + 1..].to_owned()),
        None => (String::new(), partial.to_owned()),
    };
    let folder = directory.trim_end_matches(['/', '\\']);
    let base = if folder.is_empty() {
        if directory.is_empty() {
            root.to_path_buf()
        } else {
            // The separator alone: the filesystem root.
            PathBuf::from(&directory)
        }
    } else if Path::new(folder).is_absolute() {
        PathBuf::from(folder)
    } else {
        root.join(folder)
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let ignore = tools::Ignore::load(root);
    let lowered = prefix.to_ascii_lowercase();
    let mut candidates: Vec<(String, bool)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            // Hidden entries stay out of the way unless asked for by name.
            (prefix.starts_with('.') || !name.starts_with('.'))
                && name.to_ascii_lowercase().starts_with(&lowered)
                && !ignore.skip(&entry.path(), is_dir)
        })
        .map(|entry| {
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            (entry.file_name().to_string_lossy().to_string(), is_dir)
        })
        .collect();
    // Directories first, like every listing in the harness.
    candidates.sort_by_key(|(name, is_dir)| (!*is_dir, name.to_ascii_lowercase()));
    candidates.truncate(MAX_MENTION_CANDIDATES);
    candidates
        .into_iter()
        .map(|(name, is_dir)| {
            let tail = if is_dir { "/" } else { " " };
            Suggestion {
                label: format!("{directory}{name}{tail}").trim_end().to_owned(),
                completion: format!("{head}@{directory}{name}{tail}"),
                description: if is_dir {
                    "Directory · keeps completing"
                } else {
                    "File · attaches on send"
                },
                submit: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{MAX_MENTION_BYTES, expand, mentioned, suggestions};
    use std::fs;
    use std::path::Path;

    fn workspace(files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = crate::temp::unique_dir("yopes-mentions-test");
        for (name, body) in files {
            fs::write(root.join(name), body).unwrap();
        }
        root
    }

    #[test]
    fn a_mention_attaches_the_file_under_a_fence() {
        let root = workspace(&[("a.rs", "fn main() {}\n")]);
        let (text, attached) = expand("look at @a.rs please", &root);
        assert_eq!(attached, vec!["a.rs".to_owned()]);
        assert!(text.starts_with("look at @a.rs please"), "{text}");
        assert!(text.contains("file a.rs"), "{text}");
        assert!(text.contains("fn main() {}"), "{text}");
        assert!(text.contains("```"), "{text}");
        // A file that itself holds a fence closes nothing: the outer one grows.
        let fenced = workspace(&[("d.md", "```\ninner\n```")]);
        let (text, _) = expand("@d.md", &fenced);
        assert!(text.contains("````"), "{text}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_or_untouched_mention_stays_as_typed() {
        let root = workspace(&[]);
        let (text, attached) = expand("email me@example.com and @missing.rs", &root);
        assert!(attached.is_empty());
        assert_eq!(text, "email me@example.com and @missing.rs");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn punctuation_after_a_mention_is_not_part_of_the_path() {
        let root = workspace(&[("b.rs", "x")]);
        let (_, attached) = expand("read @b.rs.", &root);
        // Windows resolves `b.rs.` to `b.rs`, so the token keeps its dot there;
        // elsewhere the trailing stop is trimmed. Either way, one file attaches.
        assert_eq!(attached.len(), 1, "{attached:?}");
        assert!(attached[0].starts_with("b.rs"), "{attached:?}");
        assert_eq!(mentioned("read @b.rs.", &root).len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_mention_that_resolves_is_listed_once() {
        let root = workspace(&[("c.rs", "y")]);
        assert_eq!(
            mentioned("@c.rs and again @c.rs", &root),
            vec!["c.rs".to_owned()]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_big_or_binary_file_is_cut_and_skipped() {
        let root = workspace(&[("big.rs", &"a".repeat(MAX_MENTION_BYTES + 500))]);
        let (text, attached) = expand("@big.rs", &root);
        assert_eq!(attached, vec!["big.rs".to_owned()]);
        assert!(text.len() < MAX_MENTION_BYTES + 200, "{}", text.len());
        let binary = workspace(&[]);
        fs::write(binary.join("bin.dat"), b"\0\0\0").unwrap();
        assert!(expand("@bin.dat", &binary).1.is_empty());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(binary);
    }

    #[test]
    fn typing_a_mention_completes_the_directory_name() {
        let root = workspace(&[("main.rs", ""), ("mod.rs", "")]);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();

        let rows = suggestions("look at @m", &root);
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, vec!["main.rs", "mod.rs"]);
        assert_eq!(rows[0].completion, "look at @main.rs ");
        assert!(!rows[0].submit);

        // A directory keeps the slash so the next level completes at once.
        let rows = suggestions("@s", &root);
        assert_eq!(rows[0].label, "src/");
        assert_eq!(rows[0].completion, "@src/");
        let rows = suggestions("@src/", &root);
        assert_eq!(rows[0].label, "src/lib.rs");
        // Nothing typed after the `@`, and no `@` at all: no rows.
        assert!(!suggestions("@", &root).is_empty());
        assert!(suggestions("plain text", &root).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_mention_path_may_leave_the_workspace() {
        let root = workspace(&[("inside.rs", "")]);
        let outside = workspace(&[("outside.rs", "")]);
        let absolute = format!("@{}", outside.join("outside.rs").display());
        assert_eq!(
            mentioned(&absolute, &root),
            vec![outside.join("outside.rs").display().to_string()]
        );
        let relative = format!("@{}", Path::new("..").join("nowhere.rs").display());
        assert!(mentioned(&relative, &root).is_empty());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }
}
