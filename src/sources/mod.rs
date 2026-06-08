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

/// Run all sources on threads and collect outcomes, bounded by an overall deadline.
/// A source that doesn't report in time is recorded as a timeout outcome and we render
/// without joining it (a channel timeout cannot cancel the worker, hence each source also
/// sets its own protocol-level timeout).
pub fn run_sources(target: &Target, sources: Vec<Box<dyn Source>>) -> Vec<SourceOutcome> {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    let kinds: Vec<SourceKind> = sources.iter().map(|s| s.kind()).collect();
    let n = sources.len();
    let (tx, rx) = mpsc::channel::<SourceOutcome>();
    for src in sources {
        let tx = tx.clone();
        let t = target.clone();
        thread::spawn(move || {
            let _ = tx.send(src.collect(&t));
        });
    }
    drop(tx);

    let overall = Instant::now() + target.timeout + Duration::from_millis(500);
    let mut got: Vec<SourceOutcome> = Vec::with_capacity(n);
    while got.len() < n {
        match overall.checked_duration_since(Instant::now()) {
            Some(rem) => match rx.recv_timeout(rem) {
                Ok(o) => got.push(o),
                Err(_) => break, // overall timeout
            },
            None => break,
        }
    }
    // Record timeout outcomes for any source that didn't report (kinds are unique).
    for k in kinds {
        if !got.iter().any(|o| o.kind == k) {
            got.push(SourceOutcome::failed(k, "timeout", target.timeout));
        }
    }
    got
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Fake {
        kind: SourceKind,
        sleep: Duration,
        jobs: Option<u32>,
    }
    impl Source for Fake {
        fn kind(&self) -> SourceKind {
            self.kind
        }
        fn collect(&self, _t: &Target) -> SourceOutcome {
            std::thread::sleep(self.sleep);
            let mut partial = PartialPrinter::default();
            partial.jobs = self.jobs;
            SourceOutcome { kind: self.kind, partial, duration: self.sleep, error: None }
        }
    }

    fn target() -> Target {
        Target {
            host: None,
            ipp_path: "/ipp/print".into(),
            cups: None,
            snmp_enabled: false,
            community: "public".into(),
            timeout: Duration::from_millis(100),
        }
    }

    #[test]
    fn slow_source_times_out_without_hanging() {
        let fast: Box<dyn Source> = Box::new(Fake { kind: SourceKind::Ipp, sleep: Duration::ZERO, jobs: Some(2) });
        let slow: Box<dyn Source> = Box::new(Fake { kind: SourceKind::Snmp, sleep: Duration::from_secs(5), jobs: Some(9) });
        let start = Instant::now();
        let outs = run_sources(&target(), vec![fast, slow]);
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(2), "should not wait for the slow source: {elapsed:?}");
        let ipp = outs.iter().find(|o| o.kind == SourceKind::Ipp).unwrap();
        let snmp = outs.iter().find(|o| o.kind == SourceKind::Snmp).unwrap();
        assert!(ipp.error.is_none());
        assert_eq!(ipp.partial.jobs, Some(2));
        assert!(snmp.error.is_some()); // timed out
    }
}
