//! Structured JSON output (`--json`) for GUI frontends such as the Omarchy shell
//! plugin in `omarchy/`. Raw data and no rendering — no Pango markup, no
//! pre-rendered bars; the frontend lays it out.
//!
//! Severity thresholds AND the colors they resolve to are decided here and
//! published in `palette`, so the frontend consumes printbar's ramp rather than
//! keeping a second copy that drifts from it. `--no-color` never touches this
//! document: it describes the reading, not a rendering of it.
//!
//! Same contract as the Waybar mode: every path prints valid JSON and exits 0.

use serde::Serialize;

use crate::actions;
use crate::config::PrinterConfig;
use crate::model::{
    Color, InputTray, Level, PrinterState, Reason, Status, Supply, SupplyClass, SupplyKind,
};
use crate::palette;
use crate::render::{supply_badness, worst_class};
use crate::theme::ThemeColors;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Debug)]
pub struct JsonOutput {
    pub schema_version: u32,
    /// The config section this poll was run for (`printbar <printer> --json`).
    pub printer: String,
    pub name: Option<String>,
    pub model: Option<String>,
    /// `idle | printing | stopped | offline`, or null when no source reported one.
    pub status: Option<&'static str>,
    /// The literal text on the printer's front-panel display.
    pub display: Option<String>,
    /// Worst-state severity: `ok | warn | critical | offline | error`.
    pub state: String,
    /// Active conditions (jam, cover open, ...), each with its own severity state.
    pub reasons: Vec<JsonReason>,
    pub supplies: Vec<JsonSupply>,
    pub trays: Vec<JsonTray>,
    /// Queued jobs (count; the sources expose no per-job detail).
    pub jobs: Option<u32>,
    /// Lifetime impressions (page counter).
    pub impressions: Option<u64>,
    /// The printer's web panel (EWS), same URL `printbar action ews` opens.
    pub ews_url: Option<String>,
    /// The CUPS queue page, same URL `printbar action queue` opens.
    pub queue_url: Option<String>,
    /// The colors and threshold stops this poll resolved, so a frontend renders
    /// printbar's ramp instead of keeping its own copy. `null` on an error
    /// document, which carries no reading to color.
    pub palette: Option<JsonPalette>,
    /// Fatal error, when the poll could not run at all: `null | {message}`.
    pub error: Option<JsonError>,
}

/// The whole ramp: the colors AND the percentages they sit at. Published so a
/// threshold change in `config.toml`, a pywal-only machine or a themed install
/// all reach both frontends at once.
#[derive(Serialize, Debug)]
pub struct JsonPalette {
    /// Severity colors resolved from the active theme (Omarchy → pywal → One
    /// Dark). Keys match every `state` field in this document.
    pub severity: JsonSeverityColors,
    /// The physical colorant colors, as a filled swatch paints them. Not theme
    /// styling: cyan toner is cyan under every theme.
    pub ink: JsonInkColors,
    /// The supply gauge's ramp, ascending: at or below `pct`, a supply is in
    /// that `state` and paints in that `color`. The last stop is the open top
    /// of the scale. Derived from `[thresholds]` by the same code that decides
    /// each supply's `state`, so the two cannot disagree.
    pub stops: Vec<JsonStop>,
}

#[derive(Serialize, Debug)]
pub struct JsonStop {
    /// Upper bound of the band, inclusive.
    pub pct: u8,
    /// `critical | warn | ok`, the same vocabulary as a supply's `state`.
    pub state: &'static str,
    pub color: String,
}

#[derive(Serialize, Debug)]
pub struct JsonSeverityColors {
    pub ok: String,
    pub warn: String,
    pub critical: String,
    pub offline: String,
    pub error: String,
    /// A supply whose level the printer would not report.
    pub unknown: String,
}

