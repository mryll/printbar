//! Render a `PrinterState` into Waybar JSON: bar from a token template (with
//! literal-absorption on hidden tokens), themed tooltip, and a worst-state class.

use crate::color::ColorMode;
use crate::config::{OnMissing, PrinterConfig};
use crate::model::{Color, Level, PrinterState, Reason, Status, Supply, SupplyClass, SupplyKind};
use crate::palette;
use crate::theme::ThemeColors;
use crate::waybar::{pango_escape, strip_color_markup, visible_len, Paint, WaybarOutput};

const HIDDEN: char = '\u{0}'; // marker for a hidden token; words containing it are dropped

pub fn status_icon(s: Option<&Status>) -> &'static str {
    // Nerd Font (FontAwesome) glyphs via escapes so they persist in source.
    match s {
        Some(Status::Idle) => "\u{f02f}",     // printer
        Some(Status::Printing) => "\u{f02f}", // printer (active)
        Some(Status::Stopped) => "\u{f071}",  // warning triangle
        Some(Status::Offline) => "\u{f127}",  // broken link
        _ => "\u{f059}",                      // question circle
    }
}

fn status_text(s: Option<&Status>) -> &'static str {
    match s {
        Some(Status::Idle) => "Idle",
        Some(Status::Printing) => "Printing",
        Some(Status::Stopped) => "Stopped",
        Some(Status::Offline) => "Offline",
        _ => "Unknown",
    }
}

/// Human label + theme color for an active condition.
fn reason_display<'a>(r: &Reason, t: &'a ThemeColors) -> (String, &'a str) {
    match r {
        Reason::Jam => ("Paper jam".into(), &t.error),
        Reason::MediaEmpty => ("Out of paper".into(), &t.error),
        Reason::MediaLow => ("Paper low".into(), &t.orange),
        Reason::SupplyLow => ("Supply low".into(), &t.orange),
        Reason::SupplyEmpty => ("Supply empty".into(), &t.error),
        Reason::CoverOpen => ("Cover open".into(), &t.error),
        Reason::Offline => ("Offline".into(), &t.dim),
        Reason::Other(s) => (s.clone(), &t.orange),
    }
}

/// Effective "badness" percent for a supply: how close to empty (Consumed) or full (Filled).
/// Shared with the structured JSON mode so both frontends see the same thresholds.
pub fn supply_badness(s: &Supply) -> Option<u8> {
    s.level.as_pct().map(|p| match s.class {
        SupplyClass::Consumed => p,     // low = bad
        SupplyClass::Filled => 100 - p, // high = bad → invert to headroom
    })
}

fn worst_supply_badness(state: &PrinterState) -> Option<u8> {
    state.supplies.iter().filter_map(supply_badness).min()
}

/// Resolve a bar/tooltip token to its display value, or `None` if absent.
fn resolve(token: &str, state: &PrinterState) -> Option<String> {
    use crate::model::{Color, SupplyKind};
    let color_pct = |c: Color| {
        state
            .supplies
            .iter()
            .find(|s| s.color == Some(c))
            .and_then(|s| s.level.as_pct())
    };
    match token {
        "supply_min" => worst_supply_badness(state).map(|p| p.to_string()),
        "toner_min" => state
            .supplies
            .iter()
            .filter(|s| s.kind == SupplyKind::Toner)
            .filter_map(supply_badness)
            .min()
            .map(|p| p.to_string()),
        "ink_min" => state
            .supplies
            .iter()
            .filter(|s| s.kind == SupplyKind::Ink)
            .filter_map(supply_badness)
            .min()
            .map(|p| p.to_string()),
        "black" => color_pct(Color::Black).map(|p| p.to_string()),
        "cyan" => color_pct(Color::Cyan).map(|p| p.to_string()),
        "magenta" => color_pct(Color::Magenta).map(|p| p.to_string()),
        "yellow" => color_pct(Color::Yellow).map(|p| p.to_string()),
        "status" => state
            .status
            .as_ref()
            .map(|_| status_text(state.status.as_ref()).to_string()),
        "status_icon" => Some(status_icon(state.status.as_ref()).to_string()), // always present
        "model" => state.model.clone(),
        "name" => state.name.clone(),
        "jobs" => state.jobs.map(|j| j.to_string()),
        "pages" | "impressions" => state.pages.map(|p| p.to_string()),
        "paper" => state
            .paper
            .iter()
            .filter_map(|t| t.level.as_pct())
            .min()
            .map(|p| p.to_string()),
        _ => None,
    }
}

