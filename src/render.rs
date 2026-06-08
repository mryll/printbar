//! Render a `PrinterState` into Waybar JSON: bar from a token template (with
//! literal-absorption on hidden tokens), framed/themed tooltip, and a worst-state class.

use crate::config::{OnMissing, PrinterConfig};
use crate::model::{Color, Level, PrinterState, Reason, Status, Supply, SupplyClass, SupplyKind};
use crate::theme::ThemeColors;
use crate::waybar::{
    bold_fg, bottom_border, fg, pango_escape, separator, top_border, visible_len, WaybarOutput,
};

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
fn supply_badness(s: &Supply) -> Option<u8> {
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
        "pages" => state.pages.map(|p| p.to_string()),
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
                Some(v) => out.push_str(&v),
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

fn supply_color<'a>(s: &Supply, cfg: &PrinterConfig, t: &'a ThemeColors) -> &'a str {
    match supply_badness(s) {
        Some(b) if b <= cfg.thresholds.supply_critical => &t.error,
        Some(b) if b <= cfg.thresholds.supply_low => &t.orange,
        Some(_) => &t.green,
        None => &t.dim,
    }
}

/// Sentinel row rendered as a full-width section separator.
const SEP: &str = "\u{1}sep";

/// A representative visible color for a supply's colorant (toner/ink identity swatch).
fn swatch_color(c: Option<Color>) -> &'static str {
    match c {
        Some(Color::Black) => "#c8c8c8",
        Some(Color::Cyan) => "#26c6da",
        Some(Color::Magenta) => "#d05ce3",
        Some(Color::Yellow) => "#fbc02d",
        Some(Color::TriColor) => "#9ccc65",
        Some(Color::Photo) => "#90a4ae",
        _ => "#8a8a8a",
    }
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

/// The 5-cell level bar, or `None` when the level is a sentinel (unknown/etc).
fn supply_cells(s: &Supply) -> Option<String> {
    s.level.as_pct().map(|p| {
        let filled = (p as usize).div_ceil(20);
        (0..5).map(|i| if i < filled { '▰' } else { '▱' }).collect()
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

/// Build the framed, themed tooltip from configured items.
fn build_tooltip(state: &PrinterState, cfg: &PrinterConfig, t: &ThemeColors) -> String {
    let mut rows: Vec<String> = Vec::new();
    let label = |k: &str| fg(&t.dim, k);

    for item in &cfg.tooltip.items {
        match item.as_str() {
            "model" => {
                if let Some(m) = &state.model {
                    rows.push(bold_fg(&t.accent, &pango_escape(m)));
                    rows.push(SEP.into());
                } else if cfg.tooltip.on_missing == OnMissing::Error {
                    rows.push(format!("{} {}", label("Model"), fg(&t.error, "n/d")));
                }
            }
            "status" if state.status.is_some() => {
                rows.push(format!(
                    "{} {}",
                    fg(status_dot(state, t), "●"),
                    fg(&t.text, status_text(state.status.as_ref()))
                ));
            }
            // The literal text on the printer's front panel ("Ready", "Paper jam in tray 2", ...).
            "display" => {
                if let Some(d) = &state.display {
                    rows.push(format!(
                        "{} {}",
                        label("Panel"),
                        fg(&t.accent, &pango_escape(d))
                    ));
                }
            }
            // Active conditions (jam, cover open, toner low, ...), colored by severity.
            "alerts" => {
                for r in &state.reasons {
                    let (txt, color) = reason_display(r, t);
                    rows.push(fg(color, &format!("\u{26a0} {}", pango_escape(&txt))));
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
                let val_w = shown
                    .iter()
                    .map(|s| level_str(s.level).chars().count())
                    .max()
                    .unwrap_or(0);
                for s in &shown {
                    let lbl = supply_label(s);
                    let lpad = " ".repeat(label_w.saturating_sub(lbl.chars().count()));
                    let val = level_str(s.level);
                    let vpad = " ".repeat(val_w.saturating_sub(val.chars().count()));
                    let bar_col = supply_color(s, cfg, t);
                    let cells = supply_cells(s).unwrap_or_else(|| "     ".into());
                    rows.push(format!(
                        "{} {}{}  {}  {}{}",
                        fg(swatch_color(s.color), "●"),
                        fg(&t.text, &pango_escape(&lbl)),
                        lpad,
                        fg(bar_col, &cells),
                        vpad,
                        fg(bar_col, &val)
                    ));
                }
                if state.supplies.len() > cap {
                    rows.push(fg(
                        &t.dim,
                        &format!("   +{} more", state.supplies.len() - cap),
                    ));
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
                        label("\u{f0a48}"), // tray glyph
                        fg(&t.dim, &pango_escape(&tray.name)),
                        npad,
                        fg(&t.text, &level_str(tray.level))
                    ));
                }
                if state.paper.len() > cap {
                    rows.push(fg(&t.dim, &format!("   +{} more", state.paper.len() - cap)));
                }
            }
            "jobs" => {
                if let Some(j) = state.jobs {
                    rows.push(format!("{} {}", label("Jobs"), fg(&t.text, &j.to_string())));
                }
            }
            "pages" => {
                if let Some(p) = state.pages {
                    rows.push(format!(
                        "{} {}",
                        label("Pages"),
                        fg(&t.text, &p.to_string())
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
        rows.push(fg(&t.dim, "no data"));
    }

    let width = rows
        .iter()
        .filter(|r| r.as_str() != SEP)
        .map(|r| visible_len(r))
        .max()
        .unwrap_or(0)
        .max(12);
    let mut out = vec![top_border(width, &t.border)];
    for r in &rows {
        if r == SEP {
            out.push(separator(width, &t.border, &t.dim));
        } else {
            let pad = " ".repeat(width.saturating_sub(visible_len(r)));
            out.push(format!(
                "{} {r}{pad} {}",
                fg(&t.border, "│"),
                fg(&t.border, "│")
            ));
        }
    }
    out.push(bottom_border(width, &t.border));
    out.join("\n")
}

pub fn render(state: &PrinterState, cfg: &PrinterConfig, t: &ThemeColors) -> WaybarOutput {
    let (text, err) = render_template(&cfg.bar.format, state, cfg.bar.on_missing);
    let class = worst_class(state, cfg, err);
    WaybarOutput {
        text,
        tooltip: build_tooltip(state, cfg, t),
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
        let mut st = PrinterState::default();
        for i in 0..20 {
            st.supplies
                .push(consumed(&format!("S{i}"), Color::Other, 50));
        }
        let tip = build_tooltip(&st, &c, &t);
        assert!(tip.contains("+8 more"));
        // 12 supply rows + "+8 more" row → 13 data rows, + 2 borders
        assert_eq!(tip.lines().count(), 12 + 1 + 2);
    }
}