#[derive(Serialize, Debug)]
pub struct JsonInkColors {
    pub black: &'static str,
    pub cyan: &'static str,
    pub magenta: &'static str,
    pub yellow: &'static str,
    #[serde(rename = "tri-color")]
    pub tri_color: &'static str,
    pub photo: &'static str,
    /// A colorant printbar could not name.
    pub other: &'static str,
}

#[derive(Serialize, Debug)]
pub struct JsonError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct JsonReason {
    /// Stable machine code: `jam | media-empty | media-low | supply-low |
    /// supply-empty | cover-open | offline | other`.
    pub code: &'static str,
    /// Human label (for `other`, the raw reason text).
    pub label: String,
    /// `warn | critical | offline`.
    pub state: &'static str,
}

#[derive(Serialize, Debug)]
pub struct JsonSupply {
    /// The supply's own name as reported by the printer.
    pub name: String,
    /// `toner | ink | drum | waste | other`.
    pub kind: &'static str,
    /// `consumed` (low = bad) or `filled` (high = bad, e.g. waste tank).
    pub class: &'static str,
    /// Semantic colorant name when recognized: `black | cyan | magenta | yellow |
    /// tri-color | photo`; null otherwise (see `color_raw`).
    pub color: Option<&'static str>,
    /// The raw colorant string the printer reported (may be a name or `#RRGGBB`).
    pub color_raw: Option<String>,
    /// Remaining/filled percent 0-100 when known.
    pub level_pct: Option<u8>,
    /// `percent | no-restriction | unknown | some-remaining` (RFC 3805 sentinels).
    pub level: &'static str,
    /// Threshold state for this supply: `ok | warn | critical | unknown`.
    pub state: &'static str,
}

#[derive(Serialize, Debug)]
pub struct JsonTray {
    pub name: String,
    pub level_pct: Option<u8>,
    /// Same vocabulary as supply `level`.
    pub level: &'static str,
    pub empty: bool,
}

impl JsonOutput {
    pub fn print(&self) {
        println!(
            "{}",
            serde_json::to_string(self)
                .unwrap_or_else(|_| error_output(&self.printer, "serialize"))
        );
    }
}

fn status_str(s: &Status) -> &'static str {
    match s {
        Status::Idle => "idle",
        Status::Printing => "printing",
        Status::Stopped => "stopped",
        Status::Offline => "offline",
    }
}

fn reason_json(r: &Reason) -> JsonReason {
    let (code, label, state): (&'static str, String, &'static str) = match r {
        Reason::Jam => ("jam", "Paper jam".into(), "critical"),
        Reason::MediaEmpty => ("media-empty", "Out of paper".into(), "critical"),
        Reason::MediaLow => ("media-low", "Paper low".into(), "warn"),
        Reason::SupplyLow => ("supply-low", "Supply low".into(), "warn"),
        Reason::SupplyEmpty => ("supply-empty", "Supply empty".into(), "critical"),
        Reason::CoverOpen => ("cover-open", "Cover open".into(), "critical"),
        Reason::Offline => ("offline", "Offline".into(), "offline"),
        Reason::Other(s) => ("other", s.clone(), "warn"),
    };
    JsonReason { code, label, state }
}

fn kind_str(k: SupplyKind) -> &'static str {
    match k {
        SupplyKind::Toner => "toner",
        SupplyKind::Ink => "ink",
        SupplyKind::Drum => "drum",
        SupplyKind::Waste => "waste",
        SupplyKind::Other => "other",
    }
}

fn color_str(c: Option<Color>) -> Option<&'static str> {
    match c {
        Some(Color::Black) => Some("black"),
        Some(Color::Cyan) => Some("cyan"),
        Some(Color::Magenta) => Some("magenta"),
        Some(Color::Yellow) => Some("yellow"),
        Some(Color::TriColor) => Some("tri-color"),
        Some(Color::Photo) => Some("photo"),
        Some(Color::Other) | None => None,
    }
}

