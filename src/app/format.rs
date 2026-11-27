use super::*;

pub(super) fn estimated_output_tokens(session: &Session) -> u64 {
    match session.messages.last() {
        Some(message) if message.role == Role::Assistant => {
            (message.content.chars().count() + message.reasoning.chars().count()) as u64 / 4
        }
        _ => 0,
    }
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else {
        format!("{:.1}k", tokens as f64 / 1000.0)
    }
}

/// A context size the way a window is written: `850`, `38.3K`, `128K`, `1M`.
///
/// The model's window is a round number in millions, so it earns a suffix
/// instead of five digits, and a `.0` is dropped: `1M`, not `1.0M`.
pub fn format_context(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 999_950 {
        format!("{:.1}K", tokens as f64 / 1_000.0).replace(".0K", "K")
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0).replace(".0M", "M")
    }
}

/// Dollars per million tokens, `(input, output)`, for the common models when
/// the provider catalog has no price. `None` means the price is unknown: the
/// session counter shows a dash instead of inventing a bill.
pub(super) fn known_model_price(model: &str) -> Option<(f64, f64)> {
    let model = model.to_ascii_lowercase();
    let table: [(&str, f64, f64); 8] = [
        ("gpt-4o-mini", 0.15, 0.60),
        ("gpt-4o", 2.50, 10.0),
        ("gpt-4.1", 2.00, 8.00),
        ("o3", 2.00, 8.00),
        ("o4-mini", 1.10, 4.40),
        ("claude-3-5-sonnet", 3.00, 15.0),
        ("claude-3-5-haiku", 0.80, 4.00),
        ("deepseek", 0.27, 1.10),
    ];
    // Longest match wins so `gpt-4o-mini` does not bill as `gpt-4o`.
    let mut best: Option<(&str, f64, f64)> = None;
    for (name, input, output) in table {
        if model.contains(name) && best.is_none_or(|(seen, _, _)| name.len() > seen.len()) {
            best = Some((name, input, output));
        }
    }
    best.map(|(_, input, output)| (input, output))
}

/// Session spend as `$0.0000`, priced from the active model. A model with no
/// known price yields an empty string — the header drops the segment rather
/// than printing a placeholder for a bill it cannot price; a free one shows
/// `free` rather than a fake bill.
pub fn format_cost(
    prompt_tokens: u64,
    completion_tokens: u64,
    price: Option<(f64, f64)>,
) -> String {
    let Some((input, output)) = price else {
        return String::new();
    };
    if input == 0.0 && output == 0.0 {
        return "free".to_owned();
    }
    let dollars = (prompt_tokens as f64 * input + completion_tokens as f64 * output) / 1_000_000.0;
    // Small sessions cost fractions of a cent, so keep four decimals until it
    // is worth showing cents.
    if dollars < 1.0 {
        format!("${dollars:.4}")
    } else {
        format!("${dollars:.2}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// A round trip as `340ms` or `1.2s`. Sub-second keeps milliseconds: the
/// difference between a 200ms and an 800ms RTT is the whole point.
pub fn format_rtt(millis: u64) -> String {
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.1}s", millis as f64 / 1000.0)
    }
}

/// Elapsed milliseconds as `12s`, `1m05s` or `1h02m`: compact enough for a
/// sidebar row.
pub fn format_duration_ms(millis: u64) -> String {
    let seconds = millis / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, format_cost, format_tokens};

    #[test]
    fn rtt_keeps_milliseconds_under_a_second() {
        use super::format_rtt;
        assert_eq!(format_rtt(1), "1ms");
        assert_eq!(format_rtt(340), "340ms");
        assert_eq!(format_rtt(999), "999ms");
        assert_eq!(format_rtt(1_240), "1.2s");
    }

    #[test]
    fn formats_keep_tokens_and_bytes_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1200), "1.2k");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2150), "2.1 KB");
    }

    #[test]
    fn context_sizes_read_like_a_window() {
        use super::format_context;
        assert_eq!(format_context(850), "850");
        assert_eq!(format_context(38_300), "38.3K");
        assert_eq!(format_context(128_000), "128K");
        // A rounding tail must not print `1000.0K`: it is a million.
        assert_eq!(format_context(999_950), "1M");
        assert_eq!(format_context(1_000_000), "1M");
        assert_eq!(format_context(2_000_000), "2M");
    }

    #[test]
    fn an_unpriced_model_has_no_cost_segment() {
        // No catalog or built-in price: nothing to show, not a `—`.
        assert_eq!(format_cost(1_000_000, 0, None), "");
        // A free model says so instead of `$0.0000`.
        assert_eq!(format_cost(1_000_000, 0, Some((0.0, 0.0))), "free");
        // A priced model keeps four decimals under a dollar.
        assert_eq!(format_cost(1_000_000, 0, Some((0.12, 0.3))), "$0.1200");
    }
}
