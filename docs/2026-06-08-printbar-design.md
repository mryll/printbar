# printbar — Design Spec

**Date:** 2026-06-08
**Status:** Reviewed (Codex consensus, 2 rounds) — ready for implementation plan
**Repo:** `waybar-widgets/printbar/` (own git repo, follows the monorepo Shared Widget Contract)

## 1. Purpose

A Waybar widget that monitors a **printer** and shows configurable status/supplies/jobs info in the bar and tooltip. Generic by design: works with **any** printer regardless of brand or connection (network or USB), collecting everything the printer exposes and showing only what the user configures.

## 2. Goals / Non-goals

**Goals**
- Collect the maximum data a printer exposes, via standard protocols, merged from multiple sources.
- Cover **network** printers (IPP + SNMP) and **USB / local-queue** printers (CUPS, which is IPP at localhost).
- Display fully configurable: bar via template, tooltip via item list. Per-section "hide vs explicit error" on missing data.
- One printer per module instance (multi-instance for multiple printers), matching the `logibar` precedent.
- Click actions (open EWS / open queue) and desktop notifications (mako) on state transitions.
- Tooltip visually consistent with `meteobar`/`tickerbar`.

**Non-goals (v1)**
- Auto-discovery (mDNS/Bonjour). Future.
- One module aggregating multiple printers. Future.
- Controlling the printer (cancel jobs, change settings). Read-only monitor.
- Historical graphs / page-count trends.

## 3. Language & execution model

- **Rust** (consistent with `meteobar`/`tickerbar`). Fully **blocking** — no async runtime.
- **One-shot binary**: run → collect → print Waybar JSON → exit 0. Waybar drives cadence via the module `interval`; also supports refresh via real-time signal `RTMIN+N` (signal number documented in README, must not collide with other widgets).
- **Exit-0 JSON contract**: EVERY path produces valid Waybar JSON `{"text","tooltip","class":[...],"alt"}` — config parse error, missing printer section, render error, serialization error, a source panic. Mirror tickerbar's `error_output` helper (`tickerbar/src/platform/waybar.rs:93`): on any fatal error, emit `{"text":"?","tooltip":"<reason>","class":["error"],"alt":"error"}` and exit 0.
- Subcommands:
  - `printbar <name>` (default: poll & print JSON for the printer section `<name>`).
  - `printbar action <ews|queue> --printer <name>` (used by Waybar `on-click` / `on-click-right`).

## 4. Architecture: multi-source collector

```
  config[name] ──▶  Collector
                     ├─ IppSource   (one parser; URI = ipp://host/<path>  OR  ipp://localhost:631/printers/<queue>)
                     └─ SnmpSource  (network: SNMP Printer MIB @ host, enrichment)
                          │  each returns (SourceKind, SourceOutcome)
                          ▼
                     merge() ──▶ PrinterState (unified)
                          ▼
            render() ─┬─ bar text   (template)
                      └─ tooltip    (item list, framed, themed, capped)
            notify()  (compare vs cached previous state → mako on transitions)
```

- `trait Source { fn collect(&self, target: &Target, timeout: Duration) -> SourceOutcome; }`
- `SourceOutcome { kind: SourceKind, partial: PartialPrinter, duration: Duration, error: Option<String> }` — carries identity + diagnostics for priority merge, error reporting, and fixture debugging.
- **CUPS and direct IPP share ONE IPP attribute parser.** They differ only in URI construction and job-source semantics. A `Target` may declare a network `host` (→ direct IPP at `ipp://host/<ipp_path>`) and/or a `cups` queue (→ IPP at `ipp://localhost:631/printers/<queue>`). SNMP applies only when `host` is set and `snmp.enabled`.
- Each source is independent, testable in isolation, and **best-effort**: a source that errors, times out, or is not applicable contributes nothing rather than failing the poll. **Acceptance bar: IPP/CUPS alone produces valid output; SNMP only adds pages/trays/alerts/supplies when it succeeds and never degrades the base output.**

