//! Weather for theme `auto`, strictly opt-in: nothing here talks to the
//! network until `/set weather <city>` saves a place.
//!
//! One background thread asks Open-Meteo (free, no key) for the current WMO
//! weather code every 15 minutes and parks the answer in an atomic, so every
//! draw call reads it lock-free. Only the saved coordinates are ever sent.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What the sky is doing, boiled down to what the backdrop can paint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Weather {
    /// No place saved, or no answer yet: `auto` paints the plain clock sky.
    #[default]
    Unknown,
    Clear,
    Cloudy,
    Fog,
    Rain,
    Snow,
    Storm,
}

const ALL: [Weather; 7] = [
    Weather::Unknown,
    Weather::Clear,
    Weather::Cloudy,
    Weather::Fog,
    Weather::Rain,
    Weather::Snow,
    Weather::Storm,
];

impl Weather {
    /// The WMO weather interpretation code Open-Meteo reports.
    fn from_code(code: u8) -> Self {
        match code {
            0 | 1 => Self::Clear,
            2 | 3 => Self::Cloudy,
            45 | 48 => Self::Fog,
            51..=67 | 80..=82 => Self::Rain,
            71..=77 | 85 | 86 => Self::Snow,
            95..=99 => Self::Storm,
            _ => Self::Unknown,
        }
    }

    /// The code together with the precipitation measured for the same moment.
    ///
    /// A forecast cell is coarse: it reports the rain of a whole grid box, and
    /// it keeps reporting the hour's code after the shower moved on. Dry air
    /// under a rainy code is therefore drawn as cloud, not as falling water,
    /// which is why the sky stops raining when the rain does.
    fn from_report(code: u8, precipitation: f64) -> Self {
        let weather = Self::from_code(code);
        if weather.wet() && precipitation <= 0.0 {
            return Self::Cloudy;
        }
        weather
    }

    /// Falling water: the weathers the backdrop animates.
    pub fn wet(self) -> bool {
        matches!(self, Self::Rain | Self::Snow | Self::Storm)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Clear => "clear",
            Self::Cloudy => "cloudy",
            Self::Fog => "fog",
            Self::Rain => "rain",
            Self::Snow => "snow",
            Self::Storm => "storm",
        }
    }
}

/// A place saved by `/set weather`: a name for display, coordinates for the
/// forecast request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

static STATE: AtomicU8 = AtomicU8::new(0);
/// Bumped by every `start` and `stop`, so a superseded poller notices and exits.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Raised when the terminal comes back: the poller drops its wait and fetches.
static REFRESH: AtomicBool = AtomicBool::new(false);
/// On while a request is in flight, so the frame with the answer gets drawn.
static FETCHING: AtomicBool = AtomicBool::new(false);

/// The last weather the poller saw.
pub fn current() -> Weather {
    ALL.get(usize::from(STATE.load(Ordering::Relaxed)))
        .copied()
        .unwrap_or_default()
}

