//! The post-update changelog popup: what changed since the version the user
//! last ran. The text comes from the `CHANGELOG.md` compiled into the binary,
//! so it reads the same offline and needs no extra release file.
//!
//! One version, grouped by the `###` heading each bullet sits under: a release
//! that opens `### Added` three times reads as one Added block, and every group
//! knows how many bullets it holds so the popup can count them.

/// The changelog shipped with this build.
const SOURCE: &str = include_str!("../CHANGELOG.md");

/// The `###` heading a bullet sits under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Added,
    Fixed,
    Removed,
    Changed,
    /// A heading outside the common four (`Security`, ...): one group per
    /// heading text, listed in file order.
    Other,
}

impl Kind {
    fn of_heading(heading: &str) -> Self {
        match heading.trim().to_ascii_lowercase().as_str() {
            "added" => Self::Added,
            "fixed" => Self::Fixed,
            "removed" => Self::Removed,
            "changed" => Self::Changed,
            _ => Self::Other,
        }
    }

    /// The order the popup lists groups in: the news first, the long tail last.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Added => 0,
            Self::Fixed => 1,
            Self::Removed => 2,
            Self::Changed => 3,
            Self::Other => 4,
        }
    }

    /// The mark in front of the heading, so a group reads at a glance.
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Self::Added => "+",
            Self::Fixed => "~",
            Self::Removed => "-",
            Self::Changed | Self::Other => "·",
        }
    }

    /// The word in the count row and the `+N more` line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Fixed => "fixed",
            Self::Removed => "removed",
            Self::Changed => "changed",
            Self::Other => "more",
        }
    }
}

/// One `###` heading's bullets, with every repeat of that heading merged in.
#[derive(Clone)]
pub(crate) struct Group {
    pub(crate) kind: Kind,
    /// The heading as written; empty for bullets that sit above any heading.
    pub(crate) title: String,
    pub(crate) items: Vec<String>,
}

/// One version of the changelog: its `##` heading and its groups.
#[derive(Clone)]
pub(crate) struct Section {
    pub(crate) title: String,
    pub(crate) groups: Vec<Group>,
}

impl Section {
    /// How many bullets the group of `kind` holds.
    pub(crate) fn count(&self, kind: Kind) -> usize {
        self.groups
            .iter()
            .filter(|group| group.kind == kind)
            .map(|group| group.items.len())
            .sum()
    }

    fn is_empty(&self) -> bool {
        self.groups.iter().all(|group| group.items.is_empty())
    }

    /// The group index for `heading`, merging a repeat of a heading already
    /// seen in this version.
    fn group_for(&mut self, heading: &str) -> usize {
        let kind = Kind::of_heading(heading);
        let existing = self.groups.iter().position(|group| {
            if kind == Kind::Other {
                group.kind == Kind::Other && group.title.eq_ignore_ascii_case(heading)
            } else {
                group.kind == kind
            }
        });
        if let Some(index) = existing {
            return index;
        }
        self.groups.push(Group {
            kind,
            title: heading.to_owned(),
            items: Vec::new(),
        });
        self.groups.len() - 1
    }

    /// Listed in reading order, with headings that carry no bullet dropped.
    fn ordered(mut self) -> Self {
        // `sort_by_key` is stable, so several `Other` groups keep file order.
        self.groups.sort_by_key(|group| group.kind.rank());
        self.groups.retain(|group| !group.items.is_empty());
        self
    }
}

/// The section for `version`, or the `Unreleased` one when the running build
/// has no release heading yet.
pub(crate) fn section_for(version: &str) -> Option<Section> {
    parse(SOURCE, version)
}

/// Parse an arbitrary changelog text: the popup rendering tests build their own
/// sections with it.
#[cfg(test)]
pub(crate) fn parse_for_tests(source: &str, version: &str) -> Option<Section> {
    parse(source, version)
}

