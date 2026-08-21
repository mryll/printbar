//! Click actions: open the printer's EWS (web panel) or its CUPS queue page.

use crate::config::PrinterConfig;

/// Build the EWS URL: an explicit `ews_url` wins; otherwise `http://<host>`,
/// bracketing bare IPv6 literals.
pub fn ews_url(pc: &PrinterConfig) -> Result<String, String> {
    if let Some(u) = &pc.actions.ews_url {
        return Ok(u.clone());
    }
    let host = pc.host.as_deref().ok_or("ews: no host configured")?;
    if host.contains("://") {
        return Ok(host.to_string());
    }
    // Bare IPv6 literal (has ':' but isn't host:port and isn't a v4 address) → bracket it.
    let looks_v6 = host.matches(':').count() >= 2 && !host.starts_with('[');
    let h = if looks_v6 {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Ok(format!("http://{h}"))
}

/// Percent-encode a string as a URL *path segment* (RFC 3986): every byte
/// outside the unreserved set is encoded, so a queue name can't inject path
/// separators, queries, or fragments into a generated URL.
fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The CUPS queue page (job list) when a local queue is configured, else the EWS.
pub fn queue_url(pc: &PrinterConfig) -> Result<String, String> {
    if let Some(q) = &pc.cups {
        Ok(format!(
            "http://localhost:631/printers/{}",
            encode_path_segment(q)
        ))
    } else {
        ews_url(pc)
    }
}

/// Open the URL for the given action, best-effort (never blocks the caller).
pub fn run(action: &str, pc: &PrinterConfig) -> Result<(), String> {
    let url = match action {
        "ews" => ews_url(pc)?,
        "queue" => queue_url(pc)?,
        other => return Err(format!("unknown action '{other}'")),
    };
    std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("xdg-open: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn pc(toml: &str) -> PrinterConfig {
        Config::parse(toml).unwrap().printer.remove("x").unwrap()
    }

    #[test]
    fn ews_from_host() {
        let p = pc("[printer.x]\nhost=\"192.0.2.70\"\n");
        assert_eq!(ews_url(&p).unwrap(), "http://192.0.2.70");
    }

    #[test]
    fn ews_explicit_url_wins() {
        let p = pc("[printer.x]\nhost=\"192.0.2.70\"\n[printer.x.actions]\news_url=\"https://printer.local:443\"\n");
        assert_eq!(ews_url(&p).unwrap(), "https://printer.local:443");
    }

    #[test]
    fn ews_brackets_ipv6() {
        let p = pc("[printer.x]\nhost=\"fe80::1\"\n");
        assert_eq!(ews_url(&p).unwrap(), "http://[fe80::1]");
    }

    #[test]
    fn queue_uses_cups() {
        let p = pc("[printer.x]\nhost=\"h\"\ncups=\"HP_M477fdw\"\n");
        assert_eq!(
            queue_url(&p).unwrap(),
            "http://localhost:631/printers/HP_M477fdw"
        );
    }

    #[test]
    fn queue_name_is_path_segment_encoded() {
        let p = pc("[printer.x]\ncups=\"Sala 2/Läser?x=1#f\"\n");
        assert_eq!(
            queue_url(&p).unwrap(),
            "http://localhost:631/printers/Sala%202%2FL%C3%A4ser%3Fx%3D1%23f"
        );
    }
}
