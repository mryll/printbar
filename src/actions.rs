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

/// The CUPS queue page (job list) when a local queue is configured, else the EWS.
pub fn queue_url(pc: &PrinterConfig) -> Result<String, String> {
    if let Some(q) = &pc.cups {
        Ok(format!("http://localhost:631/printers/{q}"))
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
        let p = pc("[printer.x]\nhost=\"192.168.1.70\"\n");
        assert_eq!(ews_url(&p).unwrap(), "http://192.168.1.70");
    }

    #[test]
    fn ews_explicit_url_wins() {
        let p = pc("[printer.x]\nhost=\"192.168.1.70\"\n[printer.x.actions]\news_url=\"https://printer.local:443\"\n");
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
}
