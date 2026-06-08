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
        println!("{}", serde_json::to_string(self).unwrap_or_else(|_| error_output("serialize")));
    }
}

/// Valid Waybar JSON for any fatal error — keeps the exit-0 contract.
pub fn error_output(reason: &str) -> String {
    let out = WaybarOutput {
        text: "?".into(),
        tooltip: reason.into(),
        class: vec!["error".into()],
        alt: "error".into(),
    };
    serde_json::to_string(&out)
        .unwrap_or_else(|_| r#"{"text":"?","tooltip":"error","class":["error"],"alt":"error"}"#.into())
}

pub fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn fg(color: &str, text: &str) -> String {
    format!("<span foreground='{color}'>{text}</span>")
}

pub fn bold_fg(color: &str, text: &str) -> String {
    format!("<span font_weight='bold' foreground='{color}'>{text}</span>")
}

pub fn border_line(content: &str, width: usize, border_color: &str) -> String {
    let pad = width.saturating_sub(visible_len(content));
    let right_pad = " ".repeat(pad);
    format!("{} {content}{right_pad} {}", fg(border_color, "│"), fg(border_color, "│"))
}

pub fn separator(width: usize, border_color: &str, dim_color: &str) -> String {
    border_line(&fg(dim_color, &"─".repeat(width)), width, border_color)
}

pub fn top_border(width: usize, border_color: &str) -> String {
    fg(border_color, &format!("╭{}╮", "─".repeat(width + 2)))
}

pub fn bottom_border(width: usize, border_color: &str) -> String {
    fg(border_color, &format!("╰{}╯", "─".repeat(width + 2)))
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
    fn fg_wraps_in_pango_span() {
        assert_eq!(fg("#fff", "x"), "<span foreground='#fff'>x</span>");
    }

    #[test]
    fn visible_len_ignores_markup_and_entities() {
        assert_eq!(visible_len("<span foreground='#fff'>ab</span>"), 2);
        assert_eq!(visible_len("a&amp;b"), 3); // a + entity(1) + b
    }
}
