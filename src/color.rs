//! Monochrome mode: which output surfaces still emit color.
//!
//! One flag with an optional value — `--no-color[=all|bar|tooltip]` — plus the
//! conventional `NO_COLOR` environment variable (<https://no-color.org>).
//! A non-empty `NO_COLOR` behaves as `--no-color=all`; an explicit flag on the
//! command line always wins over it, being the more specific instruction.
//!
//! Only presentation is affected. The Waybar `class`/`alt` fields and the
//! structured `--json` document are untouched — they are machine contracts, and
//! the CSS classes are precisely what lets a monochrome user style the bar from
//! their own stylesheet instead.

/// The flag, spelled the same in every *bar repo.
pub const FLAG: &str = "--no-color";

const FLAG_EQ: &str = "--no-color=";

/// Which surfaces keep their colors. `true` = colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorMode {
    pub bar: bool,
    pub tooltip: bool,
}

impl Default for ColorMode {
    fn default() -> Self {
        Self::FULL
    }
}

impl ColorMode {
    /// Default: both surfaces colored.
    pub const FULL: Self = Self {
        bar: true,
        tooltip: true,
    };
    /// `--no-color` / `--no-color=all`.
    pub const PLAIN_ALL: Self = Self {
        bar: false,
        tooltip: false,
    };
    /// `--no-color=bar`: plain bar text, colored tooltip.
    pub const PLAIN_BAR: Self = Self {
        bar: false,
        tooltip: true,
    };
    /// `--no-color=tooltip`: colored bar text, plain tooltip.
    pub const PLAIN_TOOLTIP: Self = Self {
        bar: true,
        tooltip: false,
    };

    fn from_value(value: &str) -> Result<Self, String> {
        match value {
            "all" => Ok(Self::PLAIN_ALL),
            "bar" => Ok(Self::PLAIN_BAR),
            "tooltip" => Ok(Self::PLAIN_TOOLTIP),
            other => Err(format!(
                "{FLAG}: unknown value '{other}' (expected all, bar or tooltip)"
            )),
        }
    }

    /// Resolve the mode from argv plus the `NO_COLOR` env value.
    ///
    /// Last flag wins; an unknown value is an argument error, which the caller
    /// funnels into the repo's usual exit-0 error output.
    pub fn resolve(args: &[String], no_color_env: Option<&str>) -> Result<Self, String> {
        let mut mode = match no_color_env {
            Some(v) if !v.is_empty() => Self::PLAIN_ALL,
            _ => Self::FULL,
        };
        for a in args.iter().skip(1) {
            if a == FLAG {
                mode = Self::PLAIN_ALL;
            } else if let Some(v) = a.strip_prefix(FLAG_EQ) {
                mode = Self::from_value(v)?;
            }
        }
        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn resolve(v: &[&str], env: Option<&str>) -> Result<ColorMode, String> {
        ColorMode::resolve(&args(v), env)
    }

    #[test]
    fn the_four_states() {
        assert_eq!(resolve(&["printbar", "x"], None).unwrap(), ColorMode::FULL);
        assert_eq!(
            resolve(&["printbar", "x", "--no-color"], None).unwrap(),
            ColorMode::PLAIN_ALL
        );
        assert_eq!(
            resolve(&["printbar", "x", "--no-color=all"], None).unwrap(),
            ColorMode::PLAIN_ALL
        );
        assert_eq!(
            resolve(&["printbar", "x", "--no-color=bar"], None).unwrap(),
            ColorMode::PLAIN_BAR
        );
        assert_eq!(
            resolve(&["printbar", "x", "--no-color=tooltip"], None).unwrap(),
            ColorMode::PLAIN_TOOLTIP
        );
    }

    #[test]
    fn no_color_env_turns_everything_plain_when_non_empty() {
        assert_eq!(
            resolve(&["printbar", "x"], Some("1")).unwrap(),
            ColorMode::PLAIN_ALL
        );
        // no-color.org: an EMPTY value does not count as set.
        assert_eq!(
            resolve(&["printbar", "x"], Some("")).unwrap(),
            ColorMode::FULL
        );
    }

    #[test]
    fn explicit_flag_beats_the_env_var() {
        // The more specific instruction wins — even when it re-enables a surface.
        assert_eq!(
            resolve(&["printbar", "x", "--no-color=bar"], Some("1")).unwrap(),
            ColorMode::PLAIN_BAR // tooltip stays colored despite NO_COLOR
        );
        assert_eq!(
            resolve(&["printbar", "x", "--no-color=tooltip"], Some("1")).unwrap(),
            ColorMode::PLAIN_TOOLTIP
        );
    }

    #[test]
    fn unknown_value_is_an_argument_error() {
        let err = resolve(&["printbar", "x", "--no-color=bogus"], None).unwrap_err();
        assert_eq!(
            err,
            "--no-color: unknown value 'bogus' (expected all, bar or tooltip)"
        );
        // An empty value is a value, and it isn't one of the three.
        assert!(resolve(&["printbar", "x", "--no-color="], None).is_err());
    }

    #[test]
    fn last_flag_wins() {
        assert_eq!(
            resolve(&["printbar", "x", "--no-color", "--no-color=tooltip"], None).unwrap(),
            ColorMode::PLAIN_TOOLTIP
        );
    }

    #[test]
    fn a_printer_named_like_the_flag_is_not_a_flag() {
        // argv[0] is never scanned, and only exact spellings match.
        assert_eq!(
            resolve(&["--no-color", "x", "--no-colour"], None).unwrap(),
            ColorMode::FULL
        );
    }
}