fn parse(source: &str, version: &str) -> Option<Section> {
    let wanted = format!("[{}]", version.trim_start_matches('v'));
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;
    let mut group: Option<usize> = None;
    for line in source.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.extend(current.take());
            current = Some(Section {
                title: heading.trim().to_owned(),
                groups: Vec::new(),
            });
            group = None;
            continue;
        }
        let Some(section) = current.as_mut() else {
            continue;
        };
        let line = line.trim_end();
        if let Some(heading) = line.strip_prefix("### ") {
            group = Some(section.group_for(heading.trim()));
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            // A bullet above any `###` heading still belongs to the version.
            let index = match group {
                Some(index) => index,
                None => {
                    let index = section.group_for("");
                    group = Some(index);
                    index
                }
            };
            section.groups[index]
                .items
                .push(format!("- {}", item.trim()));
            continue;
        }
        // A wrapped bullet continues the one above it.
        if !line.is_empty()
            && line.starts_with(' ')
            && let Some(index) = group
            && let Some(last) = section.groups[index].items.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
        }
    }
    sections.extend(current);
    sections
        .iter()
        .find(|section| section.title == wanted && !section.is_empty())
        .or_else(|| {
            sections
                .iter()
                .find(|section| section.title == "[Unreleased]" && !section.is_empty())
        })
        .cloned()
        .map(Section::ordered)
}

#[cfg(test)]
mod tests {
    use super::{Kind, parse};

    const SAMPLE: &str = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- Unreleased thing.\n\n## [0.2.0]\n\n- Shipped thing that\n  wraps onto a second line.\n\n### Fixed\n\n- First fix.\n\n### Added\n\n- A new thing.\n\n### Added\n\n- Another new thing.\n\n### Removed\n\n- A dropped thing.\n\n## [0.1.0]\n\n- Old thing.\n";

    #[test]
    fn picks_the_running_version_and_falls_back_to_unreleased() {
        let shipped = parse(SAMPLE, "0.2.0").expect("section");
        assert_eq!(shipped.title, "[0.2.0]");

        let unreleased = parse(SAMPLE, "0.3.0").expect("unreleased section");
        assert_eq!(unreleased.title, "[Unreleased]");
        assert_eq!(unreleased.count(Kind::Added), 1);
    }

    #[test]
    fn repeated_headings_merge_and_counts_add_up() {
        let shipped = parse(SAMPLE, "0.2.0").expect("section");
        // Three `### Added` blocks are one group, not three headings.
        let added: Vec<_> = shipped
            .groups
            .iter()
            .filter(|group| group.kind == Kind::Added)
            .collect();
        assert_eq!(added.len(), 1);
        assert_eq!(shipped.count(Kind::Added), 2);
        assert_eq!(shipped.count(Kind::Fixed), 1);
        assert_eq!(shipped.count(Kind::Removed), 1);
        // The order is the reading order the popup draws: Added, Fixed,
        // Removed, then anything else.
        let kinds: Vec<Kind> = shipped.groups.iter().map(|group| group.kind).collect();
        assert_eq!(
            kinds,
            vec![Kind::Added, Kind::Fixed, Kind::Removed, Kind::Other]
        );
        // The bullets above the first heading keep their own group.
        let other = shipped.groups.last().expect("the heading-less group");
        assert_eq!(other.title, "");
        assert_eq!(
            other.items,
            vec!["- Shipped thing that wraps onto a second line.".to_owned()]
        );
    }

    #[test]
    fn a_heading_with_no_bullet_is_dropped() {
        let source = "## [1.0.0]\n\n### Added\n\n- One thing.\n\n### Fixed\n\n### Changed\n\n- Changed thing.\n";
        let section = parse(source, "1.0.0").expect("section");
        let titles: Vec<&str> = section
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Added", "Changed"]);
    }

    #[test]
    fn the_shipped_changelog_parses() {
        let section = super::section_for(crate::update::current_version())
            .or_else(|| super::section_for("Unreleased"))
            .expect("a section to show");
        assert!(!section.groups.is_empty());
        assert!(section.count(Kind::Added) + section.count(Kind::Changed) > 0);
    }
}