/// Substitute `{token}`s. Hidden tokens (Hide mode) become a marker, then any
/// whitespace-delimited word containing the marker is dropped, so `"{x}%"` leaves no
/// dangling `%`. Returns (text, had_error) where had_error is set when Error mode hit a miss.
///
/// Substituted VALUES are Pango-escaped — `{model}` and `{name}` come from the
/// printer, and Waybar renders `text` as markup, so a model called
/// `Smith & Sons <Lab>` would otherwise blank the bar. The format's own
/// literals are left alone: markup there is the user's, deliberately written.
fn render_template(fmt: &str, state: &PrinterState, on_missing: OnMissing) -> (String, bool) {
    let mut out = String::new();
    let mut had_error = false;
    let mut rest = fmt;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('}') {
            let token = &after[..end];
            match resolve(token, state) {
                Some(v) => out.push_str(&pango_escape(&v)),
                None => match on_missing {
                    OnMissing::Hide => out.push(HIDDEN),
                    OnMissing::Error => {
                        out.push_str("n/d");
                        had_error = true;
                    }
                },
            }
            rest = &after[end + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    // Drop words carrying the hidden marker; collapse whitespace.
    let cleaned = out
        .split_whitespace()
        .filter(|w| !w.contains(HIDDEN))
        .collect::<Vec<_>>()
        .join(" ");
    (cleaned, had_error)
}

/// Worst severity class. Order: ok < warn < critical < offline < error.
pub fn worst_class(state: &PrinterState, cfg: &PrinterConfig, template_error: bool) -> String {
    let mut rank = 0u8; // 0 ok,1 warn,2 critical,3 offline,4 error
    let bump = |r: &mut u8, v: u8| *r = (*r).max(v);

    if template_error {
        bump(&mut rank, 4);
    }
    if state.status == Some(Status::Offline) {
        bump(&mut rank, 3);
    }
    if state.status == Some(Status::Stopped) {
        bump(&mut rank, 2);
    }
    for r in &state.reasons {
        match r {
            Reason::Offline => bump(&mut rank, 3),
            Reason::Jam | Reason::MediaEmpty | Reason::SupplyEmpty | Reason::CoverOpen => {
                bump(&mut rank, 2)
            }
            Reason::MediaLow | Reason::SupplyLow => bump(&mut rank, 1),
            Reason::Other(_) => bump(&mut rank, 1),
        }
    }
    for s in &state.supplies {
        if let Some(b) = supply_badness(s) {
            if b <= cfg.thresholds.supply_critical {
                bump(&mut rank, 2);
            } else if b <= cfg.thresholds.supply_low {
                bump(&mut rank, 1);
            }
        }
    }
    match rank {
        4 => "error",
        3 => "offline",
        2 => "critical",
        1 => "warn",
        _ => "ok",
    }
    .to_string()
}

fn level_str(l: Level) -> String {
    match l {
        Level::Pct(p) => format!("{p}%"),
        Level::NoRestriction => "∞".into(),
        Level::Unknown => "?".into(),
        Level::SomeRemaining => "ok".into(),
    }
}

/// Threshold color for a supply row — the same stops the structured JSON
/// publishes, so both frontends change together.
fn supply_color<'a>(s: &Supply, cfg: &PrinterConfig, t: &'a ThemeColors) -> &'a str {
    palette::severity(palette::supply_state(supply_badness(s), &cfg.thresholds), t)
}

/// Sentinel row rendered as a full-width section separator.
const SEP: &str = "\u{1}sep";

/// Sentinel prefix for a meter row parked until the tooltip's width is known.
const METER: &str = "\u{1}meter";

/// A supply row waiting for its geometry.
///
/// The bar has to reach the tooltip's right edge, and that edge is the widest
/// TEXT row — which does not exist yet while the supplies are being built. So
/// the row keeps its painted pieces here and the width pass applies the
/// geometry afterwards. Every meter in one tooltip gets the SAME bar length:
/// they stack, so a reader compares them against each other, and a per-row
/// length would turn that comparison into a lie.
struct MeterRow {
    /// Everything left of the bar, already painted and padded.
    lead: String,
    /// Visible columns that `lead` occupies.
    lead_w: usize,
    color: String,
    supply: Supply,
    value: String,
    value_w: usize,
}

