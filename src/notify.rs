//! Desktop notifications on state *transitions* (not steady state). Best-effort:
//! shells out to `notify-send`; a missing cache dir or notifier never fails the poll.

use crate::config::PrinterConfig;
use crate::model::{PrinterState, Reason, Status, SupplyClass};
use std::path::PathBuf;

/// The set of currently-active notifiable events for a printer state.
pub fn active_events(state: &PrinterState, cfg: &PrinterConfig) -> Vec<String> {
    let mut e = Vec::new();
    let has = |r: &Reason| state.reasons.contains(r);
    if has(&Reason::Jam) {
        e.push("jam".into());
    }
    if has(&Reason::MediaEmpty) {
        e.push("media_empty".into());
    }
    if has(&Reason::CoverOpen) {
        e.push("cover_open".into());
    }
    if state.status == Some(Status::Offline) || has(&Reason::Offline) {
        e.push("offline".into());
    }
    let low = cfg.thresholds.supply_low;
    let supply_low = has(&Reason::SupplyLow)
        || state.supplies.iter().any(|s| {
            s.class == SupplyClass::Consumed && s.level.as_pct().is_some_and(|p| p <= low)
        });
    if supply_low {
        e.push("supply_low".into());
    }
    if has(&Reason::SupplyEmpty) {
        e.push("supply_empty".into());
    }
    e
}

/// Newly-active events that are configured to notify (transition = in `cur`, not in `prev`).
pub fn diff_events(prev: &[String], cur: &[String], configured: &[String]) -> Vec<String> {
    cur.iter()
        .filter(|e| !prev.contains(e) && configured.iter().any(|c| c == *e))
        .cloned()
        .collect()
}

/// Cache file path for a printer's last event set, with the name sanitized.
pub fn cache_path(name: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    dir.join(format!("printbar-{safe}.json"))
}

fn load_prev(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cur(path: &std::path::Path, cur: &[String]) {
    // Atomic: write to a temp sibling then rename.
    let tmp = path.with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string(cur) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn emit(event: &str, printer: &str) {
    let msg = match event {
        "jam" => "Paper jam",
        "media_empty" => "Out of paper",
        "cover_open" => "Cover open",
        "offline" => "Printer offline",
        "supply_low" => "Supply low",
        "supply_empty" => "Supply empty",
        _ => event,
    };
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "printbar", printer, msg])
        .spawn();
}

/// Best-effort: fire notifications for new events, update the cache. Never errors.
pub fn maybe_notify(name: &str, cfg: &PrinterConfig, state: &PrinterState) {
    if !cfg.notify.enabled {
        return;
    }
    let path = cache_path(name);
    let prev = load_prev(&path);
    let cur = active_events(state, cfg);
    for ev in diff_events(&prev, &cur, &cfg.notify.events) {
        emit(&ev, name);
    }
    save_cur(&path, &cur);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_fires_once() {
        let cfg = vec!["jam".to_string()];
        // prev empty, cur has jam → fires
        assert_eq!(
            diff_events(&[], &["jam".into()], &cfg),
            vec!["jam".to_string()]
        );
        // prev already jam, cur still jam → nothing (steady state)
        assert!(diff_events(&["jam".into()], &["jam".into()], &cfg).is_empty());
    }

    #[test]
    fn only_configured_events_fire() {
        let cfg = vec!["jam".to_string()];
        // offline became active but isn't configured → not fired
        assert!(diff_events(&[], &["offline".into()], &cfg).is_empty());
    }

    #[test]
    fn cache_path_sanitizes_name() {
        let p = cache_path("../evil/name");
        let fname = p.file_name().unwrap().to_str().unwrap();
        assert!(!fname.contains('/'));
        assert!(!fname.contains(".."));
        assert_eq!(fname, "printbar-___evil_name.json");
    }
}
