//! End-to-end checks of the exit-0 contract and of monochrome mode across the
//! two output surfaces.
//!
//! The fixture printer points at TEST-NET-1 (192.0.2.0/24, reserved for
//! documentation and guaranteed unroutable) with a 1s timeout, so every source
//! fails and the binary renders its offline document without touching anything
//! on this machine.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use assert_cmd::Command;

const CONFIG: &str = r#"
[printer.x]
host = "192.0.2.1"
timeout = 1

[printer.x.bar]
format = "<span foreground='#ff0000'>P</span> {status}"

[printer.x.tooltip]
items = ["model", "status", "alerts", "supplies", "paper"]
frame = true
"#;

/// Written exactly once per test binary: these tests run in parallel threads,
/// and re-writing the file under a peer's read makes it look like an empty
/// config for as long as the truncate lasts.
fn config_path() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-color-config.toml");
        std::fs::write(&path, CONFIG).unwrap();
        path
    })
}

/// An empty directory standing in for `$HOME` and every XDG base dir.
///
/// The binary resolves its theme from `$XDG_STATE_HOME`, `$XDG_CONFIG_HOME`,
/// `$XDG_CACHE_HOME` and `$HOME`. Left unpinned, these tests would read the
/// DEVELOPER'S live Omarchy theme: they would still pass — they only compare
/// runs to each other — but for the wrong reason, and the palette in the
/// output would change under them whenever someone switched theme mid-run.
/// Pinned to an empty dir, every run resolves the built-in One Dark palette,
/// on this machine and on CI alike.
fn empty_home() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("empty-home");
        std::fs::create_dir_all(&path).unwrap();
        path
    })
}

/// Run the binary and return its stdout, asserting the exit-0 contract.
fn run(args: &[&str], no_color_env: Option<&str>) -> String {
    let mut cmd = Command::cargo_bin("printbar").unwrap();
    cmd.env("PRINTBAR_CONFIG", config_path());
    for var in [
        "HOME",
        "XDG_STATE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
    ] {
        cmd.env(var, empty_home());
    }
    cmd.env_remove("NO_COLOR");
    if let Some(v) = no_color_env {
        cmd.env("NO_COLOR", v);
    }
    let Output { status, stdout, .. } = cmd.args(args).output().unwrap();
    assert!(status.success(), "printbar must always exit 0: {status}");
    String::from_utf8(stdout).unwrap()
}

fn field(doc: &str, key: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(doc).unwrap();
    v[key].as_str().unwrap_or_default().to_string()
}

fn has_color(s: &str) -> bool {
    s.contains("foreground") || s.contains('#')
}

#[test]
fn the_four_states_reach_the_right_surface() {
    let colored = run(&["x"], None);
    assert!(has_color(&field(&colored, "text")));
    assert!(has_color(&field(&colored, "tooltip")));

    let none = run(&["x", "--no-color"], None);
    assert!(!has_color(&field(&none, "text")));
    assert!(!has_color(&field(&none, "tooltip")));
    assert_eq!(run(&["x", "--no-color=all"], None), none);

    let plain_bar = run(&["x", "--no-color=bar"], None);
    assert!(!has_color(&field(&plain_bar, "text")));
    assert!(has_color(&field(&plain_bar, "tooltip")));

    let plain_tooltip = run(&["x", "--no-color=tooltip"], None);
    assert!(has_color(&field(&plain_tooltip, "text")));
    assert!(!has_color(&field(&plain_tooltip, "tooltip")));

    // The class is a machine contract and never changes with the flag.
    for doc in [&colored, &none, &plain_bar, &plain_tooltip] {
        let v: serde_json::Value = serde_json::from_str(doc).unwrap();
        assert_eq!(v["class"], serde_json::json!(["offline"]));
    }
}

#[test]
fn no_color_env_is_honored_and_the_flag_overrides_it() {
    assert_eq!(run(&["x"], Some("1")), run(&["x", "--no-color"], None));
    // Empty means unset (no-color.org).
    assert_eq!(run(&["x"], Some("")), run(&["x"], None));
    // The explicit flag wins, even when it re-colors a surface NO_COLOR muted.
    let tooltip_kept = run(&["x", "--no-color=bar"], Some("1"));
    assert!(has_color(&field(&tooltip_kept, "tooltip")));
    assert!(!has_color(&field(&tooltip_kept, "text")));
}

#[test]
fn structured_json_is_byte_identical_with_and_without_the_flag() {
    // --json carries raw data and `state`, never presentation.
    let base = run(&["x", "--json"], None);
    for (args, env) in [
        (vec!["x", "--json", "--no-color"], None),
        (vec!["x", "--json", "--no-color=all"], None),
        (vec!["x", "--json", "--no-color=bar"], None),
        (vec!["x", "--json", "--no-color=tooltip"], None),
        (vec!["x", "--json"], Some("1")),
    ] {
        assert_eq!(
            run(&args, env),
            base,
            "{args:?} changed the structured JSON"
        );
    }
    // Raw data and a palette, never a rendering.
    assert!(!base.contains("<span"));

    // With every XDG dir pinned at an empty directory there is no Omarchy
    // theme and no pywal cache, so the palette resolves deterministically to
    // One Dark — which is also the proof that the harness really is hermetic
    // and is not quietly reading the developer's live theme.
    let doc: serde_json::Value = serde_json::from_str(&base).unwrap();
    assert_eq!(doc["palette"]["severity"]["ok"], "#98c379");
    assert_eq!(doc["palette"]["severity"]["critical"], "#e06c75");
    assert_eq!(doc["palette"]["ink"]["cyan"], "#26c6da");
    assert_eq!(
        doc["palette"]["stops"],
        serde_json::json!([
            { "pct": 5,   "state": "critical", "color": "#e06c75" },
            { "pct": 15,  "state": "warn",     "color": "#d19a66" },
            { "pct": 100, "state": "ok",       "color": "#98c379" },
        ])
    );
}

#[test]
fn an_unknown_value_is_an_argument_error_on_both_paths() {
    let msg = "--no-color: unknown value 'bogus' (expected all, bar or tooltip)";

    let waybar: serde_json::Value = serde_json::from_str(&run(&["x", "--no-color=bogus"], None))
        .expect("the waybar error path must still print valid JSON");
    assert_eq!(waybar["text"], "?");
    assert_eq!(waybar["tooltip"], msg);
    assert_eq!(waybar["class"], serde_json::json!(["error"]));

    let structured: serde_json::Value =
        serde_json::from_str(&run(&["x", "--json", "--no-color=bogus"], None))
            .expect("the structured error path must still print valid JSON");
    assert_eq!(structured["schema_version"], 1);
    assert_eq!(structured["printer"], "x");
    assert_eq!(structured["state"], "error");
    assert_eq!(structured["error"]["message"], msg);
}
