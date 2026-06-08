#![allow(dead_code)]
mod config;
mod merge;
mod model;
mod render;
mod sources;
mod theme;
mod waybar;

use std::path::PathBuf;
use std::time::Duration;

use config::{Config, PrinterConfig};
use sources::ipp::IppSource;
use sources::{run_sources, Source, SourceKind, Target};

fn main() {
    // The exit-0 JSON contract: any error still prints valid Waybar JSON, exit 0.
    if let Err(e) = run() {
        println!("{}", waybar::error_output(&e));
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("action") {
        return run_action(&args);
    }

    let name = args.get(1).ok_or("usage: printbar <printer-name>")?;
    let cfg = Config::load(&config_path())?;
    let pc = cfg.for_printer(name).ok_or_else(|| format!("no [printer.{name}] in config"))?;

    let target = build_target(pc);
    let srcs = build_sources(pc);
    if srcs.is_empty() {
        return Err(format!("printer '{name}' has neither host nor cups configured"));
    }
    let outcomes = run_sources(&target, srcs);
    let state = merge::merge(&outcomes);
    let theme = theme::ThemeColors::load();
    render::render(&state, pc, &theme).print();
    Ok(())
}

fn run_action(args: &[String]) -> Result<(), String> {
    // Wired in Task 8 (actions module). For now, a clear error keeps the contract.
    let _ = args;
    Err("actions not implemented yet".into())
}

fn build_target(pc: &PrinterConfig) -> Target {
    Target {
        host: pc.host.clone(),
        ipp_path: pc.ipp_path.clone(),
        cups: pc.cups.clone(),
        snmp_enabled: pc.snmp.enabled,
        community: pc.snmp.community.clone(),
        timeout: Duration::from_secs(pc.timeout),
    }
}

fn build_sources(pc: &PrinterConfig) -> Vec<Box<dyn Source>> {
    let mut v: Vec<Box<dyn Source>> = Vec::new();
    if pc.host.is_some() {
        v.push(Box::new(IppSource { kind: SourceKind::Ipp }));
    }
    if pc.cups.is_some() {
        v.push(Box::new(IppSource { kind: SourceKind::Cups }));
    }
    // SNMP source added in Task 9.
    v
}

fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("PRINTBAR_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("printbar/config.toml")
}
