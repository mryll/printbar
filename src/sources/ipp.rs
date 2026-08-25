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

/// What is kept out of a printer's answer.
///
/// A printer is a device on the network, and this program runs inside the
/// long-lived omarchy-shell process — so "the printer would not do that" is not
/// a bound. A printer that is hostile, compromised, or merely broken can answer
/// with an attribute set that never ends, and everything retained here is later
/// serialized into the JSON the shell reads.
///
/// These stop the RETENTION. `request_timeout` on the client stops the
/// transfer. What neither stops is the crate's own peak while it parses a group,
/// which it accumulates before this code sees it — bounded in practice by that
/// timeout, not by a byte count.
const MAX_ATTRS: usize = 512;
const MAX_VALS_PER_ATTR: usize = 256;
const MAX_STR: usize = 4096;
/// The one that actually bounds the answer.
///
/// The three caps above are per-item, and per-item caps multiply: 512 × 256 ×
/// 4096 is 512 MiB, which is not a bound at all. This is the budget for the
/// whole group. Retention stops when it runs out, so a printer cannot spend a
/// little at a time and add up to something large.
const MAX_TOTAL_CHARS: usize = 256 * 1024;

/// Apply the retention caps to one group's worth of attributes.
///
/// Split out of the IPP call so a test can reach it: building a real
/// `IppAttributeGroup` needs the crate's types, and what needs proving here is
/// the arithmetic, not the crate's parser.
fn cap_attrs<I>(pairs: I) -> AttrMap
where
    I: IntoIterator<Item = (String, Vec<AttrVal>)>,
{
    let mut m = AttrMap::new();
    let mut budget = MAX_TOTAL_CHARS;
    for (name, vals) in pairs.into_iter().take(MAX_ATTRS) {
        if budget == 0 {
            break;
        }
        let name: String = name.chars().take(MAX_STR.min(budget)).collect();
        budget -= name.chars().count();
        let mut kept: Vec<AttrVal> = Vec::new();
        for v in vals.into_iter().take(MAX_VALS_PER_ATTR) {
            if budget == 0 {
                break;
            }
            kept.push(match v {
                // A single value is a name, a model or a state reason. Anything
                // past this is not one of those.
                AttrVal::Str(s) => {
                    let cut: String = s.chars().take(MAX_STR.min(budget)).collect();
                    budget -= cut.chars().count();
                    AttrVal::Str(cut)
                }
                // A number costs nothing worth counting.
                other => other,
            });
        }
        m.insert(name, kept);
    }
    m
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

    fn group_to_map(group: &IppAttributeGroup) -> AttrMap {
        cap_attrs(group.attributes().iter().map(|(name, attr)| {
            (
                name.to_string(),
                attr.value().into_iter().filter_map(ipp_val).collect::<Vec<_>>(),
            )
        }))
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

    // The caps exist because a printer is a device on the network, not because
    // any real printer misbehaves. Each of these describes what a reader would
    // see if one did.

    fn pairs(n: usize) -> Vec<(String, Vec<AttrVal>)> {
        (0..n).map(|k| (format!("attr-{k}"), vec![i(0)])).collect()
    }

    #[test]
    fn an_answer_within_the_caps_is_kept_whole() {
        let out = cap_attrs(pairs(10));
        assert_eq!(out.len(), 10);
        assert_eq!(out["attr-0"].len(), 1);
    }

    // The expected numbers below are written out, not taken from the
    // constants. A test that reads the same constant it is checking moves with
    // it and can never fail — which is how a cap raised by accident would ship.
    // Changing a bound here is meant to cost a deliberate edit to these tests.

    #[test]
    fn a_printer_that_sends_more_attributes_than_the_cap_has_the_rest_dropped() {
        let out = cap_attrs(pairs(612));
        assert_eq!(out.len(), 512);
    }

    #[test]
    fn an_attribute_with_more_values_than_the_cap_keeps_only_the_first_ones() {
        let many: Vec<AttrVal> = (0..306).map(|n| i(n as i64)).collect();
        let out = cap_attrs([("marker-names".to_string(), many)]);
        assert_eq!(out["marker-names"].len(), 256);
    }

    #[test]
    fn a_value_longer_than_the_cap_is_cut_to_it() {
        let long = "x".repeat(4596);
        let out = cap_attrs([("printer-info".to_string(), vec![s(&long)])]);
        match &out["printer-info"][0] {
            AttrVal::Str(v) => assert_eq!(v.chars().count(), 4096),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn a_value_that_is_not_a_string_is_left_alone_by_the_length_cap() {
        let out = cap_attrs([("printer-state".to_string(), vec![i(3)])]);
        assert_eq!(out["printer-state"], vec![i(3)]);
    }

    #[test]
    fn an_attribute_name_longer_than_the_cap_is_cut_to_it() {
        let long = "n".repeat(4106);
        let out = cap_attrs([(long, vec![i(1)])]);
        let key = out.keys().next().unwrap();
        assert_eq!(key.chars().count(), 4096);
    }

    #[test]
    fn a_multibyte_value_is_cut_by_characters_and_stays_valid_text() {
        // Cutting by bytes would split a codepoint and give back mojibake.
        let long = "ñ".repeat(4196);
        let out = cap_attrs([("printer-info".to_string(), vec![s(&long)])]);
        match &out["printer-info"][0] {
            AttrVal::Str(v) => {
                assert_eq!(v.chars().count(), 4096);
                assert!(v.chars().all(|c| c == 'ñ'));
            }
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn the_whole_group_cannot_cost_more_than_the_budget() {
        // The per-item caps multiply: 512 attributes of 256 values of 4096
        // characters is 512 MiB. This is the number that holds.
        let huge: Vec<(String, Vec<AttrVal>)> = (0..500)
            .map(|k| {
                (
                    format!("attr-{k}"),
                    (0..200).map(|_| s(&"x".repeat(4000))).collect(),
                )
            })
            .collect();
        let out = cap_attrs(huge);
        let total: usize = out
            .iter()
            .map(|(k, vs)| {
                k.chars().count()
                    + vs.iter()
                        .map(|v| match v {
                            AttrVal::Str(x) => x.chars().count(),
                            _ => 0,
                        })
                        .sum::<usize>()
            })
            .sum();
        assert!(total <= 262144, "kept {total} characters");
    }

    #[test]
    fn a_printer_cannot_spend_the_budget_a_little_at_a_time() {
        // Every value is under every per-item cap. Only the running budget
        // stops this one.
        let many: Vec<(String, Vec<AttrVal>)> = (0..500)
            .map(|k| (format!("a{k}"), vec![s(&"y".repeat(1000))]))
            .collect();
        let out = cap_attrs(many);
        let total: usize = out
            .values()
            .flatten()
            .map(|v| match v {
                AttrVal::Str(x) => x.chars().count(),
                _ => 0,
            })
            .sum();
        assert!(total <= 262144, "kept {total} characters");
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
