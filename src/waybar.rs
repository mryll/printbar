//! Waybar JSON output + Pango/box-drawing helpers, matching the meteobar/tickerbar
//! tooltip style. Every fatal path goes through `error_output` so the binary always
//! exits 0 with valid Waybar JSON.

use serde::Serialize;

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: Vec<String>,
    pub alt: String,
}

impl WaybarOutput {
    pub fn print(&self) {
        // serde_json on these owned String/Vec fields cannot fail.
        println!(
            "{}",
            serde_json::to_string(self).unwrap_or_else(|_| error_output("serialize"))
        );
    }
}

/// Valid Waybar JSON for any fatal error — keeps the exit-0 contract.
///
/// The reason is Pango-escaped: it can quote a config path, a printer name or
/// a parser message, and Waybar renders the tooltip as markup. An unescaped
/// `&` there would silently blank the tooltip.
pub fn error_output(reason: &str) -> String {
    let out = WaybarOutput {
        text: "?".into(),
        tooltip: pango_escape(reason),
        class: vec!["error".into()],
        alt: "error".into(),
    };
    serde_json::to_string(&out).unwrap_or_else(|_| {
        r#"{"text":"?","tooltip":"error","class":["error"],"alt":"error"}"#.into()
    })
}

pub fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn fg(color: &str, text: &str) -> String {
    format!("<span foreground='{color}'>{text}</span>")
}

pub fn bold_fg(color: &str, text: &str) -> String {
    format!("<span font_weight='bold' foreground='{color}'>{text}</span>")
}

/// Emits Pango markup for one surface, honoring that surface's color mode.
///
/// Monochrome drops the color attributes and nothing else: weight, the box
/// drawing, the glyphs, the padding and the layout are structure, not color, so
/// a plain tooltip is the same tooltip with the hues taken out.
#[derive(Debug, Clone, Copy)]
pub struct Paint {
    colored: bool,
}

impl Paint {
    pub fn new(colored: bool) -> Self {
        Self { colored }
    }

    pub fn fg(&self, color: &str, text: &str) -> String {
        if self.colored {
            fg(color, text)
        } else {
            text.to_string()
        }
    }

    pub fn bold_fg(&self, color: &str, text: &str) -> String {
        if self.colored {
            bold_fg(color, text)
        } else {
            format!("<span font_weight='bold'>{text}</span>")
        }
    }

    pub fn border_line(&self, content: &str, width: usize, border_color: &str) -> String {
        let pad = width.saturating_sub(visible_len(content));
        let right_pad = " ".repeat(pad);
        format!(
            "{} {content}{right_pad} {}",
            self.fg(border_color, "│"),
            self.fg(border_color, "│")
        )
    }

    pub fn separator(&self, width: usize, border_color: &str, dim_color: &str) -> String {
        self.border_line(&self.fg(dim_color, &"─".repeat(width)), width, border_color)
    }

    pub fn top_border(&self, width: usize, border_color: &str) -> String {
        self.fg(border_color, &format!("╭{}╮", "─".repeat(width + 2)))
    }

    pub fn bottom_border(&self, width: usize, border_color: &str) -> String {
        self.fg(border_color, &format!("╰{}╯", "─".repeat(width + 2)))
    }
}

/// Pango attributes that paint something. Everything else on a `<span>` is
/// structure (weight, family, size, rise, …) and survives monochrome mode.
const COLOR_ATTRS: [&str; 8] = [
    "foreground",
    "fgcolor",
    "color",
    "background",
    "bgcolor",
    "underline_color",
    "overline_color",
    "strikethrough_color",
];

/// Strip every color attribute from a Pango string, dropping any `<span>` left
/// with nothing to say. Text, `<b>`/`<i>` and non-color span attributes pass
/// through untouched.
///
/// The bar text is the surface that needs this: printbar emits no color there
/// itself, but `bar.format` is the user's own string and may carry markup. The
/// flag has to hold for the whole surface, not just for the parts we wrote.
pub fn strip_color_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dropped: Vec<bool> = Vec::new(); // one entry per open <span>
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[i..]); // unterminated '<': leave it exactly as it came
            return out;
        };
        let tag = after[..end].trim();
        rest = &after[end + 1..];
        if tag == "/span" {
            if !dropped.pop().unwrap_or(false) {
                out.push_str("</span>");
            }
        } else if let Some(attrs) = span_attrs(tag) {
            let kept = strip_color_attrs(attrs);
            dropped.push(kept.is_empty());
            if !kept.is_empty() {
                out.push_str(&format!("<span{kept}>"));
            }
        } else {
            out.push('<');
            out.push_str(&after[..end]);
            out.push('>');
        }
    }
    out.push_str(rest);
    out
}

