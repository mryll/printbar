//! The colors printbar itself defines, and the thresholds they sit at.
//!
//! Two kinds of color live here:
//!
//! * **severity** — resolved from the active theme, so a pywal-only machine
//!   gets its own ramp rather than One Dark's.
//! * **ink** — the physical colorant a cartridge holds. These are NOT theme
//!   styling: cyan toner is cyan under every theme, and the swatch depicts the
//!   thing in the printer.
//!
//! Both, plus the threshold stops the ramp sits on, are published in the
//! structured JSON (`palette`) so the Quickshell panel consumes them instead of
//! keeping a second copy that drifts. Changing a colorant color or a threshold
//! here moves both frontends at once.

use crate::config::Thresholds;
use crate::model::Color;
use crate::theme::ThemeColors;

/// The physical colorant colors, as a filled swatch should paint them.
///
/// Black is the true near-black of toner on paper. A surface that draws the
/// swatch as an outlined, filled shape (the Quickshell panel) can use it
/// directly; a surface that draws an unoutlined glyph (the Waybar tooltip)
/// must substitute a color that is legible on its own background — see
/// [`swatch_on_surface`].
pub fn ink(c: Option<Color>) -> &'static str {
    match c {
        Some(Color::Black) => "#262626",
        Some(Color::Cyan) => "#26c6da",
        Some(Color::Magenta) => "#d05ce3",
        Some(Color::Yellow) => "#fbc02d",
        Some(Color::TriColor) => "#9ccc65",
        Some(Color::Photo) => "#90a4ae",
        Some(Color::Other) | None => INK_UNKNOWN,
    }
}

/// Sentinel for a colorant printbar could not name. Published as `ink.other`
/// so a frontend can tell "no colorant" apart from a real one.
pub const INK_UNKNOWN: &str = "#8a8a8a";

/// The colorant swatch as a bare glyph must paint it on a themed surface.
///
/// Black ink and an unnamed colorant are the two cases where the true ink is
/// not reliably visible: a `●` in `#262626` disappears on a dark tooltip, and
/// `#8a8a8a` is muddy on both. Both defer to a theme color that is legible on
/// the surface by construction — text for black (ink on paper reads as the
/// text color does), dim for unknown. Every saturated colorant paints as
/// itself, on every theme.
pub fn swatch_on_surface(c: Option<Color>, t: &ThemeColors) -> &str {
    match c {
        Some(Color::Black) => &t.text,
        Some(Color::Other) | None => &t.dim,
        other => ink(other),
    }
}

/// Severity color for a threshold state, from the active theme.
pub fn severity<'a>(state: &str, t: &'a ThemeColors) -> &'a str {
    match state {
        "critical" | "error" => &t.error,
        "warn" => &t.orange,
        "offline" | "unknown" => &t.dim,
        _ => &t.green,
    }
}

/// The supply gauge's ramp, ascending: at or below `pct`, a supply is in that
/// state. The last stop is the open top of the scale.
///
/// This is the ONE definition of where the bands change. It is what the
/// structured JSON publishes as `palette.stops` and what [`supply_state`]
/// classifies against, so a published stop cannot drift from the verdict the
/// same code hands out — there is no second copy to drift from.
pub fn supply_stops(th: &Thresholds) -> [(u8, &'static str); 3] {
    [
        (th.supply_critical, "critical"),
        (th.supply_low, "warn"),
        (100, "ok"),
    ]
}

/// The threshold state of a supply, given its badness percent.
///
/// The single source for both frontends' ramps: the Waybar tooltip colors a
/// row with it, the structured JSON publishes it per supply, and the panel
/// renders its meter outline from it.
pub fn supply_state(badness: Option<u8>, th: &Thresholds) -> &'static str {
    let Some(b) = badness else {
        return "unknown";
    };
    supply_stops(th)
        .into_iter()
        .find(|(pct, _)| b <= *pct)
        .map_or("ok", |(_, state)| state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> Thresholds {
        Thresholds {
            supply_low: 15,
            supply_critical: 5,
        }
    }

    #[test]
    fn stops_are_inclusive_and_ordered() {
        let th = thresholds();
        assert_eq!(supply_state(Some(0), &th), "critical");
        assert_eq!(supply_state(Some(5), &th), "critical"); // inclusive
        assert_eq!(supply_state(Some(6), &th), "warn");
        assert_eq!(supply_state(Some(15), &th), "warn"); // inclusive
        assert_eq!(supply_state(Some(16), &th), "ok");
        assert_eq!(supply_state(None, &th), "unknown");
    }

    #[test]
    fn the_published_stops_are_the_configured_thresholds() {
        // Guards the derivation against decaying into a second copy: the stop
        // positions must BE the thresholds, not numbers that happen to match.
        for (low, critical) in [(15u8, 5u8), (30, 10), (100, 0), (8, 8)] {
            let th = Thresholds {
                supply_low: low,
                supply_critical: critical,
            };
            let stops = supply_stops(&th);
            assert_eq!(stops[0], (critical, "critical"));
            assert_eq!(stops[1], (low, "warn"));
            assert_eq!(stops[2], (100, "ok"));
            // Ascending, so "first stop at or above the level" is well defined.
            assert!(stops[1].0 <= stops[2].0);
        }
    }

    #[test]
    fn classifying_against_the_published_stops_reproduces_the_verdict() {
        // A consumer that renders the published ramp must land on exactly the
        // state the core hands out for the same level, at every percent.
        let th = thresholds();
        let stops = supply_stops(&th);
        for level in 0u8..=100 {
            let from_stops = stops
                .iter()
                .find(|(pct, _)| level <= *pct)
                .map_or("ok", |(_, state)| *state);
            assert_eq!(
                from_stops,
                supply_state(Some(level), &th),
                "stops and supply_state disagree at {level}%"
            );
        }
    }

    #[test]
    fn severity_colors_come_from_the_theme_not_constants() {
        let t = ThemeColors {
            green: "#111111".into(),
            orange: "#222222".into(),
            error: "#333333".into(),
            dim: "#444444".into(),
            ..ThemeColors::default()
        };
        assert_eq!(severity("ok", &t), "#111111");
        assert_eq!(severity("warn", &t), "#222222");
        assert_eq!(severity("critical", &t), "#333333");
        assert_eq!(severity("error", &t), "#333333");
        assert_eq!(severity("offline", &t), "#444444");
        assert_eq!(severity("unknown", &t), "#444444");
    }

    #[test]
    fn saturated_inks_paint_as_themselves_on_every_surface() {
        let t = ThemeColors::default();
        for c in [Color::Cyan, Color::Magenta, Color::Yellow, Color::TriColor] {
            assert_eq!(swatch_on_surface(Some(c), &t), ink(Some(c)));
        }
    }

    #[test]
    fn black_and_unknown_defer_to_theme_colors_on_a_glyph_surface() {
        let t = ThemeColors::default();
        // The published ink stays the true colorant...
        assert_eq!(ink(Some(Color::Black)), "#262626");
        assert_eq!(ink(Some(Color::Other)), INK_UNKNOWN);
        // ...while an unoutlined glyph uses a color legible on the surface.
        assert_eq!(swatch_on_surface(Some(Color::Black), &t), t.text);
        assert_eq!(swatch_on_surface(Some(Color::Other), &t), t.dim);
        assert_eq!(swatch_on_surface(None, &t), t.dim);
    }
}
