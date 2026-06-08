//! Pure merge of per-source partials into one `PrinterState` (design spec §7).
//! No I/O — fully unit-testable.

use crate::model::{PrinterState, Reason, Status, Supply};
use crate::sources::{SourceKind, SourceOutcome};

/// Priority order for "richest data first" fields (supplies, paper, pages, model).
const RICH_PRIORITY: [SourceKind; 3] = [SourceKind::Snmp, SourceKind::Ipp, SourceKind::Cups];
/// Priority for the most semantic state.
const STATUS_PRIORITY: [SourceKind; 3] = [SourceKind::Ipp, SourceKind::Cups, SourceKind::Snmp];
/// Jobs: the local CUPS spool is the most relevant backlog.
const JOBS_PRIORITY: [SourceKind; 3] = [SourceKind::Cups, SourceKind::Ipp, SourceKind::Snmp];

pub fn merge(outcomes: &[SourceOutcome]) -> PrinterState {
    // All sources errored/unreachable → offline.
    if !outcomes.is_empty() && outcomes.iter().all(|o| o.error.is_some()) {
        return PrinterState { status: Some(Status::Offline), ..Default::default() };
    }

    let find = |kind: SourceKind| outcomes.iter().find(|o| o.kind == kind).map(|o| &o.partial);

    // Supplies: wholesale from the highest-priority source with a USABLE set; else the
    // highest-priority non-empty set (so a lone waste row still shows when nothing better exists).
    let supplies = pick_supplies(outcomes);

    let mut state = PrinterState {
        supplies,
        ..Default::default()
    };

    for k in RICH_PRIORITY {
        if let Some(p) = find(k) {
            if state.paper.is_empty() && !p.paper.is_empty() {
                state.paper = p.paper.clone();
            }
            if state.pages.is_none() && p.pages.is_some() {
                state.pages = p.pages;
            }
            if state.model.is_none() && p.model.is_some() {
                state.model = p.model.clone();
            }
            if state.name.is_none() && p.name.is_some() {
                state.name = p.name.clone();
            }
        }
    }
    for k in STATUS_PRIORITY {
        if state.status.is_none() {
            if let Some(p) = find(k) {
                if p.status.is_some() {
                    state.status = p.status.clone();
                }
            }
        }
    }
    for k in JOBS_PRIORITY {
        if state.jobs.is_none() {
            if let Some(p) = find(k) {
                if p.jobs.is_some() {
                    state.jobs = p.jobs;
                }
            }
        }
    }

    // Reasons: IPP/CUPS primary (union), then SNMP additive (already pre-filtered by the
    // SNMP source to active + critical/warning). Dedupe, preserve first-seen order.
    let mut reasons: Vec<Reason> = Vec::new();
    let push = |src: &[Reason], out: &mut Vec<Reason>| {
        for r in src {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
    };
    if let Some(p) = find(SourceKind::Ipp) {
        push(&p.reasons, &mut reasons);
    }
    if let Some(p) = find(SourceKind::Cups) {
        push(&p.reasons, &mut reasons);
    }
    if let Some(p) = find(SourceKind::Snmp) {
        push(&p.reasons, &mut reasons);
    }
    state.reasons = reasons;

    state
}

fn pick_supplies(outcomes: &[SourceOutcome]) -> Vec<Supply> {
    let by = |kind: SourceKind| outcomes.iter().find(|o| o.kind == kind).map(|o| &o.partial.supplies);
    // First: a source with at least one usable consumable.
    for k in RICH_PRIORITY {
        if let Some(s) = by(k) {
            if s.iter().any(|x| x.is_usable()) {
                return s.clone();
            }
        }
    }
    // Fallback: highest-priority non-empty set.
    for k in RICH_PRIORITY {
        if let Some(s) = by(k) {
            if !s.is_empty() {
                return s.clone();
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Color, Level, SupplyClass, SupplyKind};
    use std::time::Duration;

    fn ok(kind: SourceKind, partial: PrinterState) -> SourceOutcome {
        SourceOutcome { kind, partial, duration: Duration::ZERO, error: None }
    }
    fn supply(kind: SupplyKind, class: SupplyClass, name: &str, level: Level) -> Supply {
        Supply { name: name.into(), kind, class, color_raw: None, color: Some(Color::Black),
            level, max_capacity: None, unit: None }
    }

    #[test]
    fn supplies_taken_wholesale_from_highest_usable() {
        let snmp = PrinterState {
            supplies: vec![supply(SupplyKind::Waste, SupplyClass::Filled, "Waste", Level::Pct(20))],
            ..Default::default()
        };
        let cmyk = vec![
            supply(SupplyKind::Toner, SupplyClass::Consumed, "Black", Level::Pct(54)),
            supply(SupplyKind::Toner, SupplyClass::Consumed, "Cyan", Level::Pct(69)),
        ];
        let ipp = PrinterState { supplies: cmyk.clone(), ..Default::default() };
        let got = merge(&[ok(SourceKind::Snmp, snmp), ok(SourceKind::Ipp, ipp)]);
        assert_eq!(got.supplies, cmyk); // IPP's set, not SNMP's lone waste
    }

    #[test]
    fn jobs_prefer_cups_over_network() {
        let cups = PrinterState { jobs: Some(3), ..Default::default() };
        let ipp = PrinterState { jobs: Some(0), ..Default::default() };
        let got = merge(&[ok(SourceKind::Ipp, ipp), ok(SourceKind::Cups, cups)]);
        assert_eq!(got.jobs, Some(3));
    }

    #[test]
    fn status_prefers_ipp() {
        let ipp = PrinterState { status: Some(Status::Printing), ..Default::default() };
        let snmp = PrinterState { status: Some(Status::Idle), ..Default::default() };
        let got = merge(&[ok(SourceKind::Snmp, snmp), ok(SourceKind::Ipp, ipp)]);
        assert_eq!(got.status, Some(Status::Printing));
    }

    #[test]
    fn reasons_ipp_primary_plus_snmp_additive_deduped() {
        let ipp = PrinterState { reasons: vec![Reason::Jam], ..Default::default() };
        let snmp = PrinterState {
            reasons: vec![Reason::CoverOpen, Reason::Jam], // Jam dup, CoverOpen new
            ..Default::default()
        };
        let got = merge(&[ok(SourceKind::Ipp, ipp), ok(SourceKind::Snmp, snmp)]);
        assert_eq!(got.reasons, vec![Reason::Jam, Reason::CoverOpen]);
    }

    #[test]
    fn all_sources_failed_is_offline() {
        let outs = vec![
            SourceOutcome::failed(SourceKind::Ipp, "no route", Duration::ZERO),
            SourceOutcome::failed(SourceKind::Snmp, "timeout", Duration::ZERO),
        ];
        let got = merge(&outs);
        assert_eq!(got.status, Some(Status::Offline));
        assert!(got.supplies.is_empty());
    }
}
