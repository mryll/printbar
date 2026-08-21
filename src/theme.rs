//! Theme resolution, shared with the rest of the *bar family.
//!
//! Chain: the active Omarchy theme (state dir, then the legacy config dir) →
//! the pywal cache → the built-in One Dark palette. Every tier degrades on its
//! own and per field: a missing file, an unreadable file, a malformed value or
//! a key a theme simply does not ship must never sink the whole load, and must
//! never reach Pango markup.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

#[allow(dead_code)] // shared palette; not all colors used here
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeColors {
    pub border: String,
    pub text: String,
    pub dim: String,
    pub accent: String,
    pub green: String,
    pub yellow: String,
    pub orange: String,
    pub error: String,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            border: "#61afef".into(),
            text: "#abb2bf".into(),
            dim: "#5c6370".into(),
            accent: "#61afef".into(),
            green: "#98c379".into(),
            yellow: "#e5c07b".into(),
            orange: "#d19a66".into(),
            error: "#e06c75".into(),
        }
    }
}

/// Omarchy 4.x keeps the active theme under the STATE dir — that's what the
/// shell's own `Commons/Color.qml` reads. The old config-dir path is kept as a
/// legacy fallback for pre-4.x installs.
const THEME_SUFFIX: &str = "omarchy/current/theme/colors.toml";

/// pywal's cache file. The original pywal (dylanaraps) is archived, but this
/// path is the de-facto standard: the maintained `pywal16` fork writes the
/// same file, and `wallust` can target it for pywal compatibility — so one
/// path covers all three ecosystems.
const PYWAL_SUFFIX: &str = "wal/colors.json";

/// Resolve an XDG base directory: the env var when set and NON-EMPTY,
/// otherwise the conventional path under `$HOME`. An explicitly empty variable
/// is unset per the XDG spec — treating it as a path would silently look for
/// the theme in a relative directory. Split from the env lookup so tests can
/// exercise the precedence without mutating the environment.
fn dir_from(
    env_value: Option<OsString>,
    home: Option<PathBuf>,
    home_suffix: &str,
) -> Option<PathBuf> {
    match env_value {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => home.map(|home| home.join(home_suffix)),
    }
}

fn resolve_dir(env_var: &str, home_suffix: &str) -> Option<PathBuf> {
    dir_from(std::env::var_os(env_var), dirs::home_dir(), home_suffix)
}

/// Directory Omarchy keeps the active theme under: `$XDG_STATE_HOME`, or
/// `~/.local/state` when unset.
fn state_dir() -> Option<PathBuf> {
    resolve_dir("XDG_STATE_HOME", ".local/state")
}

/// Candidate `colors.toml` locations, most current first.
fn candidate_paths(state: Option<PathBuf>, config: Option<PathBuf>) -> Vec<PathBuf> {
    [state, config]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(THEME_SUFFIX))
        .collect()
}

/// First existing colors.toml, or None when no Omarchy theme is installed —
/// the caller then falls through to pywal, and finally to the built-in palette.
fn theme_colors_file() -> Option<PathBuf> {
    candidate_paths(state_dir(), dirs::config_dir())
        .into_iter()
        .find(|path| path.is_file())
}

fn pywal_path(cache: Option<PathBuf>) -> Option<PathBuf> {
    cache.map(|dir| dir.join(PYWAL_SUFFIX))
}

/// pywal's colors.json when one exists.
fn pywal_colors_file() -> Option<PathBuf> {
    pywal_path(resolve_dir("XDG_CACHE_HOME", ".cache")).filter(|path| path.is_file())
}

/// pywal cache document. Both sections are optional and read as plain string
/// maps so a partial or unusual file still yields whatever keys it does have.
#[derive(Deserialize, Default)]
struct PywalColors {
    #[serde(default)]
    special: HashMap<String, String>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

impl ThemeColors {
    /// Theme resolution chain: Omarchy theme (state dir, then the legacy
    /// config dir) → pywal cache → built-in One Dark defaults. pywal is only
    /// consulted when no Omarchy theme was found.
    pub fn load() -> Self {
        Self::load_from(theme_colors_file(), pywal_colors_file())
    }