/// Poll `place` every 15 minutes (a minute after a failed try), replacing any
/// earlier poller.
pub fn start(place: &Place) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let (lat, lon) = (place.lat, place.lon);
    thread::spawn(move || {
        while GENERATION.load(Ordering::Relaxed) == generation {
            FETCHING.store(true, Ordering::Relaxed);
            let wait = match fetch_report(lat, lon) {
                Ok((code, precipitation)) => {
                    // A poller replaced mid-request must not overwrite the new one.
                    if GENERATION.load(Ordering::Relaxed) == generation {
                        STATE.store(
                            Weather::from_report(code, precipitation) as u8,
                            Ordering::Relaxed,
                        );
                    }
                    900
                }
                Err(_) => 60,
            };
            FETCHING.store(false, Ordering::Relaxed);
            // One-second slices, so `stop` ends the thread within a second.
            for _ in 0..wait {
                if GENERATION.load(Ordering::Relaxed) != generation {
                    return;
                }
                // A focus regain cuts the wait short: the forecast is fetched
                // now instead of whenever the 15-minute wait happened to end.
                if REFRESH.swap(false, Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

/// Poll only while there is a saved place and theme `auto` is on, so no
/// coordinates leave the machine under any other theme.
pub fn follow(place: Option<&Place>, auto: bool) {
    match place {
        Some(place) if auto => start(place),
        _ => stop(),
    }
}

/// Stop polling and forget the weather: `auto` is the plain clock sky again.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
    STATE.store(Weather::Unknown as u8, Ordering::Relaxed);
    REFRESH.store(false, Ordering::Relaxed);
    FETCHING.store(false, Ordering::Relaxed);
}

/// Ask the running poller to fetch now, without restarting it. The terminal
/// coming back is when the sky on screen is most likely stale; with no poller
/// (no place, or not theme `auto`) this does nothing.
pub fn refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

/// Whether a forecast request is in flight. The ticker keeps drawing while it
/// is, so the answer shows up on the frame it lands instead of the next minute.
pub fn fetching() -> bool {
    FETCHING.load(Ordering::Relaxed)
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the weather client")
}

fn fetch_report(lat: f64, lon: f64) -> Result<(u8, f64)> {
    #[derive(Deserialize)]
    struct Reply {
        current: Current,
    }
    #[derive(Deserialize)]
    struct Current {
        weather_code: u8,
        #[serde(default)]
        precipitation: f64,
    }
    let reply: Reply = client()?
        .get(format!(
            "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
             &current=weather_code,precipitation"
        ))
        .send()
        .context("contacting Open-Meteo")?
        .error_for_status()
        .context("fetching the weather")?
        .json()
        .context("reading the weather")?;
    Ok((reply.current.weather_code, reply.current.precipitation))
}

/// Resolve a city name to a place through Open-Meteo's geocoder.
///
/// ponytail: blocks the caller for up to ~10s, on a rare typed command. Move
/// it behind an `Action` and a channel if that ever feels slow.
pub fn locate(city: &str) -> Result<Place> {
    let city = city.trim().to_owned();
    if city.is_empty() {
        bail!("no city given");
    }
    // The blocking client panics inside the async runtime, so it gets its own thread.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(lookup(&city));
    });
    receiver
        .recv_timeout(Duration::from_secs(10))
        .context("the lookup timed out")?
}

fn lookup(city: &str) -> Result<Place> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        results: Vec<Hit>,
    }
    #[derive(Deserialize)]
    struct Hit {
        name: String,
        latitude: f64,
        longitude: f64,
        #[serde(default)]
        country: String,
    }
    let reply: Reply = client()?
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query(&[("name", city), ("count", "1"), ("format", "json")])
        .send()
        .context("contacting the geocoder")?
        .error_for_status()
        .context("looking up the city")?
        .json()
        .context("reading the city")?;
    let Some(hit) = reply.results.into_iter().next() else {
        bail!("no such place");
    };
    let name = if hit.country.is_empty() {
        hit.name
    } else {
        format!("{}, {}", hit.name, hit.country)
    };
    Ok(Place {
        name,
        lat: hit.latitude,
        lon: hit.longitude,
    })
}

/// The precipitation glyph at one cell, if any. Pure in its inputs, so the
/// same frame is repeatable and a test can look at it: every column hashes to
/// its own stream of drops, and `phase` (the app's 100ms tick) slides each
/// stream down the screen.
pub fn drop_at(weather: Weather, x: usize, y: usize, phase: usize) -> Option<&'static str> {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 13;
    let (columns, period, fall, glyph) = match weather {
        // A storm is denser and falls faster than rain.
        Weather::Rain => (
            5,
            7 + (h >> 4) % 6,
            phase * (1 + (h >> 9) as usize % 2),
            "│",
        ),
        Weather::Storm => (
            3,
            6 + (h >> 4) % 5,
            phase * (2 + (h >> 9) as usize % 2),
            "│",
        ),
        // Snow drifts: one row per three ticks.
        Weather::Snow => (
            6,
            9 + (h >> 4) % 6,
            phase / 3,
            if h % 5 == 0 { "*" } else { "·" },
        ),
        _ => return None,
    };
    if h % columns != 0 {
        return None;
    }
    let lag = (y as isize - fall as isize - (h >> 12) as isize).rem_euclid(period as isize);
    (lag == 0).then_some(glyph)
}

#[cfg(test)]
#[path = "weather_tests.rs"]
mod extracted_tests;