fn level_str(l: Level) -> &'static str {
    match l {
        Level::Pct(_) => "percent",
        Level::NoRestriction => "no-restriction",
        Level::Unknown => "unknown",
        Level::SomeRemaining => "some-remaining",
    }
}

fn palette_json(cfg: &PrinterConfig, t: &ThemeColors) -> JsonPalette {
    let sev = |state: &str| palette::severity(state, t).to_string();
    JsonPalette {
        severity: JsonSeverityColors {
            ok: sev("ok"),
            warn: sev("warn"),
            critical: sev("critical"),
            offline: sev("offline"),
            error: sev("error"),
            unknown: sev("unknown"),
        },
        ink: JsonInkColors {
            black: palette::ink(Some(Color::Black)),
            cyan: palette::ink(Some(Color::Cyan)),
            magenta: palette::ink(Some(Color::Magenta)),
            yellow: palette::ink(Some(Color::Yellow)),
            tri_color: palette::ink(Some(Color::TriColor)),
            photo: palette::ink(Some(Color::Photo)),
            other: palette::INK_UNKNOWN,
        },
        // Straight from the classifier's own definition — never re-listed here.
        stops: palette::supply_stops(&cfg.thresholds)
            .into_iter()
            .map(|(pct, state)| JsonStop {
                pct,
                state,
                color: palette::severity(state, t).to_string(),
            })
            .collect(),
    }
}

fn supply_json(s: &Supply, cfg: &PrinterConfig) -> JsonSupply {
    let state = palette::supply_state(supply_badness(s), &cfg.thresholds);
    JsonSupply {
        name: s.name.clone(),
        kind: kind_str(s.kind),
        class: match s.class {
            SupplyClass::Consumed => "consumed",
            SupplyClass::Filled => "filled",
        },
        color: color_str(s.color),
        color_raw: s.color_raw.clone(),
        level_pct: s.level.as_pct(),
        level: level_str(s.level),
        state,
    }
}

fn tray_json(t: &InputTray) -> JsonTray {
    JsonTray {
        name: t.name.clone(),
        level_pct: t.level.as_pct(),
        level: level_str(t.level),
        empty: t.empty,
    }
}

/// Build the structured view of a merged printer state.
pub fn render(
    printer: &str,
    state: &PrinterState,
    cfg: &PrinterConfig,
    t: &ThemeColors,
) -> JsonOutput {
    JsonOutput {
        schema_version: SCHEMA_VERSION,
        printer: printer.to_string(),
        name: state.name.clone(),
        model: state.model.clone(),
        status: state.status.as_ref().map(status_str),
        display: state.display.clone(),
        state: worst_class(state, cfg, false),
        reasons: state.reasons.iter().map(reason_json).collect(),
        supplies: state.supplies.iter().map(|s| supply_json(s, cfg)).collect(),
        trays: state.paper.iter().map(tray_json).collect(),
        jobs: state.jobs,
        impressions: state.pages,
        ews_url: actions::ews_url(cfg).ok(),
        queue_url: actions::queue_url(cfg).ok(),
        palette: Some(palette_json(cfg, t)),
        error: None,
    }
}

