//! TOML configuration: one `[printer.<name>]` section per printer. The binary
//! is invoked as `printbar <name>` and looks up that section.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub printer: HashMap<String, PrinterConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrinterConfig {
    pub host: Option<String>,
    #[serde(default = "default_ipp_path")]
    pub ipp_path: String,
    pub cups: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub snmp: SnmpCfg,
    #[serde(default)]
    pub bar: BarCfg,
    #[serde(default)]
    pub tooltip: TooltipCfg,
    #[serde(default)]
    pub thresholds: Thresholds,
    #[serde(default)]
    pub actions: ActionsCfg,
    #[serde(default)]
    pub notify: NotifyCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnmpCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_community")]
    pub community: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BarCfg {
    #[serde(default = "default_bar_format")]
    pub format: String,
    #[serde(default)]
    pub on_missing: OnMissing,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TooltipCfg {
    #[serde(default = "default_tooltip_items")]
    pub items: Vec<String>,
    #[serde(default)]
    pub on_missing: OnMissing,
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
    /// Draw the framed tooltip box and pin `JetBrainsMono Nerd Font Mono` so rows
    /// stay aligned under any bar font. Off (default) = plain, borderless, no font
    /// pin — renders in the user's font; needs no specific font installed.
    #[serde(default)]
    pub frame: bool,
    /// Font family pinned in framed mode — must be a complete Mono Nerd Font.
    #[serde(default = "default_frame_font")]
    pub frame_font: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Thresholds {
    #[serde(default = "default_low")]
    pub supply_low: u8,
    #[serde(default = "default_critical")]
    pub supply_critical: u8,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActionsCfg {
    // on_click/on_click_right are consumed by the Waybar module config, not the binary.
    #[allow(dead_code)]
    pub on_click: Option<String>,
    #[allow(dead_code)]
    pub on_click_right: Option<String>,
    pub ews_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct NotifyCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnMissing {
    #[default]
    Hide,
    Error,
}

fn default_ipp_path() -> String {
    "/ipp/print".into()
}
fn default_timeout() -> u64 {
    4
}
fn default_community() -> String {
    "public".into()
}
fn default_bar_format() -> String {
    // Nerd Font printer glyph (nf-md-printer) — shares the bar font's baseline, unlike a
    // color emoji which renders misaligned.
    "\u{f042a} {supply_min}%".into()
}
fn default_tooltip_items() -> Vec<String> {
    [
        "model", "status", "alerts", "display", "supplies", "paper", "jobs", "pages",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_max_rows() -> usize {
    12
}
fn default_frame_font() -> String {
    "JetBrainsMono Nerd Font Mono".into()
}
fn default_low() -> u8 {
    15
}
fn default_critical() -> u8 {
    5
}

impl Default for SnmpCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            community: default_community(),
        }
    }
}
impl Default for BarCfg {
    fn default() -> Self {
        Self {
            format: default_bar_format(),
            on_missing: OnMissing::default(),
        }
    }
}
impl Default for TooltipCfg {
    fn default() -> Self {
        Self {
            items: default_tooltip_items(),
            on_missing: OnMissing::default(),
            max_rows: default_max_rows(),
            frame: false,
            frame_font: default_frame_font(),
        }
    }
}
impl Default for Thresholds {
    fn default() -> Self {
        Self {
            supply_low: default_low(),
            supply_critical: default_critical(),
        }
    }
}

impl Config {
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| format!("config read {}: {e}", path.display()))?;
        Self::parse(&s).map_err(|e| format!("config parse: {e}"))
    }

    pub fn for_printer(&self, name: &str) -> Option<&PrinterConfig> {
        self.printer.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_section_with_defaults() {
        let toml = r#"
            [printer.oficina]
            host = "192.0.2.70"
            cups = "HP_M477fdw"

            [printer.oficina.snmp]
            enabled = true

            [printer.oficina.bar]
            format = "🖨 {supply_min}%"

            [printer.oficina.tooltip]
            items = ["status", "supplies"]
        "#;
        let cfg = Config::parse(toml).unwrap();
        let p = cfg.for_printer("oficina").unwrap();
        assert_eq!(p.host.as_deref(), Some("192.0.2.70"));
        assert_eq!(p.ipp_path, "/ipp/print"); // default
        assert_eq!(p.timeout, 4); // default
        assert!(p.snmp.enabled);
        assert_eq!(p.snmp.community, "public"); // default
        assert_eq!(p.bar.on_missing, OnMissing::Hide); // default
        assert_eq!(p.tooltip.items, vec!["status", "supplies"]);
        assert_eq!(p.tooltip.max_rows, 12); // default
        assert_eq!(p.thresholds.supply_low, 15); // default
    }

    #[test]
    fn missing_printer_is_none() {
        let cfg = Config::parse("").unwrap();
        assert!(cfg.for_printer("nope").is_none());
    }
}
