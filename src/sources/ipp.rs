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
/// These stop the RETENTION. `MAX_WIRE_BYTES` in `query` stops the transfer
/// itself: the HTTP body is read through a `take` and buffered BEFORE
/// `IppParser` sees a byte, so the crate's peak while it parses is bounded by
/// that buffer, not by trust in the printer.
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

/// What the HTTP body of one IPP response may cost, in bytes.
///
/// The crate's own client hands the whole body straight to `IppParser`, so a
/// printer that streams for ever grows the parser's buffers until the request
/// timeout fires — elapsed time was the only bound, accumulated bytes had
/// none. Reading the wire through a `take` and buffering at most this many
/// bytes puts the limit BEFORE the parser: the peak this response can cost is
/// `MAX_WIRE_BYTES`, no matter what the printer sends. A real answer to the
/// nine attributes in `WANTED` is a few KiB.
const MAX_WIRE_BYTES: u64 = 2 * 1024 * 1024;

/// `ipp://host[:port]/path` to the `http://` URL the transfer actually uses.
///
/// The crate has this conversion too, but private to its client. printbar only
/// ever builds `ipp://` URIs (see `collect`), so anything else is refused by
/// name instead of guessed at.
fn ipp_http_url(uri: &ipp::prelude::Uri) -> Result<String, String> {
    if uri.scheme_str() != Some("ipp") {
        return Err(format!("unsupported scheme in {uri}: only ipp:// is built in"));
    }
    let authority = uri.authority().ok_or_else(|| format!("no host in {uri}"))?;
    let authority = if authority.port_u16().is_some() {
        authority.to_string()
    } else {
        format!("{authority}:631")
    };
    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or_default();
    Ok(format!("http://{authority}{path_and_query}"))
}

