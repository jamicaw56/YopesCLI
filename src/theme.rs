//! UI color themes: built-in palettes, the active one picked by an index into
//! the static list so every draw call can read colors lock-free.
//!
//! Each palette carries two extra roles beyond the classic eight: `dim`, for
//! rules and borders that must stay quieter than `muted` text, and
//! `accent2`, the secondary accent for brand marks, cursors and highlights,
//! so the primary accent stays reserved for interactive state.

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::Color;

/// One palette. `text` of `Color::Reset` keeps the terminal's default.
#[derive(Clone, Copy)]
pub struct Theme {
    pub muted: Color,
    /// Quieter than `muted`: separators, tool frames, hint footers.
    pub dim: Color,
    /// Primary accent: active tabs, selections, streaming state.
    pub accent: Color,
    /// Secondary accent: the brand mark, the composer caret, highlights.
    pub accent2: Color,
    pub error: Color,
    pub added: Color,
    pub removed: Color,
    pub warning: Color,
    pub text: Color,
    pub code: Color,
}

pub const THEMES: [(&str, Theme); 9] = [
    (
        "dark",
        Theme {
            muted: Color::Rgb(122, 134, 158),
            dim: Color::Rgb(64, 72, 92),
            accent: Color::Rgb(138, 180, 248),
            accent2: Color::Rgb(184, 148, 255),
            error: Color::Rgb(244, 113, 116),
            added: Color::Rgb(110, 225, 158),
            removed: Color::Rgb(244, 113, 116),
            warning: Color::Rgb(230, 180, 110),
            text: Color::Reset,
            code: Color::Rgb(128, 222, 234),
        },
    ),
    (
        "midnight",
        Theme {
            muted: Color::Rgb(96, 108, 140),
            dim: Color::Rgb(58, 66, 90),
            accent: Color::Rgb(122, 182, 255),
            accent2: Color::Rgb(168, 130, 255),
            error: Color::Rgb(255, 107, 107),
            added: Color::Rgb(108, 224, 144),
            removed: Color::Rgb(255, 107, 107),
            warning: Color::Rgb(240, 198, 116),
            text: Color::Rgb(205, 216, 238),
            code: Color::Rgb(102, 204, 204),
        },
    ),
    (
        "nord",
        Theme {
            muted: Color::Rgb(97, 110, 136),
            dim: Color::Rgb(69, 81, 102),
            accent: Color::Rgb(136, 192, 208),
            accent2: Color::Rgb(180, 142, 173),
            error: Color::Rgb(191, 97, 106),
            added: Color::Rgb(163, 190, 140),
            removed: Color::Rgb(191, 97, 106),
            warning: Color::Rgb(235, 203, 139),
            text: Color::Rgb(229, 233, 240),
            code: Color::Rgb(129, 161, 193),
        },
    ),
    (
        "gruvbox",
        Theme {
            muted: Color::Rgb(146, 131, 116),
            dim: Color::Rgb(104, 92, 82),
            accent: Color::Rgb(131, 165, 152),
            accent2: Color::Rgb(213, 148, 82),
            error: Color::Rgb(251, 73, 52),
            added: Color::Rgb(184, 187, 38),
            removed: Color::Rgb(251, 73, 52),
            warning: Color::Rgb(250, 189, 47),
            text: Color::Rgb(235, 219, 178),
            code: Color::Rgb(142, 192, 124),
        },
    ),
    (
        "dracula",
        Theme {
            muted: Color::Rgb(98, 114, 164),
            dim: Color::Rgb(68, 71, 90),
            accent: Color::Rgb(139, 233, 253),
            accent2: Color::Rgb(189, 147, 249),
            error: Color::Rgb(255, 85, 85),
            added: Color::Rgb(80, 250, 123),
            removed: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            text: Color::Rgb(248, 248, 242),
            code: Color::Rgb(189, 147, 249),
        },
    ),
    (
        "tokyo",
        Theme {
            muted: Color::Rgb(90, 102, 151),
            dim: Color::Rgb(54, 60, 96),
            accent: Color::Rgb(122, 162, 247),
            accent2: Color::Rgb(187, 154, 247),
            error: Color::Rgb(247, 118, 142),
            added: Color::Rgb(115, 218, 202),
            removed: Color::Rgb(247, 118, 142),
            warning: Color::Rgb(224, 175, 104),
            text: Color::Rgb(169, 177, 214),
            code: Color::Rgb(125, 207, 255),
        },
    ),
    (
        "rose",
        Theme {
            muted: Color::Rgb(110, 106, 134),
            dim: Color::Rgb(64, 61, 82),
            accent: Color::Rgb(196, 167, 231),
            accent2: Color::Rgb(156, 207, 216),
            error: Color::Rgb(235, 111, 146),
            added: Color::Rgb(149, 214, 188),
            removed: Color::Rgb(235, 111, 146),
            warning: Color::Rgb(246, 193, 119),
            text: Color::Rgb(224, 222, 244),
            code: Color::Rgb(156, 207, 216),
        },
    ),
    (
        "catppuccin",
        Theme {
            muted: Color::Rgb(147, 154, 183),
            dim: Color::Rgb(108, 112, 134),
            accent: Color::Rgb(137, 180, 250),
            accent2: Color::Rgb(203, 166, 247),
            error: Color::Rgb(243, 139, 168),
            added: Color::Rgb(166, 227, 161),
            removed: Color::Rgb(243, 139, 168),
            warning: Color::Rgb(250, 179, 135),
            text: Color::Rgb(205, 214, 244),
            code: Color::Rgb(148, 226, 213),
        },
    ),
    (
        "light",
        Theme {
            muted: Color::Rgb(120, 124, 130),
            dim: Color::Rgb(180, 184, 190),
            accent: Color::Rgb(0, 95, 175),
            accent2: Color::Rgb(108, 64, 180),
            error: Color::Rgb(196, 26, 26),
            added: Color::Rgb(26, 127, 55),
            removed: Color::Rgb(196, 26, 26),
            warning: Color::Rgb(154, 103, 0),
            text: Color::Reset,
            code: Color::Rgb(0, 105, 140),
        },
    ),
];

