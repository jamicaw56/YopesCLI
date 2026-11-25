//! The global token ledger: what every yopes run on this machine has spent,
//! one row per local day, kept in `~/.yopes/usage.json`.
//!
//! The session already knows what it cost; the ledger is what makes `/stats`
//! answer "how much have I used this month" across sessions, workspaces and
//! restarts. Writes are best effort: a read-only home directory must never
//! fail a turn, so a failed save is dropped.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

/// One day of spend.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Day {
    #[serde(default)]
    pub prompt: u64,
    #[serde(default)]
    pub completion: u64,
    /// Requests that reported usage on that day.
    #[serde(default)]
    pub requests: u64,
}

impl Day {
    pub fn total(self) -> u64 {
        self.prompt.saturating_add(self.completion)
    }
}

/// Every day this machine has recorded, keyed `YYYY-MM-DD`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub days: BTreeMap<String, Day>,
}

impl Ledger {
    /// (in, out) over every recorded day.
    pub fn totals(&self) -> (u64, u64) {
        self.days
            .values()
            .fold((0, 0), |(prompt, completion), day| {
                (
                    prompt.saturating_add(day.prompt),
                    completion.saturating_add(day.completion),
                )
            })
    }

    pub fn requests(&self) -> u64 {
        self.days.values().map(|day| day.requests).sum()
    }

    /// Days with any tokens on them.
    pub fn active_days(&self) -> usize {
        self.days.values().filter(|day| day.total() > 0).count()
    }

    /// The busiest day's total, which the heatmap scales against.
    pub fn peak(&self) -> u64 {
        self.days.values().map(|day| day.total()).max().unwrap_or(0)
    }

    /// The last `weeks` weeks as columns of seven days, oldest week first, each
    /// column running Monday to Sunday. Missing days are zero, so a heatmap
    /// always has the same shape whatever the ledger holds.
    pub fn weeks(&self, weeks: usize, today: NaiveDate) -> Vec<Vec<Day>> {
        let weekday = today.weekday().num_days_from_monday() as i64;
        let last_monday = today - chrono::Duration::days(weekday);
        let first_monday = last_monday - chrono::Duration::days(7 * (weeks as i64 - 1));
        (0..weeks)
            .map(|week| {
                let start = first_monday + chrono::Duration::days(7 * week as i64);
                (0..7)
                    .map(|offset| self.day(start + chrono::Duration::days(offset)))
                    .collect()
            })
            .collect()
    }
}

impl Ledger {
    fn day(&self, date: NaiveDate) -> Day {
        self.days.get(&key(date)).copied().unwrap_or_default()
    }
}

fn key(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// `~/.yopes/usage.json`.
pub fn path() -> PathBuf {
    crate::config::yopes_home().join("usage.json")
}

/// Read the ledger; a missing or unreadable file is an empty ledger, never an
/// error, because the stats screen must always open.
pub fn load() -> Ledger {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Ledger {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Add one request's usage to today's row. Best effort: a failed write never
/// reaches the turn.
pub fn record(prompt: u64, completion: u64) {
    let path = path();
    let _ = record_at(&path, prompt, completion, Local::now().date_naive());
}

pub fn record_at(path: &Path, prompt: u64, completion: u64, day: NaiveDate) -> std::io::Result<()> {
    let mut ledger = load_from(path);
    let entry = ledger.days.entry(key(day)).or_default();
    entry.prompt = entry.prompt.saturating_add(prompt);
    entry.completion = entry.completion.saturating_add(completion);
    entry.requests = entry.requests.saturating_add(1);
    save_to(path, &ledger)
}

fn save_to(path: &Path, ledger: &Ledger) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(ledger).map_err(std::io::Error::other)?;
    // A rename keeps a half-written file from ever being read back.
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, json)?;
    fs::rename(&temporary, path)
}

/// A heatmap cell's intensity, 0 (nothing) to 4 (the peak day).
pub fn level(total: u64, peak: u64) -> u8 {
    if total == 0 || peak == 0 {
        return 0;
    }
    // Four steps over the log of the peak: a 10k day should not flatten a
    // quiet month into the same color.
    let scaled = (total.saturating_mul(4)).div_ceil(peak).clamp(1, 4);
    u8::try_from(scaled).unwrap_or(4)
}

/// The glyph and shade a level is drawn with.
pub fn cell(level: u8) -> &'static str {
    match level {
        0 => "·",
        1 => "░",
        2 => "▒",
        3 => "▓",
        _ => "█",
    }
}

/// How long a stats refresh may reuse what it read, so a repaint per frame
/// does not touch the disk.
pub const REFRESH: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).expect("date")
    }

    #[test]
    fn recording_adds_to_the_day_and_the_totals() {
        let dir = crate::temp::unique_dir("yopes-usage");
        let path = dir.join("usage.json");
        record_at(&path, 100, 20, date(24)).expect("first write");
        record_at(&path, 50, 5, date(24)).expect("second write");
        record_at(&path, 7, 3, date(25)).expect("next day");
        let ledger = load_from(&path);
        assert_eq!(
            ledger.days["2026-11-24"],
            Day {
                prompt: 150,
                completion: 25,
                requests: 2
            }
        );
        assert_eq!(ledger.totals(), (157, 28));
        assert_eq!(ledger.requests(), 3);
        assert_eq!(ledger.active_days(), 2);
        assert_eq!(ledger.peak(), 175);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_ledger_reads_as_empty() {
        let ledger = load_from(Path::new("C:/definitely/not/here/usage.json"));
        assert!(ledger.days.is_empty());
        assert_eq!(ledger.totals(), (0, 0));
        assert_eq!(ledger.peak(), 0);
    }

    #[test]
    fn weeks_always_hold_seven_days_and_end_on_today() {
        let mut ledger = Ledger::default();
        ledger.days.insert(
            "2026-11-25".to_owned(),
            Day {
                prompt: 5,
                completion: 0,
                requests: 1,
            },
        );
        // 2026-11-25 is a Wednesday: its column runs Monday the 23rd to Sunday.
        let weeks = ledger.weeks(3, date(25));
        assert_eq!(weeks.len(), 3);
        assert!(weeks.iter().all(|week| week.len() == 7));
        assert_eq!(weeks[2][2].total(), 5, "Wednesday of the current week");
        assert_eq!(weeks[2][0].total(), 0, "Monday of the current week");
    }

    #[test]
    fn levels_climb_with_spend() {
        assert_eq!(level(0, 100), 0);
        assert_eq!(level(1, 100), 1);
        assert_eq!(level(100, 100), 4);
        assert_eq!(level(10, 100), 1);
        assert!(level(50, 100) > level(10, 100));
        assert_eq!(level(5, 0), 0);
        assert_eq!(cell(0), "·");
        assert_eq!(cell(4), "█");
    }
}
