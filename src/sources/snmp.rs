//! SNMP enrichment source (Printer MIB, RFC 3805). Pure table reconstruction over an
//! `OidMap` (unit-tested with fixtures) + a thin `snmp2` v2c walk adapter. SNMP is purely
//! additive: any failure or partial data must never degrade the IPP/CUPS base output.

use std::collections::HashMap;
use std::time::Instant;

use crate::model::{
    Color, InputTray, Level, PrinterState, Reason, Status, Supply, SupplyClass, SupplyKind,
    SupplyUnit,
};
use crate::sources::{Source, SourceKind, SourceOutcome, Target};

#[derive(Debug, Clone, PartialEq)]
pub enum SnmpVal {
    Int(i64),
    Str(String),
}

pub type OidMap = HashMap<String, SnmpVal>;

// Printer MIB column OIDs (dotted, RFC 3805 / Host Resources MIB).
const SUP: &str = "1.3.6.1.2.1.43.11.1.1"; // prtMarkerSuppliesEntry
const SUP_COLORANT_IDX: &str = "1.3.6.1.2.1.43.11.1.1.3";
const SUP_CLASS: &str = "1.3.6.1.2.1.43.11.1.1.4";
const SUP_TYPE: &str = "1.3.6.1.2.1.43.11.1.1.5";
const SUP_DESC: &str = "1.3.6.1.2.1.43.11.1.1.6";
const SUP_UNIT: &str = "1.3.6.1.2.1.43.11.1.1.7";
const SUP_MAXCAP: &str = "1.3.6.1.2.1.43.11.1.1.8";
const SUP_LEVEL: &str = "1.3.6.1.2.1.43.11.1.1.9";
const COLORANT_VALUE: &str = "1.3.6.1.2.1.43.12.1.1.4"; // prtMarkerColorantValue
const LIFE_COUNT: &str = "1.3.6.1.2.1.43.10.2.1.4"; // prtMarkerLifeCount
const IN_MAXCAP: &str = "1.3.6.1.2.1.43.8.2.1.9";
const IN_LEVEL: &str = "1.3.6.1.2.1.43.8.2.1.10";
const IN_NAME: &str = "1.3.6.1.2.1.43.8.2.1.13";
const ALERT_SEVERITY: &str = "1.3.6.1.2.1.43.18.1.1.2";
const ALERT_DESC: &str = "1.3.6.1.2.1.43.18.1.1.8";
const PRINTER_STATUS: &str = "1.3.6.1.2.1.25.3.5.1.1"; // hrPrinterStatus
const CONSOLE_TEXT: &str = "1.3.6.1.2.1.43.16.5.1.2"; // prtConsoleDisplayBufferText

// Subtrees to walk (base OID + numeric components for getnext).
const WALK_BASES: &[(&str, &[u64])] = &[
    (SUP, &[1, 3, 6, 1, 2, 1, 43, 11, 1, 1]),
    (COLORANT_VALUE, &[1, 3, 6, 1, 2, 1, 43, 12, 1, 1, 4]),
    (LIFE_COUNT, &[1, 3, 6, 1, 2, 1, 43, 10, 2, 1, 4]),
    ("1.3.6.1.2.1.43.8.2.1", &[1, 3, 6, 1, 2, 1, 43, 8, 2, 1]),
    ("1.3.6.1.2.1.43.18.1.1", &[1, 3, 6, 1, 2, 1, 43, 18, 1, 1]),
    (PRINTER_STATUS, &[1, 3, 6, 1, 2, 1, 25, 3, 5, 1, 1]),
    // NOTE: prtConsoleDisplayBufferText is fetched via direct GET, not here — many agents
    // (e.g. HP) skip the console table on getnext, so a walk never reaches it.
];

const ROW_CAP: usize = 64;

/// What retention of one SNMP poll may cost.
///
/// Every string in the map came off the network: an `OctetString` can be as
/// large as the agent's message size, and the walk accepts up to
/// `WALK_VARBIND_CAP` varbinds for each of six subtrees — per-item caps
/// multiply, so without a total budget a hostile printer could park hundreds
/// of megabytes here during one poll. `retain` applies all three limits at the
/// single point where network data enters the map. The numbers mirror the IPP
/// side: 4096 characters for a value, 256 for an OID, `256 KiB` for the whole
/// poll.
const MAX_VAL_CHARS: usize = 4096;
const MAX_OID_CHARS: usize = 256;
const MAX_TOTAL_CHARS: usize = 256 * 1024;

