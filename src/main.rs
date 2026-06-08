mod actions;
mod config;
mod merge;
mod model;
mod notify;
mod render;
mod sources;
mod theme;
mod waybar;

use std::path::PathBuf;
use std::time::Duration;

use config::{Config, PrinterConfig};
use sources::ipp::IppSource;
use sources::snmp::SnmpSource;
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
    let pc = cfg
        .for_printer(name)
        .ok_or_else(|| format!("no [printer.{name}] in config"))?;

    let target = build_target(pc);
    let srcs = build_sources(pc);
    if srcs.is_empty() {
        return Err(format!(
            "printer '{name}' has neither host nor cups configured"
        ));
    }
    let outcomes = run_sources(&target, srcs);
    let state = merge::merge(&outcomes);
    notify::maybe_notify(name, pc, &state);
    let theme = theme::ThemeColors::load();
    render::render(&state, pc, &theme).print();
    Ok(())
}

fn run_action(args: &[String]) -> Result<(), String> {
    // printbar action <ews|queue> --printer <name>
    let kind = args.get(2).ok_or("action: missing <ews|queue>")?;
    let name = flag_value(args, "--printer").ok_or("action: missing --printer <name>")?;
    let cfg = Config::load(&config_path())?;
    let pc = cfg
        .for_printer(&name)
        .ok_or_else(|| format!("no [printer.{name}] in config"))?;
    actions::run(kind, pc)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn build_target(pc: &PrinterConfig) -> Target {
    Target {
        host: pc.host.clone(),
        ipp_path: pc.ipp_path.clone(),
        cups: pc.cups.clone(),
        snmp_enabled: pc.snmp.enabled,
        community: pc.snmp.community.clone(),
        // Clamp to a sane range so a huge config value can't overflow `Instant + Duration`.
        timeout: Duration::from_secs(pc.timeout.clamp(1, 60)),
    }
}

fn build_sources(pc: &PrinterConfig) -> Vec<Box<dyn Source>> {
    let mut v: Vec<Box<dyn Source>> = Vec::new();
    if pc.host.is_some() {
        v.push(Box::new(IppSource {
            kind: SourceKind::Ipp,
        }));
    }
    if pc.cups.is_some() {
        v.push(Box::new(IppSource {
            kind: SourceKind::Cups,
        }));
    }
    if pc.snmp.enabled && pc.host.is_some() {
        v.push(Box::new(SnmpSource));
    }
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
