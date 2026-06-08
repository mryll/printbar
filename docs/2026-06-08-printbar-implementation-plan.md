# printbar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A generic Waybar widget (one-shot Rust binary) that monitors any printer over IPP (network + local CUPS queue) and SNMP enrichment, printing configurable bar + tooltip JSON.

**Architecture:** Blocking, no async. A `Collector` runs `IppSource` and `SnmpSource` on std threads (recv_timeout), each returning a `SourceOutcome`; a pure `merge()` produces a unified `PrinterState`; `render()` emits Waybar JSON. Every path exits 0 with valid JSON.

**Tech Stack:** Rust 2021, `ipp 6` (blocking), `snmp2 0.5` (no `mib`), `serde`/`serde_json`/`toml`. Tooltip theme/helpers mirror `meteobar`/`tickerbar`.

**Reference:** Spec at `printbar/docs/2026-06-08-printbar-design.md`. All work happens in `printbar/` (its own git repo).

---

## Task 0: Scaffold repo

**Files:** Create `printbar/{Cargo.toml,Makefile,.gitignore,LICENSE,CLAUDE.md,AGENTS.md}`, `printbar/src/main.rs`.

- [ ] **Step 1:** `git init` in `printbar/`. Copy `LICENSE`, `Makefile`, `.gitignore`, `CLAUDE.md`/`AGENTS.md` structure from `tickerbar/` (adjust names). Makefile targets: `build` (`cargo build --release`), `install PREFIX=~/.local` (copies `target/release/printbar` to `$(PREFIX)/bin`).
- [ ] **Step 2:** `Cargo.toml` with deps (resolve to latest stable; record in lockfile):
```toml
[package]
name = "printbar"
version = "0.1.0"
edition = "2021"

[dependencies]
ipp = { version = "6", default-features = false, features = ["client"] }   # blocking client
snmp2 = { version = "0.5", default-features = false }                       # no `mib` feature
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1"
```
- [ ] **Step 3:** `src/main.rs` stub that prints `{"text":"printbar","tooltip":"","class":["ok"],"alt":"ok"}` and exits 0.
- [ ] **Step 4:** Run `cargo build --release` → success. `git add -A && git commit -m "chore: scaffold printbar crate"`.

---

## Task 1: Data model (`src/model.rs`)

**Files:** Create `src/model.rs`; Test: inline `#[cfg(test)]`.

- [ ] **Step 1: Write failing test** for `Level::as_pct` and `Supply::is_usable`:
```rust
#[test] fn level_pct_and_sentinels() {
    assert_eq!(Level::Pct(80).as_pct(), Some(80));
    assert_eq!(Level::Unknown.as_pct(), None);
    assert_eq!(Level::NoRestriction.as_pct(), None);
}
#[test] fn usable_requires_real_consumable() {
    let waste = Supply{ kind: SupplyKind::Waste, class: SupplyClass::Filled,
        level: Level::Pct(10), name:"Waste".into(), color:None, color_raw:None, max_capacity:None, unit:None };
    let toner = Supply{ kind: SupplyKind::Toner, class: SupplyClass::Consumed,
        level: Level::Pct(54), name:"Black".into(), color:Some(Color::Black), color_raw:None, max_capacity:None, unit:None };
    assert!(!waste.is_usable());     // lone waste/sentinel ≠ usable set member
    assert!(toner.is_usable());
}
```
- [ ] **Step 2:** Run `cargo test model::tests` → FAIL (types undefined).
- [ ] **Step 3:** Define enums/structs exactly as spec §5: `Status`, `Reason`, `SupplyKind`, `SupplyClass`, `Color`, `SupplyUnit`, `Level{Pct(u8),NoRestriction,Unknown,SomeRemaining}`, `Supply`, `InputTray`, `PrinterState` (all derive `Debug,Clone,PartialEq`). Implement `Level::as_pct`, `Supply::is_usable` (real `Consumed` consumable with `Level::Pct`).
- [ ] **Step 4:** Run test → PASS.
- [ ] **Step 5:** Commit `feat: printer data model`.

---