/// The attribute text of an opening `<span …>` tag, or `None` for other tags.
fn span_attrs(tag: &str) -> Option<&str> {
    let rest = tag.strip_prefix("span")?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
    }
}

/// Re-emit `name=value` attribute pairs, minus the ones that paint.
fn strip_color_attrs(attrs: &str) -> String {
    let mut kept = String::new();
    let mut rest = attrs.trim_start();
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim().to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let (value, tail) = match after.chars().next() {
            // Quoted value: everything up to the matching quote.
            Some(q @ ('\'' | '"')) => match after[1..].find(q) {
                Some(e) => after.split_at(e + 2),
                None => (after, ""),
            },
            // Bare value: up to the next space.
            _ => after.split_at(after.find(char::is_whitespace).unwrap_or(after.len())),
        };
        if !COLOR_ATTRS.contains(&name.as_str()) {
            kept.push(' ');
            kept.push_str(&name);
            kept.push('=');
            kept.push_str(value);
        }
        rest = tail.trim_start();
    }
    kept
}

/// Visible (rendered) width of a string, ignoring Pango tags and counting entities as one.
pub fn visible_len(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    let mut plain = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut in_entity = false;
    for ch in s.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if in_entity {
            if ch == ';' {
                in_entity = false;
                plain.push('x');
            }
            continue;
        }
        match ch {
            '<' => in_tag = true,
            '&' => in_entity = true,
            _ => plain.push(ch),
        }
    }
    UnicodeWidthStr::width(plain.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_output_is_valid_waybar_json() {
        let s = error_output("boom");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["text"], "?");
        assert_eq!(v["tooltip"], "boom");
        assert_eq!(v["class"], serde_json::json!(["error"]));
        assert_eq!(v["alt"], "error");
    }

    #[test]
    fn error_output_escapes_markup_in_the_reason() {
        // Error messages quote config paths, printer names and parser output —
        // all of which can carry `&` or `<`, which Waybar would try to parse.
        let s = error_output("no [printer.Ben & Jerry's <lab>] in config");
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v["tooltip"],
            "no [printer.Ben &amp; Jerry's &lt;lab&gt;] in config"
        );
    }

    #[test]
    fn fg_wraps_in_pango_span() {
        assert_eq!(fg("#fff", "x"), "<span foreground='#fff'>x</span>");
    }

    #[test]
    fn visible_len_ignores_markup_and_entities() {
        assert_eq!(visible_len("<span foreground='#fff'>ab</span>"), 2);
        assert_eq!(visible_len("a&amp;b"), 3); // a + entity(1) + b
    }

    #[test]
    fn monochrome_paint_emits_no_color_but_keeps_weight_and_frame() {
        let p = Paint::new(false);
        assert_eq!(p.fg("#fff", "x"), "x");
        assert_eq!(p.bold_fg("#fff", "x"), "<span font_weight='bold'>x</span>");
        assert_eq!(p.top_border(3, "#fff"), "╭─────╮");
        assert_eq!(p.border_line("ab", 4, "#fff"), "│ ab   │");
        assert!(!p.separator(4, "#fff", "#888").contains("foreground"));
        // Structure is untouched: same visible width as the colored painter.
        let c = Paint::new(true);
        assert_eq!(
            visible_len(&p.border_line("ab", 4, "#fff")),
            visible_len(&c.border_line("ab", 4, "#fff"))
        );
    }

    #[test]
    fn colored_paint_is_the_plain_helpers() {
        let p = Paint::new(true);
        assert_eq!(p.fg("#fff", "x"), fg("#fff", "x"));
        assert_eq!(p.bold_fg("#fff", "x"), bold_fg("#fff", "x"));
    }

    #[test]
    fn strip_color_markup_drops_color_only() {
        // A span that only carried a color disappears entirely.
        assert_eq!(
            strip_color_markup("<span foreground='#f00'>hi</span>"),
            "hi"
        );
        // Structural attributes keep the span alive, minus the color.
        assert_eq!(
            strip_color_markup("<span font_weight='bold' foreground=\"#f00\">hi</span>"),
            "<span font_weight='bold'>hi</span>"
        );
        // Other tags and plain text pass through.
        assert_eq!(strip_color_markup("<b>a</b> b%"), "<b>a</b> b%");
        // Nesting pops the right tag.
        assert_eq!(
            strip_color_markup("<span size='large'><span color='#0f0'>x</span>y</span>"),
            "<span size='large'>xy</span>"
        );
        // Backgrounds and bare values count as color too.
        assert_eq!(strip_color_markup("<span bgcolor=red>x</span>"), "x");
        // Nothing to do on already-plain text.
        assert_eq!(strip_color_markup("🖨 54%"), "🖨 54%");
        // Malformed markup is left alone rather than mangled.
        assert_eq!(strip_color_markup("a < b"), "a < b");
    }
}
