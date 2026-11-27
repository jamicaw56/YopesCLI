//! The selection animation every arrow-driven list shares: the chosen row's
//! text glows and a soft band slides along the row, so the cursor reads as lit
//! rather than boxed. The `/settings` popup walks the palette's own colors.

use super::*;

/// A slow pulse in 0..=100, used to breathe a color without a terminal blink.
pub(super) fn pulse(phase: usize, period: usize) -> usize {
    let period = period.max(2);
    let angle = (phase % period) as f32 / period as f32 * std::f32::consts::TAU;
    ((angle.sin() * 0.5 + 0.5) * 100.0) as usize
}

/// `color` lifted toward white by `amount` percent: the glow itself.
pub(super) fn lit(color: Color, amount: usize) -> Color {
    blend(color, Color::Rgb(255, 255, 255), amount.min(100))
}

/// Style of a chosen row's text: the accent, pulsing brighter.
pub(super) fn selection(base: Color, phase: usize) -> Style {
    Style::default()
        .fg(lit(base, 10 + pulse(phase, 30) / 4))
        .add_modifier(Modifier::BOLD)
}

/// The next color of the palette cycle, blended so the walk has no jump: the
/// "cool colors" the settings popup's selected row runs through.
pub(super) fn cycling(phase: usize) -> Color {
    let colors = [accent2(), code(), added(), accent()];
    let step = phase / 6;
    let from = colors[step % colors.len()];
    let to = colors[(step + 1) % colors.len()];
    blend(from, to, (phase % 6) * 100 / 6)
}

/// Slide a soft band of `glow` along one row: the highlight travels back and
/// forth across the row instead of sitting still on it.
pub(super) fn sweep(frame: &mut Frame, row: Rect, phase: usize, base: Color, glow: Color) {
    let width = usize::from(row.width);
    if width == 0 {
        return;
    }
    let band = (width / 2).clamp(3, 24);
    let travel = width.saturating_sub(band).max(1);
    let period = travel * 2;
    let step = phase % period;
    let start = if step <= travel { step } else { period - step };
    let buffer = frame.buffer_mut();
    for x in 0..width {
        let inside = x >= start && x < start + band;
        // The band fades out at its edges instead of ending on a hard cell.
        let depth = if inside {
            let rel = x - start;
            (rel.min(band - 1 - rel) + 1).min(4) * 25
        } else {
            0
        };
        let color = blend(base, glow, depth);
        buffer[(row.x + x as u16, row.y)].set_bg(color);
    }
}

/// Paint the moving band behind a chosen row with the active theme's colors,
/// over the backdrop, so the row stays readable on light themes too.
pub(super) fn highlight(frame: &mut Frame, row: Rect, phase: usize) {
    let theme = crate::theme::current();
    let base = backdrop_base(&theme);
    sweep(
        frame,
        row,
        phase,
        blend(base, theme.accent, 12),
        blend(base, theme.accent2, 46),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_stays_inside_its_range_and_moves() {
        for phase in 0..60 {
            assert!(pulse(phase, 30) <= 100);
        }
        assert_ne!(pulse(0, 30), pulse(7, 30));
    }

    #[test]
    fn the_cycle_walks_through_every_palette_color() {
        let seen: std::collections::BTreeSet<String> = (0..24)
            .map(|phase| format!("{:?}", cycling(phase)))
            .collect();
        assert!(seen.len() > 3, "the cycle should not sit on one color");
    }
}