## Task 2: Config (`src/config.rs`)

**Files:** Create `src/config.rs`, `config.example.toml`.

- [ ] **Step 1: Failing test** — parse a TOML string with one `[printer.oficina]` section (host, ipp_path default, cups, snmp.enabled, bar.format, tooltip.items+max_rows, thresholds, actions, notify) into a `PrinterConfig`; assert defaults (`ipp_path="/ipp/print"`, `timeout=4`, `on_missing=Hide`).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Define `serde` structs: `Config{ printer: HashMap<String,PrinterConfig> }`, `PrinterConfig{ host:Option<String>, ipp_path:String(default), cups:Option<String>, timeout:u64(default 4), snmp:SnmpCfg, bar:BarCfg, tooltip:TooltipCfg, thresholds:Thresholds, actions:ActionsCfg, notify:NotifyCfg }` with `#[serde(default)]` + `Default`. `OnMissing{Hide,Error}`. `load(path)->Result`, `for_printer(name)`.
- [ ] **Step 4:** Run → PASS. Write `config.example.toml` matching spec §8.
- [ ] **Step 5:** Commit `feat: config parsing`.

---

## Task 3: Waybar output + theme + helpers (`src/waybar.rs`, `src/theme.rs`)

**Files:** Create `src/waybar.rs`, `src/theme.rs` (port from `meteobar/src/theme.rs` + `tickerbar/src/platform/waybar.rs`).

- [ ] **Step 1: Failing test** — `error_output("boom")` serializes to `{"text":"?","tooltip":"boom","class":["error"],"alt":"error"}`; `fg("#fff","x")=="<span foreground='#fff'>x</span>"`; `visible_len("<span foreground='#fff'>ab</span>")==2`.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `WaybarOutput{text,tooltip,class:Vec<String>,alt}` (Serialize), `print(&self)`, `error_output(reason)->String`, `fg`/`bold_fg`/`pango_escape`/`visible_len`/frame helpers (`top_border`/`bottom_border`/`mid`). Port `ThemeColors{border,text,dim,accent,green,yellow,orange,error}` + `load()` from omarchy + fallback, verbatim field names from `meteobar/src/theme.rs:47-52`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `feat: waybar JSON + theme + pango helpers`.

---

## Task 4: Merge (`src/merge.rs`) — pure, TDD-heavy

**Files:** Create `src/merge.rs`, `src/sources/mod.rs` (just `SourceKind`, `SourceOutcome`, `PartialPrinter` here so merge compiles).

- [ ] **Step 1:** In `sources/mod.rs` define `enum SourceKind{Ipp,Cups,Snmp}`, `struct PartialPrinter{ /* all PrinterState fields as Option/Vec */ }`, `struct SourceOutcome{kind,partial,duration,error:Option<String>}`, `trait Source{ fn collect(&self,&Target,Duration)->SourceOutcome; }`, `struct Target{host,ipp_path,cups,...}`.
- [ ] **Step 2: Failing tests** for `merge(Vec<SourceOutcome>)->PrinterState`:
  - `supplies_taken_wholesale_from_highest_usable`: SNMP partial has only a waste row (not usable) + IPP has full CMYK → result supplies == IPP's (not merged, not SNMP's lone waste).
  - `jobs_prefer_cups_over_network`: CUPS jobs=3, IPP jobs=0 → 3.
  - `status_prefers_ipp`: IPP=Printing, SNMP=Idle → Printing.
  - `reasons_ipp_primary_plus_active_critical_snmp`: IPP=[Jam], SNMP alert active+critical=[CoverOpen], SNMP non-active=[MediaLow] → {Jam,CoverOpen} (MediaLow dropped, deduped).
  - `all_sources_failed_is_offline`: all outcomes error/empty → status Offline, rest empty.
- [ ] **Step 3:** Run → FAIL.
- [ ] **Step 4:** Implement `merge` per spec §7 (priority constants, `is_usable`-gated supply selection, reason add+dedupe, offline fallback). Pure function, no I/O.
- [ ] **Step 5:** Run → PASS.
- [ ] **Step 6:** Commit `feat: source merge logic`.

