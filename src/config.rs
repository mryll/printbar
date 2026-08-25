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
    /// DEPRECATED, still accepted so an existing config keeps loading. It drew
    /// a bordered card around the tooltip and now does nothing. Read nowhere on
    /// purpose: the point is that an old `frame = true` neither errors nor
    /// changes what is drawn.
    #[serde(default)]
    #[allow(dead_code)]
    pub frame: bool,
    /// The tooltip is pinned to this family. A Pango family LIST, not one name:
    /// Pango tries them in order and falls through when one is not installed —
    /// the Arch package ttf-jetbrains-mono-nerd does NOT ship the "…Mono"
    /// family, so pinning only that name fell back to the system's proportional
    /// font without saying so.
    ///
    /// It must be monospace: the tooltip's rules are box-drawing characters,
    /// which in a proportional font render far wider than letters, so the
    /// tooltip sizes itself to the rules and grows a dead margin to the right
    /// of the text. Waybar draws the tooltip in a GTK window that ignores
    /// font-family from CSS, so the markup is the only place to say it.
    #[serde(default = "default_tooltip_font", alias = "frame_font")]
    pub tooltip_font: String,
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
fn default_tooltip_font() -> String {
    "JetBrainsMono Nerd Font Mono, JetBrainsMono Nerd Font, monospace".into()
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
            tooltip_font: default_tooltip_font(),
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

/// Where the shipped `config.example.toml` is, as a string the user can paste.
///
/// The package puts it under the install prefix, so the prefix is whatever
/// this binary was installed with — /usr for the package, ~/.local for
/// `make install PREFIX=~/.local`, and nowhere at all for a release binary
/// dropped on the PATH by hand. Each candidate is checked; the repository URL
/// is the answer when none of them is there.
fn example_path() -> String {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        // <prefix>/bin/printbar -> <prefix>/share/printbar/config.example.toml
        if let Some(prefix) = exe.parent().and_then(|d| d.parent()) {
            candidates.push(prefix.join("share/printbar/config.example.toml"));
        }
    }
    candidates.push(std::path::PathBuf::from(
        "/usr/share/printbar/config.example.toml",
    ));
    for c in candidates {
        if c.is_file() {
            return c.display().to_string();
        }
    }
    "https://github.com/mryll/printbar/raw/master/config.example.toml".into()
}

impl Config {
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        // A missing file is the FIRST RUN, not a failure to report as one. It is
        // what every new user meets, and "No such file or directory" tells them
        // nothing they can act on — so this path says what to write and where the
        // shipped example is. Every other io error keeps its own words.
        let s = crate::safe_read::read_bounded(path, crate::safe_read::CONFIG_LIMIT).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                let dir = path
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                // The example is named where it ACTUALLY is. A hardcoded
                // /usr/share is right for the package and wrong for
                // `make install PREFIX=~/.local` or a bare release binary,
                // and a copy command that names a missing file is worse
                // than no copy command.
                let example = example_path();
                format!(
                    "no config yet: {}\n\nCopy the example and name your printer in it:\n  mkdir -p {}\n  cp {} {}",
                    path.display(),
                    dir,
                    example,
                    path.display(),
                )
            } else {
                format!("config read {}: {e}", path.display())
            }
        })?;
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
