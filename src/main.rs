mod actions;
mod color;
mod config;
mod json;
mod merge;
mod model;
mod notify;
mod palette;
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


/// The reference `--help` prints. Same shape as the shell widgets in the
/// family: a usage line, then one paragraph per flag, then the subcommands.
/// Plain text on stdout, exit 0 — the exit-0 JSON contract is for the widget
/// path, and a human asking for help is not that path.
const HELP: &str = "\
Usage: printbar [--help] <printer-name> [--json] [--no-color[=all|bar|tooltip]]
       printbar action <ews|queue> --printer <name>

--help     Print this reference and exit 0. Also accepted as -h.
--json     Structured output mode: one raw-data JSON object (numbers, state
           strings, and the resolved `palette`) instead of Waybar JSON, for
           frontends that render their own UI — e.g. the Omarchy shell plugin
           in omarchy/. Always exits 0; errors are reported in an \"error\"
           field. Deliberately unaffected by --no-color: the document carries
           data, never rendered presentation.
--no-color[=all|bar|tooltip]
           Monochrome output: no color markup on the chosen surface (default
           \"all\"). Everything structural stays — glyphs, level bars, layout.
           The \"class\" field and the --json payload are unaffected, so CSS
           remains the way to style a monochrome bar. NO_COLOR=<non-empty> in
           the environment means --no-color=all; an explicit flag wins over it.

action     Run a printer action instead of reporting: `ews` opens the embedded
           web server, `queue` opens the print queue. Both need --printer.

The printer must have a [printer.<name>] section in the config file
(~/.config/printbar/config.toml, or $PRINTBAR_CONFIG).
";

fn main() {
    // The exit-0 JSON contract: any error still prints valid JSON (Waybar-shaped by
    // default, structured when --json was asked for), exit 0.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().skip(1).any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    let json_mode = args.iter().skip(1).any(|a| a == "--json");
    if let Err(e) = run(&args, json_mode) {
        if json_mode {
            println!(
                "{}",
                json::error_output(json_error_printer(&args).unwrap_or(""), &e)
            );
        } else {
            println!("{}", waybar::error_output(&e));
        }
    }
}

/// First positional argument (the printer name), skipping any flags.
fn printer_arg(args: &[String]) -> Option<&str> {
    args.iter()
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .map(String::as_str)
}

/// Printer name to report in a structured error: the `--printer` flag on the
/// action path, the positional name otherwise.
fn json_error_printer(args: &[String]) -> Option<&str> {
    if args.get(1).map(String::as_str) == Some("action") {
        return args
            .iter()
            .position(|a| a == "--printer")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str);
    }
    printer_arg(args)
}

fn run(args: &[String], json_mode: bool) -> Result<(), String> {
    if args.get(1).map(String::as_str) == Some("action") {
        // --json is the monitor's structured output; an action produces no
        // monitor document, so the combination is rejected up front (still
        // exit 0 with a structured error, and NO action is performed).
        if json_mode {
            return Err("--json applies to monitor output, not actions".into());
        }
        return run_action(args);
    }

    // Resolved before any I/O, so a bad --no-color value fails fast through the
    // usual exit-0 error path instead of after a network poll.
    let colors = color::ColorMode::resolve(args, no_color_env().as_deref())?;

    let name = printer_arg(args)
        .ok_or("usage: printbar <printer-name> [--json] [--no-color[=all|bar|tooltip]]\nRun printbar --help for the full reference.")?;
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
    // Desktop notifications belong to the waybar/printbar-watch path; the
    // Omarchy plugin (--json consumer) has its own UI, and firing here too
    // would duplicate transition notifications when both frontends poll.
    if !json_mode {
        notify::maybe_notify(name, pc, &state);
    }
    // Both modes resolve the same theme: the tooltip paints with it, and the
    // structured document publishes it as `palette` so the plugin renders
    // printbar's ramp instead of keeping a second copy of it.
    let theme = theme::ThemeColors::load();
    if json_mode {
        // Deliberately unaffected by --no-color: the structured document carries
        // raw data, `state` and the palette, never rendered presentation, and
        // the plugin decides its own rendering from it.
        json::render(name, &state, pc, &theme).print();
    } else {
        render::render(&state, pc, &theme, colors).print();
    }
    Ok(())
}

/// `NO_COLOR` per <https://no-color.org>: set to anything non-empty means
/// monochrome. Read once here so the resolver itself stays pure and testable.
fn no_color_env() -> Option<String> {
    std::env::var("NO_COLOR").ok()
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

#[cfg(test)]
mod tests {

    // --help is part of the family contract: every widget answers it, prints
    // a usage line, documents every flag it accepts, and exits 0.
    #[test]
    fn help_documents_every_flag() {
        assert!(HELP.starts_with("Usage: printbar"));
        for flag in ["--help", "--json", "--no-color"] {
            assert!(HELP.contains(flag), "HELP does not document {flag}");
        }
        assert!(HELP.contains("action"), "HELP does not document the action subcommand");
    }
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn action_with_json_is_rejected_without_running_the_action() {
        // run() must bail out before run_action (no config load, no xdg-open),
        // so main prints the structured error schema and exits 0.
        let a = args(&["printbar", "action", "ews", "--printer", "office", "--json"]);
        let err = run(&a, true).unwrap_err();
        assert_eq!(err, "--json applies to monitor output, not actions");
        // The structured error carries the action's --printer name.
        let s = json::error_output(json_error_printer(&a).unwrap_or(""), &err);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["printer"], "office");
        assert_eq!(v["state"], "error");
        assert_eq!(
            v["error"]["message"],
            "--json applies to monitor output, not actions"
        );
    }

    #[test]
    fn unknown_no_color_value_fails_before_any_io() {
        // No config is read and no printer is polled: the argument error comes
        // straight back, and main turns it into the exit-0 error document.
        let a = args(&["printbar", "office", "--no-color=bogus"]);
        let err = run(&a, false).unwrap_err();
        assert_eq!(
            err,
            "--no-color: unknown value 'bogus' (expected all, bar or tooltip)"
        );
        let v: serde_json::Value = serde_json::from_str(&waybar::error_output(&err)).unwrap();
        assert_eq!(v["class"], serde_json::json!(["error"]));
        assert_eq!(v["tooltip"], err);
        // Same error, structured shape, in --json mode.
        let a = args(&["printbar", "office", "--json", "--no-color=bogus"]);
        let err = run(&a, true).unwrap_err();
        let s = json::error_output(json_error_printer(&a).unwrap_or(""), &err);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["printer"], "office");
        assert_eq!(v["state"], "error");
        assert_eq!(v["error"]["message"], err);
    }

    #[test]
    fn flags_never_masquerade_as_the_printer_name() {
        assert_eq!(printer_arg(&args(&["printbar", "office"])), Some("office"));
        assert_eq!(
            printer_arg(&args(&["printbar", "--no-color=bar", "office", "--json"])),
            Some("office")
        );
        assert_eq!(printer_arg(&args(&["printbar", "--no-color"])), None);
    }

    #[test]
    fn json_error_printer_prefers_action_flag() {
        assert_eq!(
            json_error_printer(&args(&[
                "printbar",
                "action",
                "queue",
                "--printer",
                "x",
                "--json"
            ])),
            Some("x")
        );
        assert_eq!(
            json_error_printer(&args(&["printbar", "office", "--json"])),
            Some("office")
        );
    }
}