### Concurrency
Blocking `ipp` (blocking client feature) + sync `snmp2` (no `mib` feature), each source on a `std::thread`, results delivered over a bounded channel. The collector waits with `recv_timeout(deadline)`; on timeout it records a timeout `SourceOutcome` and renders **without joining** the slow worker (a channel timeout cannot cancel a worker, so each source ALSO sets protocol-level connect/read timeouts and a retry cap). No Tokio.

## 5. Data model (generic supplies)

Consumables are modeled generically because SNMP `prtMarkerSupplies` / IPP `marker-*` describe any supply (toner, ink, drum, waste, maintenance kit), each with a type, description, color, level and capacity.

```rust
struct PrinterState {
    name: Option<String>,
    model: Option<String>,
    status: Option<Status>,          // Idle | Printing | Stopped | Offline | Unknown
    reasons: Vec<Reason>,            // Jam, MediaEmpty, MediaLow, SupplyLow, CoverOpen, ...
    supplies: Vec<Supply>,           // wholesale from ONE source (see §7)
    paper: Vec<InputTray>,           // tray name, level, status
    pages: Option<u64>,              // life page count
    jobs: Option<u32>,               // queued jobs
}

struct Supply {
    name: String,                    // "Black Toner", "Tri-color Ink", "Maintenance Kit"
    kind: SupplyKind,                // Toner | Ink | Drum | Waste | Other
    class: SupplyClass,              // Consumed (low = bad) | Filled (high = bad, e.g. waste tank)
    color_raw: Option<String>,       // raw marker-color (hex sRGB) or SNMP colorant name
    color: Option<Color>,            // normalized display color (Cyan/Magenta/Yellow/Black/TriColor/Photo/Other)
    level: Level,                    // Pct(u8) | NoRestriction | Unknown | SomeRemaining
    max_capacity: Option<i32>,       // for SNMP level→pct normalization
    unit: Option<SupplyUnit>,        // SNMP prtMarkerSuppliesSupplyUnit
}

enum Level { Pct(u8), NoRestriction /* -1 */, Unknown /* -2 */, SomeRemaining /* -3 */ }
```

Sentinels (RFC 3805 / CUPS `-1/-2/-3`) are DISTINCT states, not collapsed to one. Threshold/coloring respects `class`: a `Consumed` supply is critical when low; a `Filled` (waste) supply is critical when high.

**Template/tooltip vocabulary:**
- `{supply_min}` — worst consumable (for `Consumed`: lowest %; `Filled` contributes its headroom) across supplies.
- `{black} {cyan} {magenta} {yellow}` — by normalized color.
- `{ink_min}` / `{toner_min}` — aliases filtered by `kind`.
- `{status} {status_icon} {model} {name} {jobs} {pages} {paper}` — scalars.
- Token renderer absorbs an adjacent literal (e.g. trailing `%`, surrounding spaces) when a token resolves to hidden, so `"{supply_min}%"` leaves no dangling `%`.
- Tooltip item `supplies` lists each supply with real name + color + level bar (capped, see §9).

## 6. Sources

**IppSource** (covers USB/local via CUPS AND network)
- `Get-Printer-Attributes` over `ipp`/`ipps`. URI: `ipp://host/{ipp_path}` (configurable, default `/ipp/print`; `/ipp/print` is common but NOT mandatory per PWG) OR `ipp://localhost:631/printers/{queue}` for the CUPS path.
- Provides: `status` (`printer-state`), `reasons` (`printer-state-reasons`), `supplies` (`marker-levels` + `marker-names` + `marker-colors` + `marker-types` + `marker-high-levels` + `marker-low-levels`), `jobs` (`queued-job-count`; from CUPS path counts the local spool), `model` (`printer-make-and-model`), `name` (`printer-info`).

**SnmpSource** (network enrichment, `host` + `snmp.enabled`)
- SNMP v2c (community configurable, **explicit `snmp.enabled`** — community presence does NOT imply enabled), numeric OIDs (no MIB compilation), Printer MIB (RFC 3805/1759):
  - `prtMarkerSupplies*` → supplies; join `prtMarkerSuppliesColorantIndex` → `prtMarkerColorant` table for color; normalize `Level`/`MaxCapacity`→pct; map `SupplyClass` from `prtMarkerSuppliesClass`; sentinels distinct.
  - `prtMarkerLifeCount` → `pages` (aggregation rule: max across marker rows).
  - `prtInput*` → `paper` trays (level/max/status).
  - `prtAlertTable` → `reasons`, filtered to active + severity critical/warning.
  - `hrPrinterStatus`/`hrDeviceStatus` → `status`.