---

## Task 5: Render (`src/render.rs`) — bar template + tooltip

**Files:** Create `src/render.rs`.

- [ ] **Step 1: Failing tests:**
  - `token_substitution_basic`: format `"🖨 {supply_min}% {status_icon}"` with supply_min=54,status=Idle → `"🖨 54% <idle glyph>"`.
  - `hidden_token_absorbs_adjacent_literal`: `on_missing=Hide`, supply_min absent, format `"{supply_min}% ok"` → `"ok"` (no dangling `%`).
  - `missing_token_error_mode`: `on_missing=Error` → token renders `n/d` and class includes `error`.
  - `class_from_thresholds_consumed_vs_filled`: Consumed level 4 with critical=5 → `["critical"]`; Filled (waste) level 96 with critical(high)=95 → `["critical"]`.
  - `tooltip_caps_rows`: 20 supplies, max_rows=12 → 12 rows + `"+8 more"` (framed).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `render(state,&PrinterConfig,&ThemeColors)->WaybarOutput`: token map; literal-absorbing substitution (scan `{tok}` with surrounding non-space literals, drop together when hidden); `class` derivation (worst of threshold-by-class + reasons); framed themed tooltip from `tooltip.items` with supply bars (`▰▰▰▱`) colored by class-aware thresholds, row caps + "+N more".
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `feat: bar + tooltip rendering`.

---

## Task 6: IPP source (`src/sources/ipp.rs`) — shared parser

**Files:** Create `src/sources/ipp.rs`.

- [ ] **Step 1: Failing test** with a **fixture**: a saved `IppAttributes`-equivalent map (or a parse function taking the attribute group) for the real M477fdw → `parse_ipp_attrs(attrs)->PartialPrinter` yields status=Idle, supplies CMYK with names/colors/levels, model contains "M477", jobs from `queued-job-count`. Add an inkjet fixture (named ink + tri-color) and a short/malformed marker-array fixture (levels len ≠ names len → pair defensively, no panic).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `parse_ipp_attrs` (pure, over the attribute map) mapping `printer-state`,`printer-state-reasons`,`marker-*`(levels/names/colors/types/high/low),`queued-job-count`,`printer-make-and-model`,`printer-info`. Color: store raw hex/name + normalized. Then `IppSource::collect` building the URI (host+ipp_path OR `ipp://localhost:631/printers/<queue>`), blocking `ipp` client with protocol timeout, calling `parse_ipp_attrs`, returning `SourceOutcome`. Keep network I/O thin; parsing is the unit-tested pure fn.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `feat: IPP source (network + cups paths)`.

---

## Task 7: Collector + main wiring (IPP-only path is now a usable widget)

**Files:** Modify `src/sources/mod.rs` (threaded runner), `src/main.rs`.

- [ ] **Step 1: Failing test** — `run_sources(target, sources, deadline)` with a fake slow source (sleeps past deadline) + a fast one → returns the fast outcome and a timeout outcome for the slow one, within ~deadline (no hang).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement threaded runner: spawn each source on a thread sending `SourceOutcome` over a bounded `mpsc`; collector `recv_timeout` until deadline; missing sources recorded as timeout. Wire `main.rs`: parse args (`<name>` | `action ...`), load config, build sources (IPP always if host/cups; SNMP added in Task 9), run, merge, render, print. Top-level: any `Err` → `error_output` JSON, exit 0.
- [ ] **Step 4:** Run → PASS. `cargo build --release`; manual smoke: `printbar oficina` against M477fdw prints real JSON.
- [ ] **Step 5:** Commit `feat: collector + main (IPP path end-to-end)`.

---

## Task 8: Actions (`src/actions.rs`)

**Files:** Create `src/actions.rs`; Modify `main.rs`.

- [ ] **Step 1: Failing test** — `ews_url(&PrinterConfig)` builds `http://192.168.1.70` from host; respects explicit configured URL, https, port, and bracketed IPv6.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement `ews_url`, `queue_target`, and `run(action,&cfg)` → `xdg-open` (best-effort). Wire `printbar action ews|queue --printer <name>`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `feat: click actions (ews/queue)`.

