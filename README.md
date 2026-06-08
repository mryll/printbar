# printbar

A generic **printer monitor for Waybar**. One printer per module instance; works with **any** printer over the network (IPP + SNMP) or locally (CUPS, covering **USB**). Collects everything the printer exposes and shows only what you configure, in the bar and a themed tooltip that matches [meteobar]/[tickerbar].

```
🖨 54% Idle      ╭────────────────────────────────────────╮
                 │ HP Color LaserJet MFP M477fdw          │
                 │ Status Idle                            │
                 │ Black Cartridge   ▰▰▰▱▱ 54%            │
                 │ Cyan Cartridge    ▰▰▰▰▱ 69%            │
                 │ Magenta Cartridge ▰▰▰▰▰ 81%            │
                 │ Yellow Cartridge  ▰▰▰▰▱ 73%            │
                 │ Tray 2 ok                              │
                 │ Jobs 0   Pages 165                     │
                 ╰────────────────────────────────────────╯
```

## How it works

A one-shot binary that, per poll, runs three sources concurrently and merges them:

- **IPP** (`ipp://host`) — state, supplies, jobs, model.
- **CUPS** (`ipp://localhost:631/printers/<queue>`) — same parser, covers USB / local queues.
- **SNMP** (Printer MIB) — *enrichment*: page counts, paper trays, alerts, supplies. Purely additive: if SNMP is off or fails, the IPP/CUPS output is unaffected.

Supplies are generic — **toner, ink, drum, waste** — so it works with lasers and inkjets alike (CMYK, tri-color, photo inks, maintenance kits).

## Install

```sh
make build
make install PREFIX=~/.local      # installs to ~/.local/bin/printbar
```

Requires `xdg-open` (click actions) and `notify-send` (notifications) at runtime, both optional.

## Configure

`~/.config/printbar/config.toml`, one `[printer.<name>]` section per printer. The module runs `printbar <name>`. See [`config.example.toml`](config.example.toml).

```toml
[printer.oficina]
host = "192.168.1.70"        # enables IPP (+ SNMP if snmp.enabled); omit for USB-only
ipp_path = "/ipp/print"      # default; change for non-standard printers
cups = "HP_M477fdw"          # optional: enables the local CUPS/IPP path (covers USB)
timeout = 4

[printer.oficina.snmp]
enabled = true               # explicit; community alone does NOT enable SNMP
community = "public"

[printer.oficina.bar]
format = "🖨 {supply_min}% {status_icon}"
on_missing = "hide"          # "hide" | "error"

[printer.oficina.tooltip]
items = ["model", "status", "supplies", "paper", "jobs", "pages"]
max_rows = 12

[printer.oficina.thresholds]
supply_low = 15
supply_critical = 5

[printer.oficina.notify]
enabled = true
events = ["jam", "supply_low", "offline"]
```

### Bar tokens

`{supply_min}` (worst consumable), `{toner_min}`, `{ink_min}`, `{black}` `{cyan}` `{magenta}` `{yellow}`, `{status}`, `{status_icon}`, `{model}`, `{name}`, `{jobs}`, `{pages}`, `{paper}`.

A hidden token (when its data is absent and `on_missing = "hide"`) takes any adjacent literal with it, so `"{supply_min}%"` never leaves a dangling `%`.

### Tooltip items

`model`, `status`, `supplies`, `paper`, `jobs`, `pages`. Long lists fold at `max_rows` ("+N more").

## Waybar module

```jsonc
"custom/printbar": {
  "exec": "printbar oficina",
  "return-type": "json",
  "interval": 30,
  "signal": 15,
  "tooltip": true,
  "on-click": "printbar action ews --printer oficina",
  "on-click-right": "printbar action queue --printer oficina"
}
```

Add `"custom/printbar"` to a `modules-*` list. On-demand refresh: `pkill -RTMIN+15 waybar` (use a `signal` number not shared with your other custom modules).

### Styling

The bar emits a `class` of `ok` / `warn` / `critical` / `error` / `offline` (worst current state), so you can color it:

```css
#custom-printbar.warn     { color: #e5c07b; }
#custom-printbar.critical { color: #e06c75; }
#custom-printbar.offline  { color: #5c6370; }
```

The tooltip colors come from your Omarchy theme (`~/.config/omarchy/current/theme/colors.toml`), with a sensible fallback.

## License

MIT.

[meteobar]: ../meteobar
[tickerbar]: ../tickerbar