/// Short, alignable supply label: the colorant name, else the kind, else the full name.
fn supply_label(s: &Supply) -> String {
    let named = match s.color {
        Some(Color::Black) => Some("Black"),
        Some(Color::Cyan) => Some("Cyan"),
        Some(Color::Magenta) => Some("Magenta"),
        Some(Color::Yellow) => Some("Yellow"),
        Some(Color::TriColor) => Some("Tri-color"),
        Some(Color::Photo) => Some("Photo"),
        _ => None,
    };
    if let Some(n) = named {
        return n.into();
    }
    match s.kind {
        SupplyKind::Drum => "Drum".into(),
        SupplyKind::Waste => "Waste".into(),
        _ => s.name.clone(),
    }
}

/// Bar geometry, shared with the sibling widgets so a printbar tooltip and a
/// claudebar tooltip stacked in one bar read as one product rather than two.
///
/// The bar is no longer a fixed 20 cells: it stretches so the level lands on
/// the tooltip's right edge, the column the widest text row ends on. The floor
/// keeps a short tooltip readable; the ceiling stops one long row (a printer
/// model, a panel message) from stretching the bar into a ruler.
const BAR_MIN: usize = 20;
const BAR_MAX: usize = 48;
/// Smallest gap between the bar and the level that follows it.
const BAR_GAP: usize = 2;

/// The level bar, or `None` when the level is a sentinel (unknown/etc).
///
/// Five cells used to carry the whole 0..100 range through `div_ceil(20)`, which
/// rounded UP to the next fifth: 82% drew a completely full bar, indis-
/// tinguishable from 100%, and 46% drew 60%. Twenty cells at 5% each, rounding
/// down, means the bar can no longer claim more than the printer reported.
fn supply_cells(s: &Supply, cells: usize) -> Option<String> {
    s.level.as_pct().map(|p| {
        // A level that is not zero never draws an empty bar: 4% is the case a
        // reader most needs to see, and rounding it to nothing hides it.
        let raw = (p.min(100) as usize) * cells / 100;
        let filled = if p > 0 { raw.max(1) } else { 0 };
        (0..cells)
            .map(|i| if i < filled { '\u{2588}' } else { '\u{2591}' })
            .collect()
    })
}

/// Status dot color: green when healthy, orange for warnings, red for hard faults, dim offline.
fn status_dot<'a>(state: &PrinterState, t: &'a ThemeColors) -> &'a str {
    let hard = |r: &Reason| {
        matches!(
            r,
            Reason::Jam | Reason::MediaEmpty | Reason::SupplyEmpty | Reason::CoverOpen
        )
    };
    if state.status == Some(Status::Offline) || state.reasons.contains(&Reason::Offline) {
        &t.dim
    } else if state.status == Some(Status::Stopped) || state.reasons.iter().any(hard) {
        &t.error
    } else if !state.reasons.is_empty() {
        &t.orange
    } else {
        &t.green
    }
}