---

## Task 9: SNMP source (`src/sources/snmp.rs`) — enrichment

**Files:** Create `src/sources/snmp.rs`.

- [ ] **Step 1: Failing tests** with **fixtures** (captured SNMP walks, parsed offline):
  - `parse_marker_supplies_with_colorant_join`: supplies rows + colorant table → supplies with normalized colors; `prtMarkerSuppliesClass` → Consumed/Filled (waste → Filled).
  - `sentinels_map_distinct`: level -1→NoRestriction, -2→Unknown, -3→SomeRemaining; max_capacity used to normalize positive levels to pct.
  - `pages_aggregation_max_across_marker_rows`.
  - `trays_from_prtInput`; `alerts_active_critical_only` from `prtAlertTable`.
  - `walk_caps`: a giant table is truncated at max-rows (no unbounded growth).
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement pure parsers over fixture row-maps, then `SnmpSource::collect` (snmp2 v2c GETBULK with OID-prefix bounds, max-repetitions, row cap, protocol timeout). Gated by `snmp.enabled`+`host`.
- [ ] **Step 4:** Run → PASS. Wire SNMP into `main.rs` source list. Smoke vs M477fdw: pages/trays now populate.
- [ ] **Step 5:** Commit `feat: SNMP enrichment source`.

---

## Task 10: Notifications (`src/notify.rs`)

**Files:** Create `src/notify.rs`; Modify `main.rs` (call after render).

- [ ] **Step 1: Failing tests:**
  - `transition_fires_once`: prev=no-jam, cur=jam, events=[jam] → one notification spec; second poll cur=jam → none (steady state).
  - `cache_path_sanitizes_name`: printer name `../evil` → safe filename.
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Implement: `cache_path(name)` (sanitize; `$XDG_RUNTIME_DIR` else temp dir), atomic write (temp+rename), `diff_events(prev,cur,&events)->Vec<Notification>`, `emit(n)` shells `notify-send` best-effort with short timeout (never fails poll). Pure `diff_events` is the tested part.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit `feat: transition notifications`.

---

## Task 11: Packaging, docs, dogfood

**Files:** `README.md`, `screenshots/`, `aur/printbar/{PKGBUILD,.SRCINFO}`, `aur/printbar-bin/{PKGBUILD,.SRCINFO}`.

- [ ] **Step 1:** README: what it is, config reference (from `config.example.toml`), the Waybar module snippet (`custom/printbar` with `exec`, `interval`, `signal`, `on-click`, `on-click-right`, `return-type":"json"`), CSS class examples, signal-number note (pick one unused vs other widgets).
- [ ] **Step 2:** AUR `printbar` (build from source) + `printbar-bin` PKGBUILDs modeled on `aur/tickerbar*`; `updpkgsums`; `makepkg --printsrcinfo > .SRCINFO`.
- [ ] **Step 3:** `cargo clippy` clean, `cargo fmt`. Full `cargo test` green.
- [ ] **Step 4:** Dogfood: add the module to `~/.config/waybar/config.jsonc` (backup first per omarchy), add CSS to `style.css`, `omarchy restart waybar`, point at the M477fdw, verify bar + tooltip + click + a forced notification.
- [ ] **Step 5:** Capture `screenshots/`. Commit `docs: readme, screenshots, AUR packaging`.

---

## Self-Review notes
- Spec coverage: §3 contract (T3,T7), §4 collector/concurrency (T7), §5 model (T1), §6 sources (T6,T9), §7 merge (T4), §8 config (T2), §9 render/tooltip caps (T5), §10 actions/notify (T8,T10), §11 structure (all), §12 deps (T0), §13 tests (each task), §14 sequencing (task order). No gaps.
- Types consistent across tasks: `PartialPrinter`/`SourceOutcome`/`SourceKind` defined in T4 before use; `Level`/`Supply`/`SupplyClass` from T1 used throughout.
- TDD-pure boundaries: parsers/merge/render/notify-diff are pure fns tested with fixtures; network I/O is thin wrappers.