    fn load_from(omarchy: Option<PathBuf>, pywal: Option<PathBuf>) -> Self {
        if let Some(path) = omarchy {
            if let Ok(content) = fs::read_to_string(&path) {
                return Self::from_toml(&content);
            }
        }
        if let Some(path) = pywal {
            if let Ok(content) = fs::read_to_string(&path) {
                return Self::from_pywal_json(&content);
            }
        }
        Self::default()
    }

    fn from_toml(content: &str) -> Self {
        Self::from_map(&parse_toml_flat(content))
    }

    /// Invalid JSON degrades to the built-in palette, like a missing file.
    fn from_pywal_json(content: &str) -> Self {
        match serde_json::from_str::<PywalColors>(content) {
            Ok(parsed) => Self::from_pywal(&parsed),
            Err(_) => Self::default(),
        }
    }

    /// Map a pywal palette onto widget colors, field by field: anything the
    /// file omits — or spells with a non-hex value — keeps its default.
    fn from_pywal(parsed: &PywalColors) -> Self {
        let mut colors = Self::default();
        let special = |key: &str| {
            parsed
                .special
                .get(key)
                .map(String::as_str)
                .filter(|value| is_hex_color(value))
        };
        let color = |key: &str| {
            parsed
                .colors
                .get(key)
                .map(String::as_str)
                .filter(|value| is_hex_color(value))
        };

        if let Some(foreground) = special("foreground") {
            colors.text = foreground.to_string();
        }
        if let (Some(foreground), Some(background)) = (special("foreground"), special("background"))
        {
            colors.dim = blend_hex(foreground, background, 0.5);
        }
        // pywal has no accent slot; color4 is its blue, with the cursor color
        // as the fallback highlight.
        if let Some(accent) = color("color4").or_else(|| special("cursor")) {
            colors.border = accent.to_string();
            colors.accent = accent.to_string();
        }
        if let Some(green) = color("color2") {
            colors.green = green.to_string();
        }
        if let Some(yellow) = color("color3") {
            colors.yellow = yellow.to_string();
        }
        if let Some(red) = color("color1") {
            colors.error = red.to_string();
        }
        // Orange has no pywal slot either. Synthesize it as the midpoint of
        // yellow and red rather than aliasing it to red, which would flatten
        // the supply gauge's warn and critical steps into one color. Needs both
        // ends: with either missing, orange keeps its default.
        if let (Some(yellow), Some(red)) = (color("color3"), color("color1")) {
            colors.orange = blend_hex(yellow, red, 0.5);
        }
        colors
    }

    /// Map a theme's palette onto the widget's colors. Every field degrades on
    /// its own: a key the theme does not define keeps its built-in default
    /// instead of sinking the whole load (the old `?` chain did exactly that —
    /// and Omarchy 4.x themes carry no `colorN` keys at all, so every load
    /// bailed).
    ///
    /// Validation happens DURING selection, not after it: an invalid `orange`
    /// must fall through to `red`/`color1`, not suppress them and then get
    /// replaced by the built-in default.
    fn from_map(map: &HashMap<String, String>) -> Self {
        let mut colors = Self::default();
        let get = |key: &str| map.get(key).filter(|value| is_hex_color(value));
        let first = |keys: &[&str]| keys.iter().find_map(|key| get(key)).cloned();

        // Named keys are what current themes ship; `color1/2/3` keep pre-4.x
        // themes working exactly as before, with `color1` standing in for red.
        if let Some(accent) = first(&["accent", "blue", "color4"]) {
            colors.border = accent.clone();
            colors.accent = accent;
        }
        if let Some(foreground) = first(&["foreground"]) {
            colors.text = foreground;
        }
        if let (Some(foreground), Some(background)) = (get("foreground"), get("background")) {
            colors.dim = blend_hex(foreground, background, 0.5);
        }
        if let Some(green) = first(&["green", "color2"]) {
            colors.green = green;
        }
        if let Some(yellow) = first(&["yellow", "color3"]) {
            colors.yellow = yellow;
        }
        // `color1` stood in for both red and orange in legacy themes.
        if let Some(orange) = first(&["orange", "red", "color1"]) {
            colors.orange = orange;
        }
        if let Some(error) = first(&["red", "color1"]) {
            colors.error = error;
        }
        colors
    }
}

fn parse_toml_flat(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().trim_matches('"').to_string();
            map.insert(key, value);
        }
    }
    map
}