/// Build the themed tooltip from configured items. `p` decides whether
/// the theme colors are actually painted (see `--no-color`); everything
/// structural — glyphs, level cells, box drawing, alignment — is unaffected.
fn build_tooltip(state: &PrinterState, cfg: &PrinterConfig, t: &ThemeColors, p: Paint) -> String {
    let mut rows: Vec<String> = Vec::new();
    let mut meters: Vec<MeterRow> = Vec::new();
    let label = |k: &str| p.fg(&t.dim, k);

    for item in &cfg.tooltip.items {
        match item.as_str() {
            "model" => {
                if let Some(m) = &state.model {
                    rows.push(p.bold_fg(&t.accent, &pango_escape(m)));
                    rows.push(SEP.into());
                } else if cfg.tooltip.on_missing == OnMissing::Error {
                    rows.push(format!("{} {}", label("Model"), p.fg(&t.error, "n/d")));
                }
            }
            "status" if state.status.is_some() => {
                rows.push(format!(
                    "{} {}",
                    p.fg(status_dot(state, t), "●"),
                    p.fg(&t.text, status_text(state.status.as_ref()))
                ));
            }
            // The literal text on the printer's front panel ("Ready", "Paper jam in tray 2", ...).
            "display" => {
                if let Some(d) = &state.display {
                    rows.push(format!(
                        "{} {}",
                        label("Panel"),
                        p.fg(&t.accent, &pango_escape(d))
                    ));
                }
            }
            // Active conditions (jam, cover open, toner low, ...), colored by severity.
            "alerts" => {
                for r in &state.reasons {
                    let (txt, color) = reason_display(r, t);
                    rows.push(p.fg(color, &format!("\u{26a0} {}", pango_escape(&txt))));
                }
            }
            "supplies" => {
                let cap = cfg.tooltip.max_rows.max(1);
                let shown: Vec<&Supply> = state.supplies.iter().take(cap).collect();
                let label_w = shown
                    .iter()
                    .map(|s| supply_label(s).chars().count())
                    .max()
                    .unwrap_or(0)
                    .min(12);
                for s in &shown {
                    let lbl = supply_label(s);
                    let lpad = " ".repeat(label_w.saturating_sub(lbl.chars().count()));
                    let val = level_str(s.level);
                    let bar_col = supply_color(s, cfg, t);
                    // Parked, not rendered: the bar length is not known until
                    // every other row exists. Refer to MeterRow.
                    rows.push(format!("{METER}{}", meters.len()));
                    meters.push(MeterRow {
                        lead: format!(
                            "{} {}{}  ",
                            p.fg(palette::swatch_on_surface(s.color, t), "●"),
                            p.fg(&t.text, &pango_escape(&lbl)),
                            lpad
                        ),
                        // dot + space + label column + the two-space gap
                        lead_w: label_w + 4,
                        color: bar_col.to_string(),
                        supply: (*s).clone(),
                        value: p.fg(bar_col, &val),
                        value_w: val.chars().count(),
                    });
                }
                if state.supplies.len() > cap {
                    rows.push(p.fg(&t.dim, &format!("   +{} more", state.supplies.len() - cap)));
                }
            }
            "paper" => {
                let cap = cfg.tooltip.max_rows.max(1);
                let shown: Vec<&crate::model::InputTray> = state.paper.iter().take(cap).collect();
                let name_w = shown
                    .iter()
                    .map(|tr| tr.name.chars().count())
                    .max()
                    .unwrap_or(0)
                    .min(14);
                for tray in &shown {
                    let npad = " ".repeat(name_w.saturating_sub(tray.name.chars().count()));
                    rows.push(format!(
                        "{} {}{}  {}",
                        // nf-md-tray_full: a tray holding sheets. (U+F0A48,
                        // used before, is nf-md-exit_run — a person running
                        // through a doorway.)
                        label("\u{f1296}"),
                        p.fg(&t.dim, &pango_escape(&tray.name)),
                        npad,
                        p.fg(&t.text, &level_str(tray.level))
                    ));
                }
                if state.paper.len() > cap {
                    rows.push(p.fg(&t.dim, &format!("   +{} more", state.paper.len() - cap)));
                }
            }
            "jobs" => {
                if let Some(j) = state.jobs {
                    rows.push(format!(
                        "{} {}",
                        label("Jobs"),
                        p.fg(&t.text, &j.to_string())
                    ));
                }
            }
            "pages" | "impressions" => {
                if let Some(pages) = state.pages {
                    rows.push(format!(
                        "{} {}",
                        label("Impressions"),
                        p.fg(&t.text, &pages.to_string())
                    ));
                }
            }
            _ => {}
        }
    }
    // Drop a trailing separator (e.g. model header with nothing after it).
    while rows.last().map(|r| r.as_str()) == Some(SEP) {
        rows.pop();
    }
    if rows.is_empty() {
        rows.push(p.fg(&t.dim, "no data"));
    }

    // House freshness footer: a separator rule, then the clock glyph
    // (nf-md-clock_outline, U+F0150) and "Updated HH:MM" — the same closing line every
    // sibling widget uses, in the Omarchy shell panel and here alike. printbar
    // queries the printer on every invocation, so the stamp is the moment this
    // tooltip was built.
    rows.push(SEP.into());
    rows.push(p.fg(
        &t.dim,
        &format!(
            "\u{f0150}  Updated {}",
            chrono::Local::now().format("%H:%M")
        ),
    ));

    // The meters are skipped: their width is DERIVED from this number, so
    // measuring them here would be circular.
    let mut width = rows
        .iter()
        .filter(|r| r.as_str() != SEP && !r.starts_with(METER))
        .map(|r| visible_len(r))
        .max()
        .unwrap_or(0)
        .max(12);

    // One bar length for every meter in this tooltip. The lead column (dot +
    // label) is already uniform, and so is the level column, so the length
    // follows from the right edge alone.
    let lead_w = meters.iter().map(|m| m.lead_w).max().unwrap_or(0);
    let value_w = meters.iter().map(|m| m.value_w).max().unwrap_or(0);
    let bar_cells = width
        .saturating_sub(lead_w + BAR_GAP + value_w)
        .clamp(BAR_MIN, BAR_MAX);
    if !meters.is_empty() {
        // A clamp at the low end makes the meter row wider than every text
        // row, so the rules grow to match it and the right edge stays one
        // straight line.
        width = width.max(lead_w + bar_cells + BAR_GAP + value_w);
    }
    for r in rows.iter_mut() {
        let Some(idx) = r.strip_prefix(METER).and_then(|i| i.parse::<usize>().ok()) else {
            continue;
        };
        let m = &meters[idx];
        let cells = supply_cells(&m.supply, bar_cells).unwrap_or_else(|| " ".repeat(bar_cells));
        // The gap absorbs the level's own width, so the LAST character of the
        // level lands on the right edge whatever the level says.
        let pad = width.saturating_sub(m.lead_w + bar_cells + m.value_w).max(1);
        *r = format!(
            "{}{}{}{}",
            m.lead,
            p.fg(&m.color, &cells),
            " ".repeat(pad),
            m.value
        );
    }
    // One tooltip shape, pinned to a monospace font. The pin is not decoration:
    // the rules are made of ─, and in a proportional font that character is
    // nearly twice as wide as a letter — the tooltip then sizes itself to the
    // rules and grows a dead margin to the right of the text. Waybar draws the
    // tooltip in a GTK window that IGNORES font-family from CSS, so the markup
    // is the only place this can be said.
    let out: Vec<String> = rows
        .iter()
        .map(|r| {
            if r == SEP {
                p.fg(&t.dim, &"─".repeat(width))
            } else {
                r.clone()
            }
        })
        .collect();

    let body = out.join("\n");
    format!(
        "<span font_family='{}'>{body}</span>",
        pango_escape(&cfg.tooltip.tooltip_font).replace('\'', "&apos;")
    )
}