static INDEX: AtomicUsize = AtomicUsize::new(0);

/// Index of `name` in the built-in list, case-insensitive.
pub fn by_name(name: &str) -> Option<usize> {
    THEMES
        .iter()
        .position(|(known, _)| known.eq_ignore_ascii_case(name.trim()))
}

/// Activate the named theme; `false` when the name is unknown.
pub fn set(name: &str) -> bool {
    match by_name(name) {
        Some(index) => {
            INDEX.store(index, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// The active palette.
pub fn current() -> &'static Theme {
    &THEMES[INDEX.load(Ordering::Relaxed)].1
}

/// Name of the active theme.
pub fn current_name() -> &'static str {
    THEMES[INDEX.load(Ordering::Relaxed)].0
}

/// Comma-separated list of the built-in names, for `/set theme`.
pub fn names() -> String {
    THEMES
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_case_insensitively() {
        assert_eq!(by_name("GRUVBOX"), by_name("gruvbox"));
        assert_eq!(by_name("tokyo"), by_name("Tokyo"));
        assert!(by_name("nope").is_none());
        assert!(set("dark"));
        assert_eq!(current_name(), "dark");
    }

    #[test]
    fn every_palette_defines_all_roles() {
        for (name, theme) in THEMES {
            // The dim role must sit a clear margin away from muted, whichever
            // direction the palette reads: dark themes go darker, the light
            // theme goes lighter. 60 of 765 keeps separators from sitting
            // indistinguishably on top of body text.
            let dim_is_quieter = match (theme.muted, theme.dim) {
                (Color::Rgb(mr, mg, mb), Color::Rgb(dr, dg, db)) => {
                    let m = u32::from(mr) + u32::from(mg) + u32::from(mb);
                    let d = u32::from(dr) + u32::from(dg) + u32::from(db);
                    m.abs_diff(d) >= 60
                }
                _ => true,
            };
            assert!(dim_is_quieter, "{name}: dim does not separate from muted");
        }
    }
}