fn query(uri_str: &str, timeout: Duration) -> Result<AttrMap, String> {
    use std::io::Read;

    use ipp::attribute::IppAttributeGroup;
    use ipp::model::DelimiterTag;
    use ipp::parser::IppParser;
    use ipp::prelude::*;
    use ipp::reader::IppReader;
    use ipp::request::IppRequestResponse;
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
    // The send is done by hand instead of through `IppClient` for one reason:
    // the crate's client pipes the HTTP body straight into `IppParser`, with
    // nothing between the printer and the parser's buffers. Same transport
    // (ureq, plain http, connect + global timeouts), plus a byte limit on the
    // wire. Reading one byte past the limit is what tells a body that fits
    // from one the cap truncated.
    let url = ipp_http_url(&uri)?;
    let request: IppRequestResponse = op.into();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .build()
        .into();
    let response = agent
        .post(&url)
        .header("content-type", "application/ipp")
        .send(ureq::SendBody::from_reader(&mut request.into_read()))
        .map_err(|e| format!("ipp send: {e}"))?;
    let mut body: Vec<u8> = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_WIRE_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("ipp read: {e}"))?;
    if body.len() as u64 > MAX_WIRE_BYTES {
        return Err(format!("ipp response is larger than {MAX_WIRE_BYTES} bytes"));
    }
    let resp = IppParser::new(IppReader::new(std::io::Cursor::new(body)))
        .parse()
        .map_err(|e| format!("ipp parse: {e}"))?;
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
        // Enough to blow the budget many times over without building half a
        // gigabyte to prove it: this runs in CI, and a test that needs 400 MB
        // to make its point is a test that gets deleted.
        let huge: Vec<(String, Vec<AttrVal>)> = (0..200)
            .map(|k| {
                (
                    format!("attr-{k}"),
                    (0..20).map(|_| s(&"x".repeat(4000))).collect(),
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

    // --- the wire limit -------------------------------------------------

    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::time::Duration;

    /// Serve one HTTP connection with `response` and return the ipp:// uri.
    /// The request is drained until the chunked terminator so the client is
    /// never mid-write when the response lands.
    fn serve_once(response: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut seen: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match sock.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            seen.extend_from_slice(&buf[..n]);
                            if seen.windows(5).any(|w| w == b"0\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let _ = sock.write_all(&response);
            }
        });
        format!("ipp://127.0.0.1:{port}/ipp/print")
    }

    fn http_response(body: &[u8]) -> Vec<u8> {
        let mut r = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/ipp\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        r.extend_from_slice(body);
        r
    }

    /// query() on a thread with a deadline, so a regression that hangs fails
    /// the test in seconds instead of hanging the suite.
    fn query_with_deadline(uri: String) -> Result<AttrMap, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(query(&uri, Duration::from_secs(30)));
        });
        rx.recv_timeout(Duration::from_secs(20))
            .expect("query did not return before the deadline")
    }

    #[test]
    fn a_printer_answer_that_fits_the_wire_limit_parses_as_before() {
        use ipp::attribute::IppAttribute;
        use ipp::model::{DelimiterTag, IppVersion, StatusCode};
        use ipp::request::IppRequestResponse;
        use ipp::value::IppValue;

        let mut resp =
            IppRequestResponse::new_response(IppVersion::v1_1(), StatusCode::SuccessfulOk, 1)
                .unwrap();
        resp.attributes_mut().add(
            DelimiterTag::PrinterAttributes,
            IppAttribute::new("printer-state".parse().unwrap(), IppValue::Enum(3)),
        );
        let mut body = Vec::new();
        resp.into_read().read_to_end(&mut body).unwrap();

        let uri = serve_once(http_response(&body));
        let map = query_with_deadline(uri).expect("a small real answer must parse");
        assert_eq!(first_int(&map, "printer-state"), Some(3));
    }

    #[test]
    fn a_response_larger_than_the_wire_limit_is_refused_by_size_not_parsed() {
        // The numbers are written out on purpose: a test that reads
        // MAX_WIRE_BYTES moves with it and can never fail.
        let uri = serve_once(http_response(&vec![0u8; 2 * 1024 * 1024 + 100]));
        let e = query_with_deadline(uri).unwrap_err();
        assert!(
            e.contains("larger than 2097152 bytes"),
            "expected the size refusal, got: {e}"
        );
    }

    #[test]
    fn a_printer_that_streams_chunks_for_ever_is_cut_at_the_wire_limit() {
        // No content-length and no end: the one shape only a byte limit stops.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = l.accept() {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf);
                if sock
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/ipp\r\ntransfer-encoding: chunked\r\n\r\n")
                    .is_err()
                {
                    return;
                }
                let chunk = format!("{:x}\r\n{}\r\n", 8192, "x".repeat(8192));
                loop {
                    if sock.write_all(chunk.as_bytes()).is_err() {
                        return; // the client hung up: the cap did its work
                    }
                }
            }
        });
        let uri = format!("ipp://127.0.0.1:{port}/ipp/print");
        let e = query_with_deadline(uri).unwrap_err();
        assert!(
            e.contains("larger than 2097152 bytes"),
            "expected the size refusal, got: {e}"
        );
    }

    #[test]
    fn an_ipp_uri_with_no_port_gets_the_ipp_default_631() {
        let uri: ipp::prelude::Uri = "ipp://printer.lan/ipp/print".parse().unwrap();
        assert_eq!(
            ipp_http_url(&uri).unwrap(),
            "http://printer.lan:631/ipp/print"
        );
    }

    #[test]
    fn an_ipp_uri_keeps_the_port_it_names() {
        let uri: ipp::prelude::Uri = "ipp://10.0.0.5:8631/x".parse().unwrap();
        assert_eq!(ipp_http_url(&uri).unwrap(), "http://10.0.0.5:8631/x");
    }

    #[test]
    fn a_scheme_that_is_not_ipp_is_refused_by_name() {
        let uri: ipp::prelude::Uri = "https://printer.lan/ipp".parse().unwrap();
        let e = ipp_http_url(&uri).unwrap_err();
        assert!(e.contains("only ipp:// is built in"), "got: {e}");
    }
}
