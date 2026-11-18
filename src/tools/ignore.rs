//! Gitignore-style path filtering for the filesystem walkers.
//!
//! The rules live in a `.yopesignore` file at the workspace root (one pattern
//! per line, `#` comments, `!` negation, `/` anchor, trailing `/` for
//! directories, `*`, `?` and `**` inside a segment). With no file present
//! nothing is filtered, so the walkers behave exactly as they did before.
//!
//! Matching follows git: a rule matches the path itself or any parent
//! directory, the last matching rule wins, and a `!` line re-includes what an
//! earlier rule excluded. A path outside the workspace is never filtered.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::search::glob_matches;

const FILE_NAME: &str = ".yopesignore";

#[derive(Debug, Clone)]
struct Rule {
    /// Pattern segments; an unanchored rule gets a leading `**`.
    segments: Vec<String>,
    negated: bool,
    directory_only: bool,
}

/// The rules read from the workspace's `.yopesignore`.
#[derive(Debug, Clone, Default)]
pub struct Ignore {
    rules: Vec<Rule>,
    root: PathBuf,
}

impl Ignore {
    /// Load `<root>/.yopesignore`. A missing or unreadable file yields no
    /// rules, which filters nothing.
    pub fn load(root: &Path) -> Self {
        let Ok(text) = fs::read_to_string(root.join(FILE_NAME)) else {
            return Self {
                rules: Vec::new(),
                root: root.to_path_buf(),
            };
        };
        let mut ignore = Self::parse(&text);
        ignore.root = root.to_path_buf();
        ignore
    }

    /// Parse rules from the file's text; blank lines and comments are skipped.
    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.trim_end_matches(['\r', '\n']).trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, pattern) = match line.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, line),
            };
            if pattern.is_empty() {
                continue;
            }
            let directory_only = pattern.ends_with('/');
            let pattern = pattern.trim_end_matches('/');
            let anchored = pattern.contains('/');
            let pattern = pattern.trim_start_matches('/').replace('\\', "/");
            let mut segments = pattern
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if segments.is_empty() {
                continue;
            }
            // An unanchored pattern matches at any depth: a leading `**`.
            if !anchored {
                segments.insert(0, "**".to_owned());
            }
            rules.push(Rule {
                segments,
                negated,
                directory_only,
            });
        }
        Self {
            rules,
            root: PathBuf::new(),
        }
    }

    /// Whether the absolute `path` is filtered out of a walk rooted at the
    /// workspace. Paths outside the workspace, and the workspace itself, are
    /// never ignored.
    pub fn skip(&self, path: &Path, is_dir: bool) -> bool {
        if self.rules.is_empty() {
            return false;
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        self.is_ignored(&relative, is_dir)
    }

    /// Whether `relative` (a `/`-joined path from the workspace root) is
    /// ignored.
    pub fn is_ignored(&self, relative: &str, is_dir: bool) -> bool {
        let relative = relative.trim();
        let relative = relative.strip_prefix("./").unwrap_or(relative);
        let relative = relative
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_owned();
        if relative.is_empty() || self.rules.is_empty() {
            return false;
        }
        let segments = relative
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut ignored = false;
        for rule in &self.rules {
            let direct = glob_matches(&rule.segments, &segments);
            let via_parent = covered_by_parent(&rule.segments, &segments);
            // A trailing-slash rule hides the directory and everything below
            // it; the file itself only matches through a parent.
            if rule.directory_only && !is_dir && !via_parent {
                continue;
            }
            if !direct && !via_parent {
                continue;
            }
            ignored = !rule.negated;
        }
        ignored
    }
}

/// Whether the pattern matches one of the path's parent directories.
fn covered_by_parent(pattern: &[String], segments: &[String]) -> bool {
    (1..segments.len()).any(|depth| glob_matches(pattern, &segments[..depth]))
}

#[cfg(test)]
mod tests {
    use super::Ignore;

    fn rules(text: &str) -> Ignore {
        Ignore::parse(text)
    }

    #[test]
    fn no_rules_ignore_nothing() {
        let rules = Ignore::parse("");
        assert!(rules.rules.is_empty());
        assert!(!rules.is_ignored("target/debug/yopes", false));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let rules = rules("# a comment\n\n  \n*.log\n");
        assert_eq!(rules.rules.len(), 1);
        assert!(rules.is_ignored("build.log", false));
    }

    #[test]
    fn unanchored_patterns_match_at_any_depth() {
        let rules = rules("*.snap\n");
        assert!(rules.is_ignored("a.snap", false));
        assert!(rules.is_ignored("one/two/a.snap", false));
        assert!(!rules.is_ignored("a.snapshot", false));
    }

    #[test]
    fn anchored_patterns_match_from_the_root_only() {
        let rules = rules("/build\n");
        assert!(rules.is_ignored("build", true));
        assert!(!rules.is_ignored("src/build", true));
    }

    #[test]
    fn directory_rules_cover_everything_below() {
        let rules = rules("node_modules/\n");
        assert!(rules.is_ignored("node_modules", true));
        assert!(rules.is_ignored("node_modules/pkg/index.js", false));
        assert!(!rules.is_ignored("node_modules_file", false));
    }

    #[test]
    fn negations_reinclude_one_path() {
        let rules = rules("*.log\n!keep.log\n");
        assert!(rules.is_ignored("debug.log", false));
        assert!(!rules.is_ignored("keep.log", false));
        assert!(rules.is_ignored("one/other.log", false));
        assert!(!rules.is_ignored("one/keep.log", false));
    }

    #[test]
    fn double_star_spans_directories() {
        let rules = rules("src/**/generated\n");
        assert!(rules.is_ignored("src/generated", true));
        assert!(rules.is_ignored("src/a/b/generated", true));
        assert!(!rules.is_ignored("tests/a/b/generated", true));
    }

    #[test]
    fn a_literal_dot_stays_literal() {
        let rules = rules(".yopes/\n");
        assert!(rules.is_ignored(".yopes", true));
        assert!(rules.is_ignored(".yopes/state.json", false));
        assert!(!rules.is_ignored("xyopes/a", false));
    }

    #[test]
    fn a_pattern_with_a_slash_is_anchored_to_the_root() {
        let rules = rules("src/generated/\n");
        assert!(rules.is_ignored("src/generated", true));
        assert!(rules.is_ignored("src/generated/a.rs", false));
        assert!(!rules.is_ignored("copies/src/generated", true));
    }

    #[test]
    fn paths_outside_the_workspace_are_never_filtered() {
        let rules = Ignore::load(std::path::Path::new("does-not-exist"));
        assert!(!rules.skip(std::path::Path::new("C:/elsewhere/a.log"), false));
    }
}
