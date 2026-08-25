//! IPP source. Network (`ipp://host/<path>`) and local CUPS (`ipp://localhost:631/printers/<q>`)
//! share ONE parser. The semantic mapping is a pure fn over a simple `AttrMap` (unit-tested);
//! the thin adapter just lifts the `ipp` crate's attribute model into that map.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::model::{Color, Level, PrinterState, Reason, Status, Supply, SupplyClass, SupplyKind};
use crate::sources::{Source, SourceKind, SourceOutcome, Target};

/// Minimal value model — what we extract from IPP attributes.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrVal {
    Int(i64),
    Str(String),
}

pub type AttrMap = HashMap<String, Vec<AttrVal>>;

fn ints(m: &AttrMap, key: &str) -> Vec<i64> {
    m.get(key)
        .map(|v| {
            v.iter()
                .filter_map(|x| {
                    if let AttrVal::Int(i) = x {
                        Some(*i)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
fn strs<'a>(m: &'a AttrMap, key: &str) -> Vec<&'a str> {
    m.get(key)
        .map(|v| {
            v.iter()
                .filter_map(|x| {
                    if let AttrVal::Str(s) = x {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
fn first_int(m: &AttrMap, key: &str) -> Option<i64> {
    ints(m, key).into_iter().next()
}
fn first_str<'a>(m: &'a AttrMap, key: &str) -> Option<&'a str> {
    strs(m, key).into_iter().next()
}

fn map_status(state: Option<i64>) -> Option<Status> {
    match state {
        Some(3) => Some(Status::Idle),
        Some(4) => Some(Status::Printing),
        Some(5) => Some(Status::Stopped),
        _ => None,
    }
}

fn map_reason(raw: &str) -> Option<Reason> {
    let base = raw
        .trim_end_matches("-warning")
        .trim_end_matches("-error")
        .trim_end_matches("-report");
    if base.is_empty() || base == "none" {
        return None;
    }
    let r = if base.contains("jam") {
        Reason::Jam
    } else if base.contains("cover-open")
        || base.contains("door-open")
        || base.contains("interlock")
    {
        Reason::CoverOpen
    } else if base.contains("media-low") || base.contains("input-media-supply-low") {
        Reason::MediaLow
    } else if base.contains("media-empty")
        || base.contains("media-needed")
        || base.contains("input-tray-missing")
    {
        Reason::MediaEmpty
    } else if (base.contains("waste") && base.contains("full"))
        || base.contains("toner-empty")
        || base.contains("marker-supply-empty")
        || base.contains("developer-empty")
    {
        Reason::SupplyEmpty
    } else if base.contains("toner-low")
        || base.contains("marker-supply-low")
        || base.contains("developer-low")
        || base.contains("waste")
    {
        Reason::SupplyLow
    } else if base.contains("offline") || base.contains("shutdown") {
        Reason::Offline
    } else {
        Reason::Other(base.to_string())
    };
    Some(r)
}

fn map_kind(t: &str) -> SupplyKind {
    let l = t.to_lowercase();
    if l.contains("waste") {
        SupplyKind::Waste
    } else if l.contains("toner") {
        SupplyKind::Toner
    } else if l.contains("ink") {
        SupplyKind::Ink
    } else if l.contains("opc")
        || l.contains("drum")
        || l.contains("photoconductor")
        || l.contains("imaging")
    {
        SupplyKind::Drum
    } else {
        SupplyKind::Other
    }
}

fn map_color(s: &str) -> Option<Color> {
    let l = s.trim().to_lowercase();
    match l.as_str() {
        "black" | "#000000" | "#000" | "k" => Some(Color::Black),
        "cyan" | "#00ffff" | "c" => Some(Color::Cyan),
        "magenta" | "#ff00ff" | "m" => Some(Color::Magenta),
        "yellow" | "#ffff00" | "y" => Some(Color::Yellow),
        "" | "none" | "unknown" => None,
        s if s.contains("tri") || s.contains("color") => Some(Color::TriColor),
        s if s.contains("photo") => Some(Color::Photo),
        _ => Some(Color::Other),
    }
}

fn map_level(raw: Option<i64>) -> Level {
    match raw {
        Some(-1) => Level::NoRestriction,
        Some(-3) => Level::SomeRemaining,
        Some(v) if v >= 0 => Level::Pct(v.min(100) as u8),
        _ => Level::Unknown, // -2 and any other negative / missing
    }
}

/// The attributes this program actually reads. Requesting only these keeps a
/// cooperative printer's answer small; `MAX_ATTRS` and its siblings are what
/// hold when a printer answers with more than it was asked for.
const WANTED: &[&str] = &[
    "marker-names",
    "marker-levels",
    "marker-colors",
    "marker-types",
    "printer-state",
    "printer-state-reasons",
    "printer-info",
    "printer-make-and-model",
    "queued-job-count",
];

/// Pure semantic mapping from IPP attributes to a partial printer view.
pub fn parse_attrs(m: &AttrMap) -> PrinterState {
    let names = strs(m, "marker-names");
    let levels = ints(m, "marker-levels");
    let colors = strs(m, "marker-colors");
    let types = strs(m, "marker-types");
    let n = names.len().max(levels.len());

    let mut supplies = Vec::new();
    for i in 0..n {
        let type_s = types.get(i).copied().unwrap_or("");
        let kind = map_kind(type_s);
        let class = if kind == SupplyKind::Waste {
            SupplyClass::Filled
        } else {
            SupplyClass::Consumed
        };
        supplies.push(Supply {
            name: names
                .get(i)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("Supply {}", i + 1)),
            kind,
            class,
            color_raw: colors.get(i).map(|s| s.to_string()),
            color: colors.get(i).and_then(|s| map_color(s)),
            level: map_level(levels.get(i).copied()),
            max_capacity: None,
            unit: None,
        });
    }

    let mut reasons = Vec::new();
    for r in strs(m, "printer-state-reasons") {
        if let Some(reason) = map_reason(r) {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    }

    PrinterState {
        name: first_str(m, "printer-info").map(|s| s.to_string()),
        model: first_str(m, "printer-make-and-model").map(|s| s.to_string()),
        status: map_status(first_int(m, "printer-state")),
        reasons,
        supplies,
        paper: Vec::new(),
        pages: None,
        jobs: first_int(m, "queued-job-count").map(|j| j.max(0) as u32),
        display: None,
    }
}

// ---- thin adapter over the `ipp` crate (network I/O; not unit-tested) ----

/// Bracket a bare IPv6 literal for use in a URL authority (no-op if already bracketed or v4).
fn bracket_ipv6(host: &str) -> String {
    if host.matches(':').count() >= 2 && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// One IPP source. `kind` is `Ipp` for a network host or `Cups` for a local queue.
pub struct IppSource {
    pub kind: SourceKind,
}

impl Source for IppSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn collect(&self, target: &Target) -> SourceOutcome {
        let start = Instant::now();
        let uri = match self.kind {
            SourceKind::Cups => format!(
                "ipp://localhost:631/printers/{}",
                target.cups.as_deref().unwrap_or("")
            ),
            _ => format!(
                "ipp://{}{}",
                bracket_ipv6(target.host.as_deref().unwrap_or("")),
                target.ipp_path
            ),
        };
        match query(&uri, target.timeout) {
            Ok(map) => SourceOutcome {
                kind: self.kind,
                partial: parse_attrs(&map),
                duration: start.elapsed(),
                error: None,
            },
            Err(e) => SourceOutcome::failed(self.kind, e, start.elapsed()),
        }
    }
}

fn query(uri_str: &str, timeout: Duration) -> Result<AttrMap, String> {
    use ipp::attribute::IppAttributeGroup;
    use ipp::model::DelimiterTag;
    use ipp::prelude::*;
    use ipp::value::IppValue;

    fn ipp_val(v: &IppValue) -> Option<AttrVal> {
        match v {
            IppValue::Integer(i) | IppValue::Enum(i) => Some(AttrVal::Int(*i as i64)),
            IppValue::Keyword(k) => Some(AttrVal::Str(k.to_string())),
            IppValue::NameWithoutLanguage(s) => Some(AttrVal::Str(s.to_string())),
            IppValue::TextWithoutLanguage(s) => Some(AttrVal::Str(s.to_string())),
            IppValue::OctetString(s) => Some(AttrVal::Str(s.to_string())),
            IppValue::Uri(s) => Some(AttrVal::Str(s.to_string())),
            IppValue::TextWithLanguage { text, .. } => Some(AttrVal::Str(text.to_string())),
            IppValue::NameWithLanguage { name, .. } => Some(AttrVal::Str(name.to_string())),
            IppValue::Boolean(b) => Some(AttrVal::Str(b.to_string())),
            _ => None,
        }
    }

    /// What is kept out of a printer's answer.
    ///
    /// A printer is a device on the network, and this program runs inside the
    /// long-lived omarchy-shell process — so "the printer would not do that" is
    /// not a bound. A printer that is hostile, compromised, or merely broken can
    /// answer with an attribute set that never ends, and everything retained
    /// here is later serialized into the JSON the shell reads.
    ///
    /// These stop the RETENTION. `request_timeout` on the client stops the
    /// transfer. What neither stops is the crate's own peak while it parses a
    /// group, which it accumulates before this code sees it — bounded in
    /// practice by that timeout, not by a byte count.
    const MAX_ATTRS: usize = 512;
    const MAX_VALS_PER_ATTR: usize = 256;
    const MAX_STR: usize = 4096;

    fn group_to_map(group: &IppAttributeGroup) -> AttrMap {
        let mut m = AttrMap::new();
        for (name, attr) in group.attributes().iter().take(MAX_ATTRS) {
            let vals: Vec<AttrVal> = attr
                .value()
                .into_iter()
                .take(MAX_VALS_PER_ATTR)
                .filter_map(ipp_val)
                .map(|v| match v {
                    // A single attribute value is a name, a model or a state
                    // reason. Anything past this is not one of those.
                    AttrVal::Str(s) if s.len() > MAX_STR => {
                        AttrVal::Str(s.chars().take(MAX_STR).collect())
                    }
                    other => other,
                })
                .collect();
            m.insert(name.chars().take(MAX_STR).collect(), vals);
        }
        m
    }

    let uri: Uri = uri_str
        .parse()
        .map_err(|e| format!("bad uri {uri_str}: {e}"))?;
    // Ask for the attributes this program reads, and no others. A cooperative
    // printer then sends a small answer instead of its whole catalogue, which
    // is the cheap half of bounding this response. The caps below are the half
    // that survives a printer which ignores the request.
    let op = IppOperationBuilder::get_printer_attributes(uri.clone())
        .attributes(WANTED)
        .build()
        .map_err(|e| format!("build op: {e}"))?;
    let client = IppClient::builder(uri).request_timeout(timeout).build();
    let resp = client.send(op).map_err(|e| format!("ipp send: {e}"))?;
    if !resp.header().status_code().is_success() {
        return Err(format!("ipp status {:?}", resp.header().status_code()));
    }
    let group = resp
        .attributes()
        .groups_of(DelimiterTag::PrinterAttributes)
        .next()
        .ok_or_else(|| "no printer-attributes group".to_string())?;
    Ok(group_to_map(group))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, Vec<AttrVal>)]) -> AttrMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }
    fn s(x: &str) -> AttrVal {
        AttrVal::Str(x.into())
    }
    fn i(x: i64) -> AttrVal {
        AttrVal::Int(x)
    }

    #[test]
    fn parses_m477_laser_fixture() {
        let map = m(&[
            ("printer-state", vec![i(3)]),
            ("printer-state-reasons", vec![s("none")]),
            (
                "marker-names",
                vec![
                    s("Black Cartridge"),
                    s("Cyan Cartridge"),
                    s("Magenta Cartridge"),
                    s("Yellow Cartridge"),
                ],
            ),
            (
                "marker-colors",
                vec![s("#000000"), s("#00FFFF"), s("#FF00FF"), s("#FFFF00")],
            ),
            ("marker-levels", vec![i(73), i(54), i(81), i(69)]),
            (
                "marker-types",
                vec![s("toner"), s("toner"), s("toner"), s("toner")],
            ),
            ("queued-job-count", vec![i(0)]),
            (
                "printer-make-and-model",
                vec![s("HP Color LaserJet MFP M477fdw")],
            ),
        ]);
        let st = parse_attrs(&map);
        assert_eq!(st.status, Some(Status::Idle));
        assert_eq!(st.jobs, Some(0));
        assert!(st.model.as_deref().unwrap().contains("M477"));
        assert_eq!(st.supplies.len(), 4);
        assert_eq!(st.supplies[0].color, Some(Color::Black));
        assert_eq!(st.supplies[0].kind, SupplyKind::Toner);
        assert_eq!(st.supplies[0].level, Level::Pct(73));
        assert!(st.supplies[0].is_usable());
    }

    #[test]
    fn parses_inkjet_with_tricolor() {
        let map = m(&[
            ("printer-state", vec![i(4)]),
            ("marker-names", vec![s("Black Ink"), s("Tri-color Ink")]),
            ("marker-colors", vec![s("black"), s("tri-color")]),
            ("marker-levels", vec![i(40), i(60)]),
            ("marker-types", vec![s("ink"), s("ink")]),
        ]);
        let st = parse_attrs(&map);
        assert_eq!(st.status, Some(Status::Printing));
        assert_eq!(st.supplies.len(), 2);
        assert!(st.supplies.iter().all(|s| s.kind == SupplyKind::Ink));
        assert_eq!(st.supplies[1].color, Some(Color::TriColor));
    }

    #[test]
    fn malformed_short_arrays_do_not_panic() {
        let map = m(&[
            ("marker-names", vec![s("A"), s("B"), s("C"), s("D")]),
            ("marker-levels", vec![i(50), i(60)]),
            ("marker-types", vec![s("toner")]),
        ]);
        let st = parse_attrs(&map);
        assert_eq!(st.supplies.len(), 4);
        assert_eq!(st.supplies[2].level, Level::Unknown);
        assert!(!st.supplies[2].is_usable());
    }

    #[test]
    fn sentinels_and_reasons() {
        let map = m(&[
            ("printer-state", vec![i(5)]),
            (
                "printer-state-reasons",
                vec![s("media-jam"), s("toner-low-warning"), s("none")],
            ),
            ("marker-names", vec![s("Black")]),
            ("marker-levels", vec![i(-2)]),
            ("marker-types", vec![s("toner")]),
        ]);
        let st = parse_attrs(&map);
        assert_eq!(st.status, Some(Status::Stopped));
        assert_eq!(st.reasons, vec![Reason::Jam, Reason::SupplyLow]);
        assert_eq!(st.supplies[0].level, Level::Unknown);
    }
}
