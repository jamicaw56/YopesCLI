//! The post-update changelog popup: what changed since the version the user
//! last ran. The text comes from the `CHANGELOG.md` compiled into the binary,
//! so it reads the same offline and needs no extra release file.

/// The changelog shipped with this build.
const SOURCE: &str = include_str!("../CHANGELOG.md");

/// One section of the changelog: its heading and the lines under it.
pub(crate) struct Section {
    pub(crate) title: String,
    pub(crate) lines: Vec<String>,
}

/// The section for `version`, or the `Unreleased` one when the running build
/// has no release heading yet.
pub(crate) fn section_for(version: &str) -> Option<Section> {
    parse(SOURCE, version)
}

fn parse(source: &str, version: &str) -> Option<Section> {
    let wanted = format!("[{}]", version.trim_start_matches('v'));
    let mut section: Option<Section> = None;
    let mut unreleased: Option<Section> = None;
    let mut current: Option<Section> = None;
    for line in source.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(done) = current.take() {
                // Keep the first match only; a duplicate heading is a typo.
                if section.is_none() && done.title == wanted {
                    section = Some(done);
                } else if unreleased.is_none() && done.title == "[Unreleased]" {
                    unreleased = Some(done);
                }
            }
            current = Some(Section {
                title: heading.trim().to_owned(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(section) = current.as_mut() else {
            continue;
        };
        let line = line.trim_end();
        // Bullets and their wrapped continuations, plus `###` subsections.
        // Anything else (blank lines) is left out: the popup stays short.
        if line.starts_with("- ") {
            section.lines.push(line.to_owned());
        } else if line.starts_with("### ") {
            section.lines.push(String::new());
            section
                .lines
                .push(line.trim_start_matches("### ").to_owned());
        } else if !line.is_empty()
            && section.lines.last().is_some_and(|last| {
                !last.is_empty() && !last.starts_with("### ") && line.starts_with(' ')
            })
        {
            if let Some(last) = section.lines.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        }
    }
    if let Some(done) = current {
        if section.is_none() && done.title == wanted {
            section = Some(done);
        } else if unreleased.is_none() && done.title == "[Unreleased]" {
            unreleased = Some(done);
        }
    }
    section
        .or(unreleased)
        .filter(|found| !found.lines.is_empty())
}

#[cfg(test)]
mod tests {
    use super::parse;

    const SAMPLE: &str = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Unreleased thing.\n\n## [0.2.0]\n\n- Shipped thing that\n  wraps onto a second line.\n\n## [0.1.0]\n\n- Old thing.\n";

    #[test]
    fn picks_the_running_version_and_falls_back_to_unreleased() {
        let shipped = parse(SAMPLE, "0.2.0").expect("section");
        assert_eq!(shipped.title, "[0.2.0]");
        assert_eq!(
            shipped.lines,
            vec!["- Shipped thing that wraps onto a second line.".to_owned()]
        );

        let unreleased = parse(SAMPLE, "0.3.0").expect("unreleased section");
        assert_eq!(unreleased.title, "[Unreleased]");
        assert!(unreleased.lines.iter().any(|line| line.contains("Added")));
        assert!(
            unreleased
                .lines
                .iter()
                .any(|line| line.contains("Unreleased thing"))
        );
    }

    #[test]
    fn the_shipped_changelog_parses() {
        let section = super::section_for(crate::update::current_version())
            .or_else(|| super::section_for("Unreleased"))
            .expect("a section to show");
        assert!(!section.lines.is_empty());
    }
}