/// Strict `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa` check. Values that fail
/// it are ignored so a malformed theme key can never reach Pango markup.
fn is_hex_color(s: &str) -> bool {
    match s.strip_prefix('#') {
        Some(hex) => {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Mix two colors. Only 6-digit hex can be blended; anything else (a short
/// `#rgb`, an alpha form) keeps the built-in dim rather than silently blending
/// One Dark's values behind the user's back.
fn blend_hex(c1: &str, c2: &str, ratio: f32) -> String {
    let (Some((r1, g1, b1)), Some((r2, g2, b2))) = (parse_hex(c1), parse_hex(c2)) else {
        return ThemeColors::default().dim;
    };
    let blend =
        |a: u8, b: u8| -> u8 { (a as f32 * (1.0 - ratio) + b as f32 * ratio).round() as u8 };
    format!(
        "#{:02x}{:02x}{:02x}",
        blend(r1, r2),
        blend(g1, g2),
        blend(b1, b2)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokyo Night as Omarchy 4.x ships it: named keys, no `colorN` at all.
    const NAMED: &str = r##"
mode = "dark"

accent = "#7aa2f7"
muted = "#414868"

background = "#1a1b26"
foreground = "#a9b1d6"

red = "#f7768e"
yellow = "#e0af68"
orange = "#eb927b"
green = "#9ece6a"
"##;

    /// A pre-4.x theme: terminal palette only, no named color keys.
    const LEGACY: &str = r##"
foreground = "#abb2bf"
background = "#282c34"
accent = "#61afef"
color1 = "#e06c75"
color2 = "#98c379"
color3 = "#e5c07b"
"##;

    const PYWAL_JSON: &str = r##"{
  "special": { "background": "#1a1b26", "foreground": "#a9b1d6", "cursor": "#ffffff" },
  "colors": {
    "color0": "#1a1b26",
    "color1": "#f7768e",
    "color2": "#9ece6a",
    "color3": "#e0af68",
    "color4": "#7aa2f7",
    "color5": "#ad8ee6"
  }
}"##;

    #[test]
    fn state_path_is_preferred_over_legacy_config_path() {
        let paths = candidate_paths(
            Some(PathBuf::from("/home/u/.local/state")),
            Some(PathBuf::from("/home/u/.config")),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/.local/state/omarchy/current/theme/colors.toml"),
                PathBuf::from("/home/u/.config/omarchy/current/theme/colors.toml"),
            ],
            "Omarchy 4.x state dir must be tried before the legacy config dir"
        );
        // With no state dir resolvable, the old config path still gets tried.
        assert_eq!(
            candidate_paths(None, Some(PathBuf::from("/home/u/.config"))),
            vec![PathBuf::from(
                "/home/u/.config/omarchy/current/theme/colors.toml"
            )]
        );
        assert!(candidate_paths(None, None).is_empty());
    }

    #[test]
    fn xdg_dirs_honor_the_env_but_not_an_empty_one() {
        assert_eq!(
            dir_from(
                Some(OsString::from("/custom/state")),
                Some(PathBuf::from("/home/u")),
                ".local/state"
            ),
            Some(PathBuf::from("/custom/state"))
        );
        // Unset AND explicitly-empty both mean "use the default location".
        for empty in [None, Some(OsString::from(""))] {
            assert_eq!(
                dir_from(empty, Some(PathBuf::from("/home/u")), ".local/state"),
                Some(PathBuf::from("/home/u/.local/state"))
            );
        }
        assert_eq!(dir_from(None, None, ".local/state"), None);
    }

    #[test]
    fn xdg_cache_home_is_honored_for_the_pywal_path() {
        assert_eq!(
            pywal_path(dir_from(
                Some(OsString::from("/xdg/cache")),
                Some(PathBuf::from("/home/u")),
                ".cache"
            )),
            Some(PathBuf::from("/xdg/cache/wal/colors.json"))
        );
        assert_eq!(
            pywal_path(dir_from(None, Some(PathBuf::from("/home/u")), ".cache")),
            Some(PathBuf::from("/home/u/.cache/wal/colors.json"))
        );
        assert_eq!(pywal_path(dir_from(None, None, ".cache")), None);
    }

    #[test]
    fn named_keys_drive_the_palette() {
        let t = ThemeColors::from_toml(NAMED);
        assert_eq!(t.accent, "#7aa2f7");
        assert_eq!(t.border, "#7aa2f7");
        assert_eq!(t.text, "#a9b1d6");
        assert_eq!(t.error, "#f7768e"); // `red`
        assert_eq!(t.orange, "#eb927b"); // own `orange`, not red
        assert_eq!(t.green, "#9ece6a");
        assert_eq!(t.yellow, "#e0af68");
        // dim = midpoint of foreground and background
        assert_eq!(t.dim, "#62667e");
        // The regression this fixes: a named-key theme must NOT fall back.
        assert_ne!(t, ThemeColors::default());
    }

    #[test]
    fn legacy_color_keys_still_work() {
        let t = ThemeColors::from_toml(LEGACY);
        assert_eq!(t.accent, "#61afef");
        assert_eq!(t.error, "#e06c75"); // color1
        assert_eq!(t.orange, "#e06c75"); // color1 stands in for orange too
        assert_eq!(t.green, "#98c379"); // color2
        assert_eq!(t.yellow, "#e5c07b"); // color3
    }

    #[test]
    fn missing_keys_fall_back_individually_without_sinking_the_load() {
        // Only an accent: everything else keeps the default palette.
        let t = ThemeColors::from_toml("accent = \"#123456\"\n");
        let d = ThemeColors::default();
        assert_eq!(t.accent, "#123456");
        assert_eq!(t.border, "#123456");
        assert_eq!(t.green, d.green);
        assert_eq!(t.error, d.error);
        assert_eq!(t.dim, d.dim); // needs both fg and bg
        assert_eq!(t.text, d.text);
    }

    #[test]
    fn no_theme_installed_degrades_to_defaults() {
        for content in ["", "# just a comment\n\n", "<<<not toml at all>>>"] {
            assert_eq!(ThemeColors::from_toml(content), ThemeColors::default());
        }
        assert_eq!(
            ThemeColors::from_map(&HashMap::new()),
            ThemeColors::default()
        );
    }

    #[test]
    fn malformed_values_are_sanitized_away() {
        let toml = r##"
accent = "not-a-color"
foreground = "#a9b1d6"
red = "<script>"
green = "#0f0"
"##;
        let t = ThemeColors::from_toml(toml);
        let d = ThemeColors::default();
        assert_eq!(t.accent, d.accent); // rejected
        assert_eq!(t.error, d.error); // rejected
        assert_eq!(t.green, "#0f0"); // short hex is valid
        assert_eq!(t.text, "#a9b1d6");
    }

    #[test]
    fn an_invalid_named_key_does_not_suppress_a_valid_legacy_one() {
        // Selection validates as it goes, so a broken `orange`/`red` falls
        // through to `color1` instead of blocking it and landing on One Dark.
        let t = ThemeColors::from_toml(
            "orange = \"not-a-color\"\nred = \"\"\ncolor1 = \"#e06c75\"\ngreen = \"nope\"\ncolor2 = \"#98c379\"\n",
        );
        assert_eq!(t.orange, "#e06c75");
        assert_eq!(t.error, "#e06c75");
        assert_eq!(t.green, "#98c379");
    }

    #[test]
    fn empty_values_are_ignored() {
        let t = ThemeColors::from_toml("accent = \"\"\ngreen = \"\"\n");
        assert_eq!(t.accent, ThemeColors::default().accent);
        assert_eq!(t.green, ThemeColors::default().green);
    }

    // ---- pywal tier ----------------------------------------------------------

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "printbar-theme-test-{}-{}",
            tag,
            std::process::id()
        ));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn pywal_is_used_when_no_omarchy_theme_exists() {
        let dir = scratch_dir("pywal-only");
        let pywal = write(&dir, "colors.json", PYWAL_JSON);

        let t = ThemeColors::load_from(None, Some(pywal));
        assert_eq!(t.text, "#a9b1d6"); // special.foreground
        assert_eq!(t.accent, "#7aa2f7"); // colors.color4
        assert_eq!(t.border, "#7aa2f7");
        assert_eq!(t.green, "#9ece6a"); // colors.color2
        assert_eq!(t.yellow, "#e0af68"); // colors.color3
        assert_eq!(t.error, "#f7768e"); // colors.color1
        assert_eq!(t.dim, "#62667e"); // fg/bg blend
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn omarchy_theme_wins_when_both_exist() {
        let dir = scratch_dir("both");
        let omarchy = write(&dir, "colors.toml", LEGACY);
        let pywal = write(&dir, "colors.json", PYWAL_JSON);

        let t = ThemeColors::load_from(Some(omarchy), Some(pywal));
        assert_eq!(t.accent, "#61afef"); // Omarchy values, not pywal's
        assert_eq!(t.green, "#98c379");
        assert_eq!(t.error, "#e06c75");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_files_fall_through_to_the_next_tier() {
        let dir = scratch_dir("missing");
        let absent_toml = dir.join("nope.toml");

        assert_eq!(ThemeColors::load_from(None, None), ThemeColors::default());
        assert_eq!(
            ThemeColors::load_from(Some(absent_toml.clone()), Some(dir.join("nope.json"))),
            ThemeColors::default()
        );
        // An unreadable Omarchy path still falls through to a usable pywal file.
        let pywal = write(&dir, "colors.json", PYWAL_JSON);
        assert_eq!(
            ThemeColors::load_from(Some(absent_toml), Some(pywal)).accent,
            "#7aa2f7"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn garbage_pywal_json_falls_back_to_defaults() {
        for content in ["", "   ", "not json at all", "{", "[]", "null", "{}"] {
            assert_eq!(
                ThemeColors::from_pywal_json(content),
                ThemeColors::default(),
                "content: {content:?}"
            );
        }
        assert_eq!(
            ThemeColors::from_pywal_json(r##"{"special":{},"colors":{}}"##),
            ThemeColors::default()
        );
    }

    #[test]
    fn pywal_orange_is_synthesized_from_yellow_and_red() {
        let t = ThemeColors::from_pywal_json(PYWAL_JSON);
        // Midpoint of yellow #e0af68 and red #f7768e — never aliased to red,
        // which would collapse the supply gauge's warn and critical steps.
        assert_eq!(t.orange, "#ec937b");
        assert_ne!(t.orange, t.error);
        assert_ne!(t.orange, t.yellow);

        let only_red = ThemeColors::from_pywal_json(r##"{"colors":{"color1":"#f7768e"}}"##);
        assert_eq!(only_red.error, "#f7768e");
        assert_eq!(only_red.orange, ThemeColors::default().orange);
    }

    #[test]
    fn pywal_accent_falls_back_to_cursor_and_ignores_non_hex() {
        let t = ThemeColors::from_pywal_json(r##"{"special":{"cursor":"#abcdef"},"colors":{}}"##);
        assert_eq!(t.accent, "#abcdef");

        let t = ThemeColors::from_pywal_json(
            r##"{"special":{"cursor":"#abcdef","foreground":"rgb(1,2,3)"},
                "colors":{"color4":"blue","color2":"not-a-color"}}"##,
        );
        assert_eq!(t.accent, "#abcdef");
        assert_eq!(t.text, ThemeColors::default().text);
        assert_eq!(t.green, ThemeColors::default().green);
    }

    #[test]
    fn is_hex_color_accepts_only_hex_forms() {
        for good in ["#abc", "#abcd", "#aabbcc", "#aabbccdd", "#ABCDEF"] {
            assert!(is_hex_color(good), "should accept {good}");
        }
        for bad in [
            "",
            "#",
            "abcdef",
            "#ab",
            "#abcde",
            "#abcdefg",
            "#gggggg",
            "blue",
            "rgb(1,2,3)",
        ] {
            assert!(!is_hex_color(bad), "should reject {bad}");
        }
    }

    #[test]
    fn dim_only_blends_what_it_can_actually_blend() {
        // Short hex is a valid color but not blendable: keep the built-in dim
        // rather than quietly blending One Dark's numbers instead.
        let t = ThemeColors::from_toml("foreground = \"#fff\"\nbackground = \"#000\"\n");
        assert_eq!(t.text, "#fff");
        assert_eq!(t.dim, ThemeColors::default().dim);
        // Without a background there is nothing to blend against.
        let t = ThemeColors::from_toml("foreground = \"#a9b1d6\"\n");
        assert_eq!(t.dim, ThemeColors::default().dim);
    }
}
