//! UI color themes: built-in palettes, the active one picked by an index into
//! the static list so every draw call can read colors lock-free.

use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::style::Color;

/// One palette. `text` of `Color::Reset` keeps the terminal's default.
#[derive(Clone, Copy)]
pub struct Theme {
    pub muted: Color,
    pub accent: Color,
    pub error: Color,
    pub added: Color,
    pub removed: Color,
    pub warning: Color,
    pub text: Color,
    pub code: Color,
}

pub const THEMES: [(&str, Theme); 6] = [
    (
        "dark",
        Theme {
            muted: Color::DarkGray,
            accent: Color::Cyan,
            error: Color::Red,
            added: Color::Green,
            removed: Color::Red,
            warning: Color::Yellow,
            text: Color::Reset,
            code: Color::LightCyan,
        },
    ),
    (
        "midnight",
        Theme {
            muted: Color::Rgb(96, 108, 140),
            accent: Color::Rgb(122, 182, 255),
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
            accent: Color::Rgb(136, 192, 208),
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
            accent: Color::Rgb(131, 165, 152),
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
            accent: Color::Rgb(139, 233, 253),
            error: Color::Rgb(255, 85, 85),
            added: Color::Rgb(80, 250, 123),
            removed: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(241, 250, 140),
            text: Color::Rgb(248, 248, 242),
            code: Color::Rgb(189, 147, 249),
        },
    ),
    (
        "light",
        Theme {
            muted: Color::Rgb(120, 124, 130),
            accent: Color::Rgb(0, 95, 175),
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

/// Comma-separated list of the built-in names, for `/theme`.
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
        assert!(by_name("nope").is_none());
        assert!(set("dark"));
        assert_eq!(current_name(), "dark");
    }
}