/// Keep one varbind, under the caps. Returns false when the budget is spent —
/// the walk must stop retaining, not merely skip.
fn retain(map: &mut OidMap, budget: &mut usize, oid: String, v: SnmpVal) -> bool {
    if *budget == 0 {
        return false;
    }
    if oid.chars().count() > MAX_OID_CHARS {
        // A legitimate printer OID is a few dozen characters. Longer is not a
        // row this program reads, and its key would eat the budget.
        return true;
    }
    // The key spends first, so key plus value can never overshoot the
    // budget — the sum of everything retained stays under MAX_TOTAL_CHARS.
    let cost_key = oid.chars().count();
    if *budget <= cost_key {
        *budget = 0;
        return false;
    }
    let room = *budget - cost_key;
    let v = match v {
        SnmpVal::Str(t) => {
            let cut: String = t.chars().take(MAX_VAL_CHARS.min(room)).collect();
            SnmpVal::Str(cut)
        }
        other => other,
    };
    let cost_val = match &v {
        SnmpVal::Str(t) => t.chars().count(),
        _ => 0,
    };
    *budget -= cost_key + cost_val;
    map.insert(oid, v);
    true
}
/// Max varbinds to fetch per subtree walk. Must be comfortably above
/// ROW_CAP * (columns-before-the-last-used-one) so a multi-column table never truncates
/// before reaching its level column.
const WALK_VARBIND_CAP: usize = 2048;

fn as_int(v: &SnmpVal) -> Option<i64> {
    if let SnmpVal::Int(i) = v {
        Some(*i)
    } else {
        None
    }
}
fn as_str(v: &SnmpVal) -> Option<&str> {
    if let SnmpVal::Str(s) = v {
        Some(s)
    } else {
        None
    }
}
fn get<'a>(m: &'a OidMap, oid: &str) -> Option<&'a SnmpVal> {
    m.get(oid)
}

