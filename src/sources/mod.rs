//! Source abstraction: each source returns a `SourceOutcome` carrying its identity,
//! a partial printer view, timing, and any error. The collector runs them on threads.

use crate::model::PrinterState;
use std::time::Duration;

pub mod ipp;
pub mod snmp;

/// A source contributes whatever it knows; the rest stays default/empty.
pub type PartialPrinter = PrinterState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Ipp,
    Cups,
    Snmp,
}

#[derive(Debug, Clone)]
pub struct SourceOutcome {
    pub kind: SourceKind,
    pub partial: PartialPrinter,
    pub duration: Duration,
    pub error: Option<String>,
}

impl SourceOutcome {
    pub fn failed(kind: SourceKind, error: impl Into<String>, duration: Duration) -> Self {
        Self { kind, partial: PartialPrinter::default(), duration, error: Some(error.into()) }
    }
}

/// How to reach a printer. A target may carry a network `host` and/or a local `cups` queue.
#[derive(Debug, Clone)]
pub struct Target {
    pub host: Option<String>,
    pub ipp_path: String,
    pub cups: Option<String>,
    pub snmp_enabled: bool,
    pub community: String,
    pub timeout: Duration,
}

pub trait Source: Send {
    fn kind(&self) -> SourceKind;
    fn collect(&self, target: &Target) -> SourceOutcome;
}