pub fn render(
    state: &PrinterState,
    cfg: &PrinterConfig,
    t: &ThemeColors,
    colors: ColorMode,
) -> WaybarOutput {
    let (text, err) = render_template(&cfg.bar.format, state, cfg.bar.on_missing);
    // printbar paints nothing in the bar itself — but `bar.format` is the user's
    // own string, so a monochrome bar has to be swept clean of any markup it
    // brought along. `class`/`alt` never change: they are the machine contract
    // that lets a monochrome user do the coloring from their own CSS.
    let text = if colors.bar {
        text
    } else {
        strip_color_markup(&text)
    };
    let class = worst_class(state, cfg, err);
    WaybarOutput {
        text,
        tooltip: build_tooltip(state, cfg, t, Paint::new(colors.tooltip)),
        alt: class.clone(),
        class: vec![class],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Color, SupplyKind};

    fn cfg() -> PrinterConfig {
        crate::config::Config::parse("[printer.x]\n")
            .unwrap()
            .printer
            .remove("x")
            .unwrap()
    }
    fn consumed(name: &str, color: Color, pct: u8) -> Supply {
        Supply {
            name: name.into(),
            kind: SupplyKind::Toner,
            class: SupplyClass::Consumed,
            color_raw: None,
            color: Some(color),
            level: Level::Pct(pct),
            max_capacity: None,
            unit: None,
        }
    }

    #[test]
    fn token_substitution_basic() {
        let mut st = PrinterState {
            status: Some(Status::Idle),
            ..Default::default()
        };
        st.supplies.push(consumed("Black", Color::Black, 54));
        let (text, _) = render_template("🖨 {supply_min}% {status_icon}", &st, OnMissing::Hide);
        assert_eq!(text, format!("🖨 54% {}", status_icon(Some(&Status::Idle))));
    }

    #[test]
    fn hidden_token_absorbs_adjacent_literal() {
        let st = PrinterState::default(); // no supplies
        let (text, err) = render_template("{supply_min}% ok", &st, OnMissing::Hide);
        assert_eq!(text, "ok");
        assert!(!err);
    }

    #[test]
    fn printer_strings_cannot_inject_markup_into_the_bar() {
        // Waybar renders `text` as Pango markup, and the model comes off the
        // wire. Unescaped, `&` alone is enough to blank the whole bar.
        let st = PrinterState {
            model: Some("Smith & Sons <Lab> \"A\"".into()),
            name: Some("a<b>c".into()),
            ..Default::default()
        };
        let (text, _) = render_template("{model}|{name}", &st, OnMissing::Hide);
        assert_eq!(text, "Smith &amp; Sons &lt;Lab&gt; \"A\"|a&lt;b&gt;c");
        assert!(!text.contains("<Lab>"));

        // The format's OWN markup is the user's and survives untouched.
        let (text, _) = render_template("<b>{model}</b>", &st, OnMissing::Hide);
        assert!(text.starts_with("<b>") && text.ends_with("</b>"));
    }

    #[test]
    fn missing_token_error_mode() {
        let st = PrinterState::default();
        let (text, err) = render_template("{supply_min}", &st, OnMissing::Error);
        assert_eq!(text, "n/d");
        assert!(err);
    }

    #[test]
    fn class_from_thresholds_consumed_vs_filled() {
        let mut c = cfg();
        c.thresholds.supply_low = 15;
        c.thresholds.supply_critical = 5;
        let mut consumed_low = PrinterState::default();
        consumed_low
            .supplies
            .push(consumed("Black", Color::Black, 4));
        assert_eq!(worst_class(&consumed_low, &c, false), "critical");

        let mut filled_high = PrinterState::default();
        filled_high.supplies.push(Supply {
            name: "Waste".into(),
            kind: SupplyKind::Waste,
            class: SupplyClass::Filled,
            color_raw: None,
            color: None,
            level: Level::Pct(96),
            max_capacity: None,
            unit: None,
        });
        assert_eq!(worst_class(&filled_high, &c, false), "critical"); // headroom 4 <= 5
    }

    #[test]
    fn tooltip_caps_rows() {
        let t = ThemeColors::default();
        let mut c = cfg();
        c.tooltip.items = vec!["supplies".into()];
        c.tooltip.max_rows = 12;
        c.tooltip.frame = true; // line count below includes the box borders
        let mut st = PrinterState::default();
        for i in 0..20 {
            st.supplies
                .push(consumed(&format!("S{i}"), Color::Other, 50));
        }
        let tip = build_tooltip(&st, &c, &t, Paint::new(true));
        assert!(tip.contains("+8 more"));
        // 12 supply rows + "+8 more" row → 13 data rows, then the freshness
        // footer (rule + "Updated HH:MM")
        assert_eq!(tip.lines().count(), 12 + 1 + 2);
    }

    #[test]
    fn the_tooltip_is_borderless_and_pinned() {
        let t = ThemeColors::default();
        let mut c = cfg();
        c.tooltip.items = vec!["supplies".into()];
        let mut st = PrinterState::default();
        st.supplies.push(consumed("Black", Color::Black, 50));
        let tip = build_tooltip(&st, &c, &t, Paint::new(true));
        assert!(!tip.contains('╭') && !tip.contains('╰') && !tip.contains('│'));
        // The pin is what keeps the rules the same width as the text they
        // underline; without it a proportional font stretches them.
        assert!(tip.contains("font_family="));
    }

    #[test]
    fn a_deprecated_frame_setting_changes_nothing() {
        let t = ThemeColors::default();
        let mut c = cfg();
        c.tooltip.items = vec!["supplies".into()];
        let mut st = PrinterState::default();
        st.supplies.push(consumed("Black", Color::Black, 50));
        let without = build_tooltip(&st, &c, &t, Paint::new(true));
        c.tooltip.frame = true; // still accepted, does nothing
        let with = build_tooltip(&st, &c, &t, Paint::new(true));
        assert_eq!(without, with);
    }

    // ---- monochrome mode (`--no-color`) ------------------------------------

    /// A printer with something to say on every tooltip row, and a bar format
    /// whose own markup the user wrote.
    fn colorful() -> (PrinterConfig, PrinterState) {
        let mut c = cfg();
        c.bar.format = "<span foreground='#ff0000'>\u{f042a}</span> {supply_min}%".into();
        c.tooltip.items = vec![
            "model".into(),
            "status".into(),
            "alerts".into(),
            "display".into(),
            "supplies".into(),
            "paper".into(),
            "jobs".into(),
            "impressions".into(),
        ];
        c.tooltip.frame = true;
        let mut st = PrinterState {
            model: Some("HP M477fdw".into()),
            status: Some(Status::Stopped),
            display: Some("Paper jam in tray 2".into()),
            jobs: Some(3),
            pages: Some(12345),
            ..Default::default()
        };
        st.reasons.push(Reason::Jam);
        st.supplies.push(consumed("Black", Color::Black, 4));
        st.supplies.push(consumed("Cyan", Color::Cyan, 64));
        st.paper.push(crate::model::InputTray {
            name: "Tray 2".into(),
            level: Level::Pct(80),
            max_capacity: None,
            empty: false,
        });
        (c, st)
    }

    fn has_color(s: &str) -> bool {
        s.contains("foreground") || s.contains('#')
    }

    #[test]
    fn each_of_the_four_states_paints_the_right_surface() {
        let t = ThemeColors::default();
        let (c, st) = colorful();
        let case = |m: ColorMode| render(&st, &c, &t, m);

        let full = case(ColorMode::FULL);
        assert!(has_color(&full.text) && has_color(&full.tooltip));

        let none = case(ColorMode::PLAIN_ALL);
        assert!(!has_color(&none.text) && !has_color(&none.tooltip));

        let plain_bar = case(ColorMode::PLAIN_BAR);
        assert!(!has_color(&plain_bar.text) && has_color(&plain_bar.tooltip));

        let plain_tip = case(ColorMode::PLAIN_TOOLTIP);
        assert!(has_color(&plain_tip.text) && !has_color(&plain_tip.tooltip));
    }

    #[test]
    fn monochrome_keeps_the_class_contract_and_the_data() {
        let t = ThemeColors::default();
        let (c, st) = colorful();
        let full = render(&st, &c, &t, ColorMode::FULL);
        let mono = render(&st, &c, &t, ColorMode::PLAIN_ALL);
        // The CSS classes are how a monochrome user styles the bar themselves.
        assert_eq!(mono.class, full.class);
        assert_eq!(mono.class, vec!["critical".to_string()]);
        assert_eq!(mono.alt, full.alt);
        // The bar keeps its glyph and its number, minus the user's color span.
        assert_eq!(mono.text, "\u{f042a} 4%");
    }

    #[test]
    fn monochrome_tooltip_keeps_every_structural_element() {
        let t = ThemeColors::default();
        let (c, st) = colorful();
        let full = render(&st, &c, &t, ColorMode::FULL);
        let mono = render(&st, &c, &t, ColorMode::PLAIN_ALL);
        // Same rows, same box, same glyphs, same words — only the hues are gone.
        assert_eq!(mono.tooltip.lines().count(), full.tooltip.lines().count());
        for needle in [
            "─", // the rules
            "●",
            "\u{26a0}",
            "\u{f1296}", // status dot, alert, tray glyphs
            "\u{2588}",
            "\u{2591}", // the level cells
            "Paper jam",
            "Paper jam in tray 2",
            "Stopped",
            "Black",
            "4%",
            "Tray 2",
            "12345",
        ] {
            assert!(
                mono.tooltip.contains(needle),
                "monochrome tooltip lost {needle:?}"
            );
        }
        // The model header keeps its weight, and the font pin survives.
        assert!(mono
            .tooltip
            .contains("<span font_weight='bold'>HP M477fdw</span>"));
        assert!(mono.tooltip.contains("font_family="));
        // Dropping color must not change any row's geometry.
        let full_widths: Vec<usize> = full.tooltip.lines().map(visible_len).collect();
        let mono_widths: Vec<usize> = mono.tooltip.lines().map(visible_len).collect();
        assert_eq!(full_widths, mono_widths);
    }
}