- Table walks (GETBULK) need OID-prefix bounds, a max-rows cap, and a max-repetitions limit.

## 7. Merge strategy

`merge(outcomes) -> PrinterState`, pure (no I/O), fully unit-testable:
- **supplies**: taken WHOLESALE from the single highest-priority source returning a **usable** set — priority SNMP > IPP > CUPS. "Usable" = at least one real consumable with known kind/name and a non-sentinel level; a lone `waste`/sentinel row does NOT count as usable (so it can't suppress a full IPP/CUPS marker list). No element-wise cross-source supply merge → no dedup hazard.
- **paper / pages / model**: prefer SNMP, then IPP.
- **jobs**: prefer the **CUPS** path (local spool backlog) over a network printer's direct `queued-job-count`.
- **status**: prefer IPP, then CUPS, then SNMP.
- **reasons**: IPP `printer-state-reasons` is primary; SNMP `prtAlertTable` only ADDS entries (active + severity critical/warning), then dedupe. Not a blind union.
- All sources failed/unreachable → `status = Offline`, rest empty.

## 8. Configuration

`~/.config/printbar/config.toml`, one `[printer.<name>]` section per printer; module invokes `printbar <name>`. Hybrid: template for the bar, item list for the tooltip.

```toml
[printer.oficina]
host = "192.168.1.70"          # enables IPP (+ SNMP if snmp.enabled); omit for USB-only
ipp_path = "/ipp/print"        # default; configurable for non-standard printers
cups = "HP_M477fdw"            # optional: enables the CUPS/local IPP path (USB/local)
timeout = 4                    # per-source seconds (protocol-level)

[printer.oficina.snmp]
enabled = true                 # explicit; not inferred from community
community = "public"

[printer.oficina.bar]
format = "🖨 {supply_min}% {status_icon}"
on_missing = "hide"            # "hide" | "error"

[printer.oficina.tooltip]
items = ["model","status","supplies","paper","jobs","pages"]
on_missing = "hide"
max_rows = 12                  # cap (see §9)

[printer.oficina.thresholds]   # drive the `class`
supply_low = 15
supply_critical = 5

[printer.oficina.actions]
on_click = "ews"               # xdg-open the EWS URL (configurable scheme/port/IPv6; default http://host)
on_click_right = "queue"

[printer.oficina.notify]
enabled = true
events = ["jam","supply_low","offline"]
```

Note: `interval` is NOT a printbar key — Waybar owns the poll interval in its own module config. printbar ships a Waybar module snippet (README + example) where the user sets `interval`/`signal`. A `config.example.toml` ships with the repo.

## 9. Presentation

**Bar**
- Rendered from `bar.format` with token substitution (custom, no templating crate). Nerd Font printer glyph default.
- `class`: `Vec<String>`, worst current state — one of `["ok"]`/`["warn"]`/`["critical"]`/`["error"]`/`["offline"]` — derived from thresholds (`class`-aware) + reasons, so user CSS colors it (matches `meteobar` `Vec<String>` classes).
- `alt` = the primary class string.

**Tooltip — MUST match `meteobar`/`tickerbar` style:**
- Framed box: `╭─…─╮` / `│` / `╰─…─╯`, separators `─`/`│`, in theme border color.
- Colors from `~/.config/omarchy/current/theme/colors.toml` into `ThemeColors { border, text, dim, accent, green, yellow, orange, error }` (same struct/fields/fallback as `meteobar/src/theme.rs`).
- Pango spans via shared `fg(color,text)` / `bold_fg(color,text)`; alignment via `visible_len()`. Pango markup, never HTML.
- Supplies as labeled bars, e.g. `Cyan  ▰▰▰▰▱ 78%`, colored green/yellow/orange/error by `class`-aware threshold.
- **Bounded growth**: supplies/trays/alerts respect `max_rows` with overflow folding ("+N more"), reusing tickerbar's row/column cap approach (`tickerbar/src/platform/render.rs:548`).

## 10. Behaviors

**Click actions** — Waybar maps `on-click` → `printbar action ews --printer <name>` (→ `xdg-open` the configured EWS URL; default `http://host`, supports https/port/IPv6/explicit URL), `on-click-right` → `printbar action queue`.

**Notifications (mako)** — each poll caches `PrinterState` to a sanitized path (printer name sanitized; `$XDG_RUNTIME_DIR` with fallback to a temp dir), written atomically (temp + rename). On the next poll, configured `events` that newly become true (transition, not steady state) fire a single best-effort `notify-send` with a short timeout (never fails the poll). Events: `jam`, `supply_low`, `offline`, + any reason.

## 11. Project structure

```
printbar/
  Cargo.toml, Cargo.lock
  Makefile                 # build (cargo build --release), install PREFIX=~/.local
  README.md  LICENSE  CLAUDE.md  AGENTS.md
  config.example.toml
  docs/2026-06-08-printbar-design.md
  src/
    main.rs                # CLI: poll (default) | action; top-level error → error_output JSON
    config.rs              # TOML load, per-printer section, snmp.enabled, ipp_path
    model.rs               # PrinterState, Supply, Level, SupplyClass, SupplyKind, Status, Reason
    merge.rs               # pure merge(outcomes) -> PrinterState
    render.rs              # bar template (literal-absorbing) + tooltip (framed, themed, capped)
    theme.rs               # ThemeColors::load() from omarchy (same as meteobar)
    waybar.rs              # JSON out + error_output + fg/bold_fg/pango_escape/visible_len/frame helpers
    notify.rs              # transition detection + best-effort notify-send (libnotify binary, NOT the `notify` crate)
    actions.rs             # ews/queue → xdg-open
    sources/
      mod.rs               # Source trait, SourceOutcome, threaded runner w/ recv_timeout
      ipp.rs               # shared IPP parser (host path + localhost/cups path)
      snmp.rs              # snmp2 (no mib), numeric OIDs, table walks w/ caps
  tests/                   # merge & render fixtures, IPP/SNMP parsing fixtures
  screenshots/
aur/
  printbar/      (PKGBUILD from source)
  printbar-bin/  (PKGBUILD from release binary)
```

## 12. Dependencies (latest stable — pin in Cargo.lock)

- `ipp 6.0.0` (blocking client feature; NOT `ipp-util`, which is a CLI).
- `snmp2 0.5.0` **without the `mib` feature** (pure Rust, sync, GETBULK; numeric OIDs).
- `toml 1.1`, `serde 1.0.228` (derive), `serde_json 1.0.150`.
- No `notify` crate (notifications shell out to `notify-send`).
- The implementation plan names the exact resolved versions.

## 13. Testing

- **Unit**: `merge` priority + usable-set selection + reasons add/dedupe + jobs/status/supplies priority; `render` token substitution + literal absorption + `on_missing` hide/error; threshold→class (Consumed vs Filled); SNMP sentinel handling; colorant-index join; pages aggregation.
- **Fixtures** (offline, no network): captured IPP attribute sets + SNMP walks for: real HP M477fdw, an inkjet (named ink + tri-color), an unknown/sentinel-level case, a waste-tank case, malformed/short marker arrays, oversized supply/tray lists.
- **Failure matrix**: single source fails; source times out; ALL sources fail (→ offline, valid JSON); SNMP partial/garbage never degrades IPP base.
- **Smoke**: run against the real M477fdw (`192.168.1.70`) during dogfooding.
- Lints: `cargo clippy`; format `cargo fmt`.

## 14. Implementation sequencing

1. Scaffold repo + model + config + waybar/theme helpers + error_output contract.
2. **IppSource** (shared parser) + merge (IPP/CUPS only) + render + actions → a useful widget on IPP alone, with fixtures.
3. **SnmpSource** enrichment + its fixtures (colorant joins, sentinels, trays, alerts, walk caps, timeout).
4. **notify** + tooltip caps + AUR packaging + README/screenshots.
5. Dogfood against the real M477fdw, then a final Codex review of the implementation.
