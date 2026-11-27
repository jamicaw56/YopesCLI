#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn dry_air_under_a_rainy_code_does_not_rain() {
        // The forecast cell keeps the hour's rain code after the shower passed:
        // measured precipitation is what decides whether water falls.
        assert_eq!(Weather::from_report(61, 0.0), Weather::Cloudy);
        assert_eq!(Weather::from_report(95, 0.0), Weather::Cloudy);
        assert_eq!(Weather::from_report(73, 0.0), Weather::Cloudy);
        assert_eq!(Weather::from_report(61, 0.4), Weather::Rain);
        assert_eq!(Weather::from_report(95, 2.1), Weather::Storm);
        assert_eq!(Weather::from_report(73, 1.0), Weather::Snow);
        // A dry code stays what it says, whatever the precipitation.
        assert_eq!(Weather::from_report(0, 0.0), Weather::Clear);
        assert_eq!(Weather::from_report(3, 0.0), Weather::Cloudy);
        assert_eq!(Weather::from_report(45, 0.0), Weather::Fog);
    }

    #[test]
    fn wmo_codes_map_to_the_sky() {
        assert_eq!(Weather::from_code(0), Weather::Clear);
        assert_eq!(Weather::from_code(3), Weather::Cloudy);
        assert_eq!(Weather::from_code(45), Weather::Fog);
        assert_eq!(Weather::from_code(61), Weather::Rain);
        assert_eq!(Weather::from_code(81), Weather::Rain);
        assert_eq!(Weather::from_code(73), Weather::Snow);
        assert_eq!(Weather::from_code(95), Weather::Storm);
        assert_eq!(Weather::from_code(200), Weather::Unknown);
        // Every state survives the round trip through the atomic.
        for weather in ALL {
            assert_eq!(ALL[weather as usize], weather);
        }
        // Nothing is known until a place is saved and polled.
        assert_eq!(current(), Weather::Unknown);
    }

    #[test]
    fn only_wet_weather_drops_and_the_drops_fall() {
        let count = |weather: Weather, phase: usize| {
            (0..40)
                .flat_map(|y| (0..120).map(move |x| (x, y)))
                .filter(|&(x, y)| drop_at(weather, x, y, phase).is_some())
                .count()
        };
        assert_eq!(count(Weather::Clear, 0), 0);
        assert_eq!(count(Weather::Cloudy, 0), 0);
        assert_eq!(count(Weather::Unknown, 0), 0);
        let rain = count(Weather::Rain, 0);
        let storm = count(Weather::Storm, 0);
        assert!(rain > 40, "rain is too sparse: {rain}");
        assert!(storm > rain, "a storm is denser than rain");
        assert!(count(Weather::Snow, 0) > 20);
        // A drop moves: the frame one tick later is not the same frame.
        let frame = |phase: usize| {
            (0..40)
                .flat_map(|y| (0..120).map(move |x| (x, y)))
                .filter(|&(x, y)| drop_at(Weather::Rain, x, y, phase).is_some())
                .collect::<Vec<_>>()
        };
        assert_ne!(frame(0), frame(1));
    }

    /// A focus regain raises one flag the poller's wait loop consumes, and
    /// `stop` clears it so a later poller starts clean.
    #[test]
    fn a_focus_regain_asks_for_a_fresh_forecast() {
        use std::sync::atomic::Ordering;
        stop();
        assert!(!fetching());
        assert!(!REFRESH.swap(false, Ordering::Relaxed));
        refresh();
        assert!(REFRESH.swap(false, Ordering::Relaxed));
        refresh();
        stop();
        assert!(!REFRESH.swap(false, Ordering::Relaxed));
    }

    /// Live check against Open-Meteo, off by default so `cargo test` stays
    /// offline: `cargo test live_ -- --ignored`.
    #[test]
    #[ignore]
    fn live_lookup_and_forecast_answer() {
        let place = lookup("Paris").expect("geocode");
        assert!(place.name.starts_with("Paris"), "{place:?}");
        let code = fetch_report(place.lat, place.lon).expect("forecast");
        println!(
            "{} -> {code:?} -> {:?}",
            place.name,
            Weather::from_report(code.0, code.1)
        );
    }
}