/// Entries of a column, as (row-index-suffix, value), sorted by suffix.
fn column<'a>(m: &'a OidMap, col: &str) -> Vec<(&'a str, &'a SnmpVal)> {
    let pfx = format!("{col}.");
    let mut out: Vec<(&str, &SnmpVal)> = m
        .iter()
        .filter_map(|(k, v)| k.strip_prefix(&pfx).map(|suf| (suf, v)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

fn map_class(class: Option<i64>, kind: SupplyKind) -> SupplyClass {
    match class {
        Some(4) => SupplyClass::Filled,   // receptacleThatIsFilled
        Some(3) => SupplyClass::Consumed, // supplyThatIsConsumed
        _ if kind == SupplyKind::Waste => SupplyClass::Filled,
        _ => SupplyClass::Consumed,
    }
}

fn map_type(t: Option<i64>) -> SupplyKind {
    match t {
        Some(3) | Some(21) => SupplyKind::Toner, // toner, tonerCartridge
        Some(4) | Some(8) | Some(14) | Some(32) => SupplyKind::Waste, // wasteToner/Ink/Wax/Paper
        Some(5) | Some(6) | Some(7) => SupplyKind::Ink, // ink, inkCartridge, inkRibbon
        Some(9) => SupplyKind::Drum,             // photoConductor
        _ => SupplyKind::Other,
    }
}

fn map_color(name: &str) -> Option<Color> {
    let l = name.trim().to_lowercase();
    match l.as_str() {
        "black" | "k" => Some(Color::Black),
        "cyan" | "c" => Some(Color::Cyan),
        "magenta" | "m" => Some(Color::Magenta),
        "yellow" | "y" => Some(Color::Yellow),
        "" | "none" | "unknown" => None,
        s if s.contains("tri") || s.contains("color") => Some(Color::TriColor),
        s if s.contains("photo") => Some(Color::Photo),
        _ => Some(Color::Other),
    }
}

fn map_level(level: i64, maxcap: Option<i64>) -> Level {
    match level {
        -1 => Level::NoRestriction,
        -3 => Level::SomeRemaining,
        v if v < 0 => Level::Unknown, // -2 and any other negative
        v => {
            let pct = match maxcap {
                Some(m) if m > 0 => v.saturating_mul(100) / m,
                _ => v,
            };
            Level::Pct(pct.clamp(0, 100) as u8)
        }
    }
}

/// Map an active critical/warning alert *description* to a Reason, conservatively.
/// Empty descriptions are skipped (no fabricated alarms).
fn map_alert(desc: &str) -> Option<Reason> {
    let l = desc.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
    let r = if l.contains("jam") {
        Reason::Jam
    } else if l.contains("cover") || l.contains("door") {
        Reason::CoverOpen
    } else if l.contains("empty")
        && (l.contains("paper") || l.contains("media") || l.contains("tray"))
    {
        Reason::MediaEmpty
    } else if l.contains("empty") {
        Reason::SupplyEmpty
    } else if l.contains("low")
        && (l.contains("toner") || l.contains("ink") || l.contains("supply"))
    {
        Reason::SupplyLow
    } else if l.contains("offline") {
        Reason::Offline
    } else {
        Reason::Other(desc.trim().to_string())
    };
    Some(r)
}

/// Pure reconstruction of a partial printer view from a flat OID→value map.
pub fn parse_snmp(m: &OidMap) -> PrinterState {
    // Supplies: one row per prtMarkerSuppliesLevel entry, joined with sibling columns
    // and the colorant table.
    let mut supplies = Vec::new();
    for (rk, lvl) in column(m, SUP_LEVEL).into_iter().take(ROW_CAP) {
        let level_i = as_int(lvl).unwrap_or(-2);
        let maxcap = get(m, &format!("{SUP_MAXCAP}.{rk}")).and_then(as_int);
        let type_i = get(m, &format!("{SUP_TYPE}.{rk}")).and_then(as_int);
        let class_i = get(m, &format!("{SUP_CLASS}.{rk}")).and_then(as_int);
        let desc = get(m, &format!("{SUP_DESC}.{rk}"))
            .and_then(as_str)
            .unwrap_or("");
        let unit_i = get(m, &format!("{SUP_UNIT}.{rk}")).and_then(as_int);
        let kind = map_type(type_i);

        // Colorant join: prtMarkerSuppliesColorantIndex → prtMarkerColorantValue[dev.cidx].
        let dev = rk.split('.').next().unwrap_or("1");
        let color_name = get(m, &format!("{SUP_COLORANT_IDX}.{rk}"))
            .and_then(as_int)
            .and_then(|cidx| get(m, &format!("{COLORANT_VALUE}.{dev}.{cidx}")))
            .and_then(as_str);

        supplies.push(Supply {
            name: if desc.is_empty() {
                format!("Supply {rk}")
            } else {
                desc.to_string()
            },
            kind,
            class: map_class(class_i, kind),
            color_raw: color_name.map(|s| s.to_string()),
            color: color_name.and_then(map_color),
            level: map_level(level_i, maxcap),
            max_capacity: maxcap.map(|v| v as i32),
            unit: unit_i.map(|u| {
                if u == 19 {
                    SupplyUnit::Percent
                } else {
                    SupplyUnit::Other
                }
            }),
        });
    }

    // Pages: max prtMarkerLifeCount across marker rows.
    let pages = column(m, LIFE_COUNT)
        .into_iter()
        .filter_map(|(_, v)| as_int(v))
        .filter(|v| *v >= 0)
        .max()
        .map(|v| v as u64);

    // Trays.
    let mut paper = Vec::new();
    for (rk, lvl) in column(m, IN_LEVEL).into_iter().take(ROW_CAP) {
        let level_i = as_int(lvl).unwrap_or(-2);
        let maxcap = get(m, &format!("{IN_MAXCAP}.{rk}")).and_then(as_int);
        let name = get(m, &format!("{IN_NAME}.{rk}"))
            .and_then(as_str)
            .unwrap_or("");
        paper.push(InputTray {
            name: if name.is_empty() {
                format!("Tray {rk}")
            } else {
                name.to_string()
            },
            level: map_level(level_i, maxcap),
            max_capacity: maxcap.map(|v| v as i32),
            empty: level_i == 0,
        });
    }

    // Alerts → reasons (active critical=3 / warning=4 only).
    let mut reasons = Vec::new();
    for (rk, sev) in column(m, ALERT_SEVERITY).into_iter().take(ROW_CAP) {
        if !matches!(as_int(sev), Some(3) | Some(4)) {
            continue;
        }
        let desc = get(m, &format!("{ALERT_DESC}.{rk}"))
            .and_then(as_str)
            .unwrap_or("");
        if let Some(reason) = map_alert(desc) {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
    }

    let status = column(m, PRINTER_STATUS)
        .into_iter()
        .next()
        .and_then(|(_, v)| {
            match as_int(v) {
                Some(3) | Some(5) => Some(Status::Idle), // idle / warmup
                Some(4) => Some(Status::Printing),
                _ => None,
            }
        });

    // Front-panel display text: join non-empty console buffer lines (in row order).
    let lines: Vec<String> = column(m, CONSOLE_TEXT)
        .into_iter()
        .filter_map(|(_, v)| as_str(v))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let display = if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    };

    PrinterState {
        name: None,
        model: None,
        status,
        reasons,
        supplies,
        paper,
        pages,
        jobs: None,
        display,
    }
}

// ---- thin snmp2 v2c walk adapter (network I/O; not unit-tested) ----

pub struct SnmpSource;

impl Source for SnmpSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Snmp
    }

    fn collect(&self, target: &Target) -> SourceOutcome {
        let start = Instant::now();
        if !target.snmp_enabled {
            return SourceOutcome::failed(SourceKind::Snmp, "snmp disabled", start.elapsed());
        }
        let host = match target.host.as_deref() {
            Some(h) => h,
            None => return SourceOutcome::failed(SourceKind::Snmp, "no host", start.elapsed()),
        };
        match walk_all(host, &target.community, target.timeout) {
            Ok(map) => SourceOutcome {
                kind: SourceKind::Snmp,
                partial: parse_snmp(&map),
                duration: start.elapsed(),
                error: None,
            },
            Err(e) => SourceOutcome::failed(SourceKind::Snmp, e, start.elapsed()),
        }
    }
}

fn walk_all(host: &str, community: &str, timeout: std::time::Duration) -> Result<OidMap, String> {
    use snmp2::{Oid, SyncSession, Value};

    // Bracket a bare IPv6 literal for the socket address.
    let needs_brackets = host.matches(':').count() >= 2 && !host.starts_with('[');
    let addr = if needs_brackets {
        format!("[{host}]:161")
    } else {
        format!("{host}:161")
    };
    let mut sess = SyncSession::new_v2c(addr.as_str(), community.as_bytes(), Some(timeout), 0)
        .map_err(|e| format!("snmp session: {e}"))?;

    fn snmp_val(v: &Value) -> Option<SnmpVal> {
        match v {
            Value::Integer(i) => Some(SnmpVal::Int(*i)),
            Value::Counter32(u) | Value::Unsigned32(u) | Value::Timeticks(u) => {
                Some(SnmpVal::Int(*u as i64))
            }
            Value::Counter64(u) => Some(SnmpVal::Int(*u as i64)),
            Value::OctetString(b) => Some(SnmpVal::Str(String::from_utf8_lossy(b).into_owned())),
            _ => None,
        }
    }
    // SNMP "end" exceptions: stop walking this subtree.
    fn is_end_exception(v: &Value) -> bool {
        matches!(
            v,
            Value::EndOfMibView | Value::NoSuchObject | Value::NoSuchInstance
        )
    }

    let mut map = OidMap::new();
    let mut budget = MAX_TOTAL_CHARS;
    let mut transport_ok = false;
    'subtrees: for (prefix, base) in WALK_BASES {
        let want = format!("{prefix}.");
        let mut cur: Vec<u64> = base.to_vec();
        for _ in 0..WALK_VARBIND_CAP {
            let cur_oid = match Oid::from(&cur) {
                Ok(o) => o,
                Err(_) => break,
            };
            let pdu = match sess.getnext(&cur_oid) {
                Ok(p) => p,
                Err(e) => {
                    // First transport failure with nothing collected ⇒ host unreachable: give up.
                    if !transport_ok {
                        return Err(format!("snmp unreachable: {e}"));
                    }
                    break; // this subtree failed; try the others
                }
            };
            transport_ok = true;
            // Extract owned data before the borrowed pdu is dropped.
            let extracted: Option<(String, Option<SnmpVal>, bool)> = pdu
                .varbinds
                .clone()
                .next()
                .map(|(oid, val)| (oid.to_id_string(), snmp_val(&val), is_end_exception(&val)));
            let (oid_s, sval, is_exc) = match extracted {
                Some(x) => x,
                None => break,
            };
            if is_exc || !oid_s.starts_with(&want) {
                break; // end of MIB / left the subtree
            }
            let next: Vec<u64> = oid_s
                .split('.')
                .filter_map(|p| p.parse::<u64>().ok())
                .collect();
            if next.is_empty() || next <= cur {
                break; // no numeric progress ⇒ avoid looping
            }
            if let Some(v) = sval {
                if !retain(&mut map, &mut budget, oid_s, v) {
                    break 'subtrees; // the poll's budget is spent
                }
            }
            cur = next;
        }
    }

    // Console display text via direct GET (device 1, lines 1..=6). Walks miss it on agents
    // that skip the console table on getnext; a direct GET works on any standard agent.
    if transport_ok {
        for line in 1..=6u64 {
            let comps = [1u64, 3, 6, 1, 2, 1, 43, 16, 5, 1, 2, 1, line];
            let oid = match Oid::from(&comps) {
                Ok(o) => o,
                Err(_) => break,
            };
            let pdu = match sess.get(&oid) {
                Ok(p) => p,
                Err(_) => break, // transport error ⇒ stop probing
            };
            if let Some((roid, val)) = pdu.varbinds.clone().next() {
                let oid_s = roid.to_id_string();
                if let Some(SnmpVal::Str(s)) = snmp_val(&val) {
                    if !s.trim().is_empty() && oid_s.starts_with(&format!("{CONSOLE_TEXT}.")) {
                        map.insert(oid_s, SnmpVal::Str(s));
                    }
                }
            }
        }
    }

    if map.is_empty() {
        return Err("snmp: no data".into());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, SnmpVal)]) -> OidMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }
    fn i(x: i64) -> SnmpVal {
        SnmpVal::Int(x)
    }
    fn s(x: &str) -> SnmpVal {
        SnmpVal::Str(x.into())
    }

    #[test]
    fn supplies_with_colorant_join_and_class() {
        // device 1, two supplies + colorant table + a waste receptacle.
        let map = m(&[
            // supply 1 = Black toner 54%
            ("1.3.6.1.2.1.43.11.1.1.5.1.1", i(3)), // type toner
            ("1.3.6.1.2.1.43.11.1.1.4.1.1", i(3)), // class consumed
            ("1.3.6.1.2.1.43.11.1.1.6.1.1", s("Black Cartridge")),
            ("1.3.6.1.2.1.43.11.1.1.8.1.1", i(100)), // maxcap
            ("1.3.6.1.2.1.43.11.1.1.9.1.1", i(54)),  // level
            ("1.3.6.1.2.1.43.11.1.1.3.1.1", i(1)),   // colorant index 1
            // supply 2 = waste, receptacleThatIsFilled, level 20
            ("1.3.6.1.2.1.43.11.1.1.5.1.2", i(4)), // type wasteToner
            ("1.3.6.1.2.1.43.11.1.1.4.1.2", i(4)), // class filled
            ("1.3.6.1.2.1.43.11.1.1.6.1.2", s("Waste Toner")),
            ("1.3.6.1.2.1.43.11.1.1.8.1.2", i(100)),
            ("1.3.6.1.2.1.43.11.1.1.9.1.2", i(20)),
            // colorant table dev 1: index1 = black
            ("1.3.6.1.2.1.43.12.1.1.4.1.1", s("black")),
            // pages
            ("1.3.6.1.2.1.43.10.2.1.4.1.1", i(54231)),
            // status idle
            ("1.3.6.1.2.1.25.3.5.1.1.1", i(3)),
        ]);
        let st = parse_snmp(&map);
        assert_eq!(st.status, Some(Status::Idle));
        assert_eq!(st.pages, Some(54231));
        assert_eq!(st.supplies.len(), 2);
        let black = &st.supplies[0];
        assert_eq!(black.kind, SupplyKind::Toner);
        assert_eq!(black.class, SupplyClass::Consumed);
        assert_eq!(black.color, Some(Color::Black));
        assert_eq!(black.level, Level::Pct(54));
        assert!(black.is_usable());
        let waste = &st.supplies[1];
        assert_eq!(waste.kind, SupplyKind::Waste);
        assert_eq!(waste.class, SupplyClass::Filled);
        assert!(!waste.is_usable());
    }

    #[test]
    fn level_normalizes_by_maxcapacity_and_sentinels() {
        // maxcap 200, level 100 → 50%
        let map = m(&[
            ("1.3.6.1.2.1.43.11.1.1.9.1.1", i(100)),
            ("1.3.6.1.2.1.43.11.1.1.8.1.1", i(200)),
            ("1.3.6.1.2.1.43.11.1.1.5.1.1", i(3)),
            // unknown sentinel
            ("1.3.6.1.2.1.43.11.1.1.9.1.2", i(-2)),
            ("1.3.6.1.2.1.43.11.1.1.5.1.2", i(3)),
        ]);
        let st = parse_snmp(&map);
        assert_eq!(st.supplies[0].level, Level::Pct(50));
        assert_eq!(st.supplies[1].level, Level::Unknown);
    }

    #[test]
    fn trays_and_active_alerts() {
        let map = m(&[
            ("1.3.6.1.2.1.43.8.2.1.10.1.1", i(80)),
            ("1.3.6.1.2.1.43.8.2.1.9.1.1", i(100)),
            ("1.3.6.1.2.1.43.8.2.1.13.1.1", s("Tray 2")),
            // active critical alert: paper jam
            ("1.3.6.1.2.1.43.18.1.1.2.1", i(3)), // severity critical
            ("1.3.6.1.2.1.43.18.1.1.8.1", s("Paper jam in tray 2")),
            // a non-active (other=1) alert that must be ignored
            ("1.3.6.1.2.1.43.18.1.1.2.2", i(1)),
            ("1.3.6.1.2.1.43.18.1.1.8.2", s("toner low")),
        ]);
        let st = parse_snmp(&map);
        assert_eq!(st.paper.len(), 1);
        assert_eq!(st.paper[0].name, "Tray 2");
        assert_eq!(st.paper[0].level, Level::Pct(80));
        assert_eq!(st.reasons, vec![Reason::Jam]); // only the critical one
    }

    // --- the poll budget ------------------------------------------------

    #[test]
    fn the_poll_limits_hold_their_documented_values() {
        // The other budget tests hand `retain` a budget of their own, so they
        // prove the mechanism and this pin proves the wiring: changing a
        // safety limit must cost an edit here, on purpose.
        assert_eq!(MAX_TOTAL_CHARS, 262_144);
        assert_eq!(MAX_VAL_CHARS, 4096);
        assert_eq!(MAX_OID_CHARS, 256);
    }

    #[test]
    fn a_normal_varbind_is_kept_whole() {
        let mut m = OidMap::new();
        let mut budget = 256 * 1024;
        assert!(retain(&mut m, &mut budget, "1.3.6.1.2.1.43.11.1.1.9.1.1".into(), SnmpVal::Str("Black Cartridge".into())));
        assert_eq!(m.len(), 1);
        assert!(matches!(m.values().next().unwrap(), SnmpVal::Str(t) if t == "Black Cartridge"));
    }

    #[test]
    fn a_value_longer_than_4096_characters_is_cut_at_4096() {
        // The number is written out on purpose: a test that reads the constant
        // moves with it and can never fail.
        let mut m = OidMap::new();
        let mut budget = 256 * 1024;
        retain(&mut m, &mut budget, "1.3.6.1.4.1".into(), SnmpVal::Str("x".repeat(100_000)));
        let SnmpVal::Str(t) = m.values().next().unwrap() else { panic!() };
        assert_eq!(t.chars().count(), 4096);
    }

    #[test]
    fn an_oid_longer_than_256_characters_is_not_kept() {
        let mut m = OidMap::new();
        let mut budget = 256 * 1024;
        let long_oid = "1.".repeat(200) + "1";
        assert!(retain(&mut m, &mut budget, long_oid, SnmpVal::Int(1)));
        assert!(m.is_empty());
        assert_eq!(budget, 256 * 1024, "a dropped row must not spend budget");
    }

    #[test]
    fn a_poll_cannot_retain_more_than_262144_characters() {
        // A hostile printer answers every getnext with a 4 KiB string. Without
        // the total budget this map would grow to WALK_VARBIND_CAP × subtrees
        // × 4 KiB — the exact shape the review named.
        let mut m = OidMap::new();
        let mut budget = 256 * 1024;
        let mut stopped = false;
        for i in 0..100_000 {
            if !retain(&mut m, &mut budget, format!("1.3.6.1.9.{i}"), SnmpVal::Str("y".repeat(4096))) {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "the walk was never told to stop");
        let total: usize = m
            .iter()
            .map(|(k, v)| k.chars().count() + match v { SnmpVal::Str(t) => t.chars().count(), _ => 0 })
            .sum();
        assert!(total <= 262_144, "retained {total} characters");
        assert!(m.len() < 100, "kept {} rows — the budget did not bite", m.len());
    }
}