/// Valid structured JSON for any fatal error — keeps the exit-0 contract.
pub fn error_output(printer: &str, reason: &str) -> String {
    let out = JsonOutput {
        schema_version: SCHEMA_VERSION,
        printer: printer.to_string(),
        name: None,
        model: None,
        status: None,
        display: None,
        state: "error".into(),
        reasons: Vec::new(),
        supplies: Vec::new(),
        trays: Vec::new(),
        jobs: None,
        impressions: None,
        ews_url: None,
        queue_url: None,
        // An error document carries no reading, so it carries no palette; the
        // panel keeps the last good one (and its last good data with it).
        palette: None,
        error: Some(JsonError {
            message: reason.to_string(),
            code: None,
        }),
    };
    serde_json::to_string(&out).unwrap_or_else(|_| {
        format!(
            r#"{{"schema_version":{SCHEMA_VERSION},"printer":"","state":"error","reasons":[],"supplies":[],"trays":[],"error":{{"message":"error"}}}}"#
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg(toml: &str) -> PrinterConfig {
        Config::parse(toml).unwrap().printer.remove("x").unwrap()
    }

    fn supply(name: &str, color: Option<Color>, level: Level) -> Supply {
        Supply {
            name: name.into(),
            kind: SupplyKind::Toner,
            class: SupplyClass::Consumed,
            color_raw: None,
            color,
            level,
            max_capacity: None,
            unit: None,
        }
    }

    #[test]
    fn full_state_round_trips_as_raw_data() {
        let c = cfg("[printer.x]\nhost=\"192.0.2.70\"\ncups=\"Queue\"\n");
        let mut st = PrinterState {
            name: Some("office".into()),
            model: Some("HP M477fdw".into()),
            status: Some(Status::Printing),
            display: Some("Printing document".into()),
            jobs: Some(2),
            pages: Some(12345),
            ..Default::default()
        };
        st.supplies.push(supply(
            "Black Cartridge",
            Some(Color::Black),
            Level::Pct(54),
        ));
        st.reasons.push(Reason::SupplyLow);
        st.paper.push(InputTray {
            name: "Tray 2".into(),
            level: Level::Pct(80),
            max_capacity: Some(250),
            empty: false,
        });

        let out = render("office", &st, &c, &ThemeColors::default());
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&out).unwrap()).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["printer"], "office");
        assert_eq!(v["status"], "printing");
        assert_eq!(v["display"], "Printing document");
        assert_eq!(v["state"], "warn"); // supply-low reason
        assert_eq!(v["reasons"][0]["code"], "supply-low");
        assert_eq!(v["reasons"][0]["state"], "warn");
        assert_eq!(v["supplies"][0]["color"], "black");
        assert_eq!(v["supplies"][0]["level_pct"], 54);
        assert_eq!(v["supplies"][0]["level"], "percent");
        assert_eq!(v["supplies"][0]["state"], "ok");
        assert_eq!(v["trays"][0]["name"], "Tray 2");
        assert_eq!(v["trays"][0]["level_pct"], 80);
        assert_eq!(v["jobs"], 2);
        assert_eq!(v["impressions"], 12345);
        assert_eq!(v["ews_url"], "http://192.0.2.70");
        assert_eq!(v["queue_url"], "http://localhost:631/printers/Queue");
        assert_eq!(v["error"], serde_json::Value::Null);
        // Raw data only: no markup anywhere in the payload.
        assert!(!serde_json::to_string(&out).unwrap().contains("<span"));
    }

    #[test]
    fn palette_publishes_the_whole_ramp_from_theme_and_thresholds() {
        let c = cfg("[printer.x]\n[printer.x.thresholds]\nsupply_low=20\nsupply_critical=8\n");
        let t = ThemeColors {
            green: "#00ff00".into(),
            orange: "#ff8800".into(),
            error: "#ff0000".into(),
            dim: "#666666".into(),
            ..ThemeColors::default()
        };
        let v: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&render("x", &PrinterState::default(), &c, &t)).unwrap(),
        )
        .unwrap();

        // Severity colors come from the resolved theme — which is how a
        // pywal-only machine reaches the panel at all.
        assert_eq!(v["palette"]["severity"]["ok"], "#00ff00");
        assert_eq!(v["palette"]["severity"]["warn"], "#ff8800");
        assert_eq!(v["palette"]["severity"]["critical"], "#ff0000");
        assert_eq!(v["palette"]["severity"]["error"], "#ff0000");
        assert_eq!(v["palette"]["severity"]["offline"], "#666666");
        assert_eq!(v["palette"]["severity"]["unknown"], "#666666");

        // The ramp travels as stops: colour AND the percent each sits at, in
        // ascending order, so a config change moves both frontends at once.
        assert_eq!(
            v["palette"]["stops"],
            serde_json::json!([
                { "pct": 8,   "state": "critical", "color": "#ff0000" },
                { "pct": 20,  "state": "warn",     "color": "#ff8800" },
                { "pct": 100, "state": "ok",       "color": "#00ff00" },
            ]),
            "published stops must be the configured thresholds, paired with \
             the theme colors, in the order the classifier tries them"
        );

        // And a consumer rendering that ramp must land on the same verdict the
        // core hands out per supply — the check that keeps the published ramp
        // from decaying into a second, drifting copy.
        let stops = v["palette"]["stops"].as_array().unwrap().clone();
        for level in 0u8..=100 {
            let from_stops = stops
                .iter()
                .find(|s| u64::from(level) <= s["pct"].as_u64().unwrap())
                .map_or("ok", |s| s["state"].as_str().unwrap());
            let mut one = PrinterState::default();
            one.supplies.push(supply("S", None, Level::Pct(level)));
            let out = render("x", &one, &c, &t);
            assert_eq!(
                from_stops, out.supplies[0].state,
                "published ramp disagrees with the supply verdict at {level}%"
            );
        }

        // Ink is colorant identity, not theme styling: unchanged by the theme.
        assert_eq!(v["palette"]["ink"]["cyan"], "#26c6da");
        assert_eq!(v["palette"]["ink"]["black"], "#262626");
        assert_eq!(v["palette"]["ink"]["tri-color"], "#9ccc65");
        assert_eq!(v["palette"]["ink"]["other"], "#8a8a8a");
        // Every colorant `color_str` can name has an ink entry to match.
        for key in ["black", "cyan", "magenta", "yellow", "tri-color", "photo"] {
            assert!(
                v["palette"]["ink"][key].is_string(),
                "no published ink for {key}"
            );
        }
    }

    #[test]
    fn an_error_document_publishes_no_palette() {
        // It carries no reading, so there is nothing to color; the panel keeps
        // the last good palette along with the last good data.
        let v: serde_json::Value = serde_json::from_str(&error_output("office", "boom")).unwrap();
        assert_eq!(v["palette"], serde_json::Value::Null);
    }

    #[test]
    fn supply_state_uses_core_thresholds() {
        let c = cfg("[printer.x]\n[printer.x.thresholds]\nsupply_low=15\nsupply_critical=5\n");
        let st = PrinterState {
            supplies: vec![
                supply("A", Some(Color::Cyan), Level::Pct(4)),
                supply("B", Some(Color::Magenta), Level::Pct(10)),
                supply("C", Some(Color::Yellow), Level::Pct(60)),
                supply("D", None, Level::Unknown),
            ],
            ..Default::default()
        };
        let out = render("x", &st, &c, &ThemeColors::default());
        let states: Vec<&str> = out.supplies.iter().map(|s| s.state).collect();
        assert_eq!(states, vec!["critical", "warn", "ok", "unknown"]);
        assert_eq!(out.supplies[3].level_pct, None);
        assert_eq!(out.supplies[3].level, "unknown");
        assert_eq!(out.state, "critical");
    }

    #[test]
    fn error_output_is_valid_json_with_error_state() {
        let s = error_output("office", "config parse: boom");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["printer"], "office");
        assert_eq!(v["state"], "error");
        assert_eq!(v["error"]["message"], "config parse: boom");
        assert!(v["error"].get("code").is_none()); // code omitted when absent
        assert_eq!(v["supplies"], serde_json::json!([]));
    }

    #[test]
    fn urls_absent_without_host_or_queue() {
        let c = cfg("[printer.x]\ncups=\"Q\"\n"); // USB-only: no host
        let out = render("x", &PrinterState::default(), &c, &ThemeColors::default());
        assert_eq!(out.ews_url, None); // ews needs a host (or explicit ews_url)
        assert_eq!(
            out.queue_url.as_deref(),
            Some("http://localhost:631/printers/Q")
        );
    }
}
