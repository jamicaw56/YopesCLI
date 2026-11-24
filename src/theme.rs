//! UI color themes: built-in palettes, the active one picked by an index into
//! the static list so every draw call can read colors lock-free, plus `auto`,
//! which blends a clock-driven palette on every read instead.
//!
//! Each palette carries two extra roles beyond the classic eight: `dim`, for
//! rules and borders that must stay quieter than `muted` text, and
//! `accent2`, the secondary accent for brand marks, cursors and highlights,
//! so the primary accent stays reserved for interactive state.

use std::{
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::Timelike;
use ratatui::style::Color;

use crate::weather::{self, Weather};

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

pub const THEMES: [(&str, Theme); 13] = [
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
        "one-dark",
        Theme {
            muted: Color::Rgb(92, 99, 112),
            dim: Color::Rgb(62, 68, 81),
            accent: Color::Rgb(97, 175, 239),
            accent2: Color::Rgb(198, 120, 221),
            error: Color::Rgb(224, 108, 117),
            added: Color::Rgb(152, 195, 121),
            removed: Color::Rgb(224, 108, 117),
            warning: Color::Rgb(229, 192, 123),
            text: Color::Rgb(171, 178, 191),
            code: Color::Rgb(86, 182, 194),
        },
    ),
    (
        "solarized",
        Theme {
            muted: Color::Rgb(88, 110, 117),
            dim: Color::Rgb(0, 43, 54),
            accent: Color::Rgb(38, 139, 210),
            accent2: Color::Rgb(131, 148, 150),
            error: Color::Rgb(220, 50, 47),
            added: Color::Rgb(133, 153, 0),
            removed: Color::Rgb(220, 50, 47),
            warning: Color::Rgb(181, 137, 0),
            text: Color::Rgb(147, 161, 161),
            code: Color::Rgb(42, 161, 152),
        },
    ),
    (
        "everforest",
        Theme {
            muted: Color::Rgb(133, 146, 137),
            dim: Color::Rgb(75, 86, 78),
            accent: Color::Rgb(131, 192, 146),
            accent2: Color::Rgb(214, 153, 182),
            error: Color::Rgb(230, 126, 128),
            added: Color::Rgb(167, 192, 128),
            removed: Color::Rgb(230, 126, 128),
            warning: Color::Rgb(219, 188, 127),
            text: Color::Rgb(211, 198, 170),
            code: Color::Rgb(127, 187, 179),
        },
    ),
    (
        "kanagawa",
        Theme {
            muted: Color::Rgb(114, 113, 105),
            dim: Color::Rgb(54, 54, 70),
            accent: Color::Rgb(126, 156, 216),
            accent2: Color::Rgb(149, 127, 184),
            error: Color::Rgb(210, 126, 129),
            added: Color::Rgb(152, 187, 108),
            removed: Color::Rgb(210, 126, 129),
            warning: Color::Rgb(192, 163, 110),
            text: Color::Rgb(220, 215, 186),
            code: Color::Rgb(127, 180, 202),
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
/// On while the palette follows the local clock instead of a static one.
static AUTO: AtomicBool = AtomicBool::new(false);
/// The minute `auto` last asked for a repaint, so a session left alone still
/// redraws when the clock moves instead of waiting for a keypress.
static AUTO_SEEN: AtomicU32 = AtomicU32::new(u32::MAX);

/// Index of `name` in the built-in list, case-insensitive.
pub fn by_name(name: &str) -> Option<usize> {
    THEMES
        .iter()
        .position(|(known, _)| known.eq_ignore_ascii_case(name.trim()))
}

/// Activate the named theme; `false` when the name is unknown.
pub fn set(name: &str) -> bool {
    if name.eq_ignore_ascii_case("auto") {
        AUTO.store(true, Ordering::Relaxed);
        return true;
    }
    match by_name(name) {
        Some(index) => {
            AUTO.store(false, Ordering::Relaxed);
            INDEX.store(index, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// The active palette. `auto` samples the local clock on every read, so every
/// draw call stays lock-free while the backdrop drifts through the day; a
/// static theme costs one atomic load and a copy.
pub fn current() -> Theme {
    if AUTO.load(Ordering::Relaxed) {
        auto_at(local_hour(), weather::current())
    } else {
        THEMES[INDEX.load(Ordering::Relaxed)].1
    }
}

/// Name of the active theme.
pub fn current_name() -> &'static str {
    if AUTO.load(Ordering::Relaxed) {
        "auto"
    } else {
        THEMES[INDEX.load(Ordering::Relaxed)].0
    }
}

/// Every name `/set theme` accepts: `auto` first, then the built-in palettes.
pub fn list() -> Vec<&'static str> {
    std::iter::once("auto")
        .chain(THEMES.iter().map(|(name, _)| *name))
        .collect()
}

/// The offered names joined up, for `/set theme`.
pub fn names() -> String {
    list().join(" · ")
}

/// One channel triple the `auto` ramp blends between keyframes.
type Rgb = (u8, u8, u8);

/// An `auto` keyframe: the palette and sky at one local hour.
struct Key {
    hour: f32,
    /// Backdrop color at the top row and at the bottom row.
    top: Rgb,
    horizon: Rgb,
    /// Star strength, 0-100.
    stars: u8,
    muted: Rgb,
    dim: Rgb,
    accent: Rgb,
    accent2: Rgb,
    text: Rgb,
    code: Rgb,
}

/// Local-hour keyframes for `auto`, midnight repeating so the ramp wraps.
/// Diff and warning colors stay fixed: their meaning must not drift with the
/// sun, and every `dim` stays under 420 so the backdrop keeps its dark base
/// all day. Sky colors stay dark too, so `muted` text keeps its contrast.
static AUTO_KEYS: [Key; 9] = [
    Key {
        hour: 0.0, // midnight
        top: (5, 7, 16),
        horizon: (12, 16, 36),
        stars: 100,
        muted: (74, 84, 116),
        dim: (40, 46, 74),
        accent: (124, 148, 236),
        accent2: (158, 134, 246),
        text: (194, 204, 228),
        code: (114, 178, 226),
    },
    Key {
        hour: 4.0, // pre-dawn
        top: (8, 10, 26),
        horizon: (22, 24, 56),
        stars: 80,
        muted: (80, 84, 120),
        dim: (44, 46, 78),
        accent: (132, 144, 230),
        accent2: (170, 136, 240),
        text: (196, 202, 226),
        code: (120, 172, 224),
    },
    Key {
        hour: 6.0, // dawn
        top: (20, 22, 50),
        horizon: (70, 38, 48),
        stars: 15,
        muted: (128, 104, 118),
        dim: (74, 58, 72),
        accent: (244, 168, 148),
        accent2: (250, 196, 132),
        text: (226, 214, 216),
        code: (150, 196, 214),
    },
    Key {
        hour: 8.0, // morning
        top: (22, 42, 78),
        horizon: (58, 52, 54),
        stars: 0,
        muted: (140, 122, 100),
        dim: (80, 70, 56),
        accent: (248, 198, 118),
        accent2: (250, 168, 96),
        text: (234, 226, 212),
        code: (110, 196, 180),
    },
    Key {
        hour: 12.0, // noon
        top: (14, 32, 68),
        horizon: (28, 58, 98),
        stars: 0,
        muted: (116, 136, 158),
        dim: (64, 76, 92),
        accent: (128, 206, 255),
        accent2: (154, 232, 222),
        text: (216, 226, 238),
        code: (110, 214, 236),
    },
    Key {
        hour: 15.0, // afternoon
        top: (16, 34, 68),
        horizon: (36, 62, 88),
        stars: 0,
        muted: (120, 138, 140),
        dim: (66, 78, 80),
        accent: (134, 214, 202),
        accent2: (246, 206, 128),
        text: (214, 226, 224),
        code: (120, 206, 214),
    },
    Key {
        hour: 18.0, // sunset
        top: (32, 26, 64),
        horizon: (80, 40, 36),
        stars: 0,
        muted: (142, 112, 124),
        dim: (82, 64, 74),
        accent: (252, 156, 116),
        accent2: (226, 146, 206),
        text: (234, 220, 220),
        code: (240, 176, 140),
    },
    Key {
        hour: 21.0, // evening
        top: (10, 11, 32),
        horizon: (34, 24, 56),
        stars: 60,
        muted: (104, 104, 146),
        dim: (52, 54, 78),
        accent: (168, 156, 248),
        accent2: (200, 148, 244),
        text: (204, 206, 234),
        code: (150, 176, 240),
    },
    Key {
        hour: 24.0, // midnight again, so the ramp wraps
        top: (5, 7, 16),
        horizon: (12, 16, 36),
        stars: 100,
        muted: (74, 84, 116),
        dim: (40, 46, 74),
        accent: (124, 148, 236),
        accent2: (158, 134, 246),
        text: (194, 204, 228),
        code: (114, 178, 226),
    },
];

/// `t` of the way from `from` to `to`, per channel.
fn mix(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let step = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    (step(from.0, to.0), step(from.1, to.1), step(from.2, to.2))
}

fn rgb(from: Rgb) -> Color {
    Color::Rgb(from.0, from.1, from.2)
}

/// How one weather bends the clock's sky.
struct Cover {
    /// Pull toward the color's own channel average, 0-1. The brightness stays
    /// put, so the contrast rules the keyframes keep survive every weather.
    gray: f32,
    /// Then lean the sky toward this color by `push`, 0-1.
    lean: Rgb,
    push: f32,
    /// Star field multiplier: cloud cover hides the stars.
    stars: f32,
}

fn cover(weather: Weather) -> Cover {
    let (gray, lean, push, stars) = match weather {
        Weather::Unknown | Weather::Clear => (0.0, (0, 0, 0), 0.0, 1.0),
        Weather::Cloudy => (0.45, (40, 46, 56), 0.20, 0.25),
        Weather::Fog => (0.60, (78, 84, 92), 0.45, 0.0),
        Weather::Rain => (0.55, (22, 32, 48), 0.35, 0.0),
        Weather::Snow => (0.55, (76, 84, 98), 0.40, 0.0),
        Weather::Storm => (0.65, (8, 10, 18), 0.55, 0.0),
    };
    Cover {
        gray,
        lean,
        push,
        stars,
    }
}

/// `color` pulled `amount` of the way to the gray of its own average.
fn gray(color: Rgb, amount: f32) -> Rgb {
    let mean = ((u16::from(color.0) + u16::from(color.1) + u16::from(color.2)) / 3) as u8;
    mix(color, (mean, mean, mean), amount)
}

/// The local hour as one fractional number, so the palette moves smoothly
/// between keyframes instead of stepping on the hour.
fn local_hour() -> f32 {
    let now = chrono::Local::now();
    now.hour() as f32 + now.minute() as f32 / 60.0
}

/// The keyframes around a fractional hour, and how far between them (0-1).
fn bracket(hour: f32) -> (&'static Key, &'static Key, f32) {
    let index = AUTO_KEYS
        .iter()
        .skip(1)
        .position(|key| hour < key.hour)
        .map(|found| found + 1)
        .unwrap_or(AUTO_KEYS.len() - 1);
    let (low, high) = (&AUTO_KEYS[index - 1], &AUTO_KEYS[index]);
    let t = ((hour - low.hour) / (high.hour - low.hour)).clamp(0.0, 1.0);
    (low, high, t)
}

/// The `auto` palette at a fractional local hour. Weather only grays the
/// colors: diff and warning colors keep their meaning.
fn auto_at(hour: f32, weather: Weather) -> Theme {
    let (low, high, t) = bracket(hour);
    let g = cover(weather).gray;
    let blend = |a: Rgb, b: Rgb| rgb(gray(mix(a, b, t), g));
    Theme {
        muted: blend(low.muted, high.muted),
        dim: blend(low.dim, high.dim),
        accent: blend(low.accent, high.accent),
        accent2: blend(low.accent2, high.accent2),
        error: Color::Rgb(244, 113, 116),
        added: Color::Rgb(110, 225, 158),
        removed: Color::Rgb(244, 113, 116),
        warning: Color::Rgb(230, 180, 110),
        text: blend(low.text, high.text),
        code: blend(low.code, high.code),
    }
}

/// The sun's glow in the backdrop: where it sits, as fractions of the
/// screen, its color, and how strongly it tints, 0-100.
pub struct Sun {
    pub x: f32,
    pub y: f32,
    pub color: Color,
    pub strength: usize,
}

/// What the backdrop paints: the sky gradient, the star field, the weather
/// in front of it, and the sun when the day is clear.
pub struct Sky {
    pub top: Color,
    pub horizon: Color,
    /// Star strength, 0-100.
    pub stars: usize,
    pub weather: Weather,
    pub sun: Option<Sun>,
}

/// The sun between 6:00 and 18:00: it climbs from the left edge, peaks at
/// noon, and sets on the right. Low sun is orange, high sun is pale, and it
/// fades in from nothing at the horizon so it never pops.
fn sun_at(hour: f32) -> Option<Sun> {
    let day = (hour - 6.0) / 12.0;
    if !(0.0..1.0).contains(&day) {
        return None;
    }
    let height = (std::f32::consts::PI * day).sin();
    Some(Sun {
        x: 0.1 + 0.8 * day,
        y: 0.82 - 0.5 * height,
        color: rgb(mix((150, 140, 112), (176, 92, 52), 1.0 - height)),
        strength: (50.0 * height.sqrt()).round() as usize,
    })
}

/// The sky at a fractional local hour under `weather`.
fn sky_at(hour: f32, weather: Weather) -> Sky {
    let (low, high, t) = bracket(hour);
    let cover = cover(weather);
    let stars = f32::from(low.stars) + (f32::from(high.stars) - f32::from(low.stars)) * t;
    let tint = |a: Rgb, b: Rgb| rgb(mix(gray(mix(a, b, t), cover.gray), cover.lean, cover.push));
    Sky {
        top: tint(low.top, high.top),
        horizon: tint(low.horizon, high.horizon),
        stars: (stars * cover.stars).round() as usize,
        weather,
        sun: (weather == Weather::Clear).then(|| sun_at(hour)).flatten(),
    }
}

/// The sky the backdrop paints right now: `None` unless `auto` is active.
pub fn sky() -> Option<Sky> {
    AUTO.load(Ordering::Relaxed).then(|| sky_at(local_hour(), weather::current()))
}

/// True once per wall-clock minute while `auto` is on: the event loop's ticker
/// uses it to repaint a session nobody is touching.
pub fn auto_redraw_due() -> bool {
    if !AUTO.load(Ordering::Relaxed) {
        return false;
    }
    let minute = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| (since.as_secs() / 60) as u32)
        .unwrap_or(0);
    AUTO_SEEN.swap(minute, Ordering::Relaxed) != minute
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEATHERS: [Weather; 6] = [
        Weather::Clear,
        Weather::Cloudy,
        Weather::Fog,
        Weather::Rain,
        Weather::Snow,
        Weather::Storm,
    ];

    /// `auto` holds the backdrop on its dark base, keeps dim separated from
    /// muted, and actually moves across the day, in every weather.
    #[test]
    fn auto_blends_the_clock_and_stays_readable() {
        for weather in WEATHERS {
            for step in 0..240 {
                let hour = step as f32 / 10.0;
                let theme = auto_at(hour, weather);
                let (Color::Rgb(mr, mg, mb), Color::Rgb(dr, dg, db)) = (theme.muted, theme.dim)
                else {
                    panic!("auto must define muted and dim as rgb");
                };
                let muted = u32::from(mr) + u32::from(mg) + u32::from(mb);
                let dim = u32::from(dr) + u32::from(dg) + u32::from(db);
                assert!(
                    muted.abs_diff(dim) >= 60,
                    "hour {hour} {weather:?} does not separate"
                );
                assert!(dim < 420, "hour {hour} {weather:?} lights the backdrop");
            }
        }
        // The ramp is not one frozen palette: noon differs from midnight.
        assert_ne!(
            auto_at(13.0, Weather::Clear).accent,
            auto_at(1.0, Weather::Clear).accent
        );
        // Weather grays the palette: rain at noon is not clear noon.
        assert_ne!(
            auto_at(12.0, Weather::Rain).accent,
            auto_at(12.0, Weather::Clear).accent
        );
        assert_eq!(list()[0], "auto");
        assert!(names().starts_with("auto"));
    }

    /// The sky changes with the hour, stars only show at night, and no hour
    /// gets bright enough to wash out the text drawn over it.
    #[test]
    fn sky_follows_the_clock_and_stays_dark() {
        for weather in WEATHERS {
            for step in 0..240 {
                let hour = step as f32 / 10.0;
                let sky = sky_at(hour, weather);
                for color in [sky.top, sky.horizon] {
                    let Color::Rgb(r, g, b) = color else {
                        panic!("sky must be rgb")
                    };
                    assert!(
                        r.max(g).max(b) <= 110,
                        "hour {hour} {weather:?} too bright"
                    );
                }
                assert!(sky.stars <= 100);
            }
        }
        let clear = |hour: f32| sky_at(hour, Weather::Clear);
        assert_ne!(clear(12.0).horizon, clear(0.0).horizon);
        assert_ne!(
            clear(18.0).horizon,
            clear(12.0).horizon,
            "sunset needs its own glow"
        );
        assert_eq!(clear(0.0).stars, 100);
        assert_eq!(clear(12.0).stars, 0);
    }

    /// Clouds hide the stars and the sun, rain darkens and grays the sky, and
    /// the sun only shows on a clear day, rising and setting with the clock.
    #[test]
    fn weather_bends_the_sky() {
        assert_eq!(sky_at(0.0, Weather::Cloudy).stars, 25);
        assert_eq!(sky_at(0.0, Weather::Rain).stars, 0);
        assert_ne!(
            sky_at(12.0, Weather::Rain).horizon,
            sky_at(12.0, Weather::Clear).horizon
        );
        // Unknown (no place saved) paints exactly the clock sky.
        assert_eq!(
            sky_at(9.0, Weather::Unknown).horizon,
            sky_at(9.0, Weather::Clear).horizon
        );
        assert!(sky_at(12.0, Weather::Clear).sun.is_some());
        assert!(sky_at(12.0, Weather::Cloudy).sun.is_none());
        assert!(sky_at(12.0, Weather::Rain).sun.is_none());
        assert!(sky_at(2.0, Weather::Clear).sun.is_none(), "no sun at night");
        let sun = |hour: f32| sky_at(hour, Weather::Clear).sun.expect("day");
        assert!(sun(7.0).x < sun(12.0).x && sun(12.0).x < sun(17.0).x);
        assert!(sun(12.0).y < sun(7.0).y, "the sun climbs toward noon");
        assert!(sun(12.0).strength > sun(6.5).strength);
    }

    #[test]
    fn names_resolve_case_insensitively() {
        assert_eq!(by_name("GRUVBOX"), by_name("gruvbox"));
        assert_eq!(by_name("tokyo"), by_name("Tokyo"));
        assert!(by_name("nope").is_none());
        assert!(set("dark"));
        assert_eq!(current_name(), "dark");
        // `auto` is a name with no palette behind it: it flips the clock
        // flag instead of the index, and the read that follows samples the
        // ramp. Kept here, the one test that owns the global, so no other
        // test can catch the flag mid-flip.
        assert!(set("auto"));
        assert_eq!(current_name(), "auto");
        assert!(matches!(current().muted, Color::Rgb(..)));
        assert!(sky().is_some());
        assert_eq!(weather::current(), Weather::Unknown);
        assert!(auto_redraw_due());
        assert!(!auto_redraw_due());
        assert!(set("dark"));
        assert_eq!(current_name(), "dark");
        assert!(sky().is_none());
        assert!(!auto_redraw_due());
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
