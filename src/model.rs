//! Unified printer data model. Every field is best-effort: absent when no source
//! provides it. Supplies are generic (toner/ink/drum/waste), per the design spec §5.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Printing,
    Stopped,
    Offline,
}

/// Normalized printer conditions, sourced primarily from IPP `printer-state-reasons`
/// and enriched (additively) from SNMP `prtAlertTable`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reason {
    Jam,
    MediaEmpty,
    MediaLow,
    SupplyLow,
    SupplyEmpty,
    CoverOpen,
    Offline,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyKind {
    Toner,
    Ink,
    Drum,
    Waste,
    Other,
}

/// Whether the level reads as "amount remaining" (low = bad) or "amount filled"
/// (high = bad, e.g. a waste receptacle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyClass {
    Consumed,
    Filled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Color {
    Cyan,
    Magenta,
    Yellow,
    Black,
    TriColor,
    Photo,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyUnit {
    Percent,
    Other,
}

/// A supply level, preserving RFC 3805 / CUPS sentinels as distinct states
/// rather than collapsing them all into "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Pct(u8),
    NoRestriction, // SNMP/CUPS -1
    Unknown,       // -2
    SomeRemaining, // -3
}

impl Level {
    /// Numeric percent when known, else `None`.
    pub fn as_pct(self) -> Option<u8> {
        match self {
            Level::Pct(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supply {
    pub name: String,
    pub kind: SupplyKind,
    pub class: SupplyClass,
    pub color_raw: Option<String>,
    pub color: Option<Color>,
    pub level: Level,
    pub max_capacity: Option<i32>,
    pub unit: Option<SupplyUnit>,
}

impl Supply {
    /// A supply contributes to a "usable" supply set only if it's a real
    /// consumable with a known kind/name and a concrete (non-sentinel) level.
    /// A lone waste tank or a sentinel-only row must not suppress a fuller list.
    pub fn is_usable(&self) -> bool {
        self.class == SupplyClass::Consumed
            && self.kind != SupplyKind::Waste
            && !self.name.is_empty()
            && self.level.as_pct().is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputTray {
    pub name: String,
    pub level: Level,
    pub max_capacity: Option<i32>,
    pub empty: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrinterState {
    pub name: Option<String>,
    pub model: Option<String>,
    pub status: Option<Status>,
    pub reasons: Vec<Reason>,
    pub supplies: Vec<Supply>,
    pub paper: Vec<InputTray>,
    pub pages: Option<u64>,
    pub jobs: Option<u32>,
    /// The literal text on the printer's front-panel display (SNMP console buffer).
    pub display: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(kind: SupplyKind, class: SupplyClass, name: &str, level: Level) -> Supply {
        Supply {
            name: name.into(),
            kind,
            class,
            color_raw: None,
            color: None,
            level,
            max_capacity: None,
            unit: None,
        }
    }

    #[test]
    fn level_pct_and_sentinels() {
        assert_eq!(Level::Pct(80).as_pct(), Some(80));
        assert_eq!(Level::Unknown.as_pct(), None);
        assert_eq!(Level::NoRestriction.as_pct(), None);
        assert_eq!(Level::SomeRemaining.as_pct(), None);
    }

    #[test]
    fn usable_requires_real_consumable() {
        let waste = supply(
            SupplyKind::Waste,
            SupplyClass::Filled,
            "Waste",
            Level::Pct(10),
        );
        let toner = supply(
            SupplyKind::Toner,
            SupplyClass::Consumed,
            "Black",
            Level::Pct(54),
        );
        let sentinel = supply(
            SupplyKind::Toner,
            SupplyClass::Consumed,
            "Cyan",
            Level::Unknown,
        );
        let unnamed = supply(SupplyKind::Ink, SupplyClass::Consumed, "", Level::Pct(40));
        assert!(!waste.is_usable());
        assert!(toner.is_usable());
        assert!(!sentinel.is_usable());
        assert!(!unnamed.is_usable());
    }
}
