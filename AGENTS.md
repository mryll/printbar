# AGENTS.md — printbar

Generic Waybar printer widget. One-shot Rust binary: collect (IPP + SNMP) → merge → print Waybar JSON, exit 0.

- MUST exit 0 with valid Waybar JSON (`{"text","tooltip","class":[..],"alt"}`) on EVERY path, including errors (see `error_output`).
- Blocking only — no async runtime. Sources run on std threads with `recv_timeout`.
- Tooltip uses Pango markup (not HTML), framed/themed like meteobar/tickerbar. Escape every string that came off the wire — on the bar and the error paths too, not just the tooltip.
- Theme chain (`theme.rs::load_from`): Omarchy `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` (legacy `~/.config/omarchy/...` as fallback) → pywal `$XDG_CACHE_HOME/wal/colors.json` → built-in One Dark. An empty XDG var means unset. Every field degrades on its own, and values are validated DURING selection so an invalid semantic key falls through to its legacy alias.
- `palette.rs` owns every color printbar itself defines (severity from the theme, ink per colorant) and the supply ramp's stops. `supply_state` classifies against `supply_stops`, and `--json` publishes both, so the QML panel never keeps a second copy. The panel's own chrome still uses the shell's live `Color` tokens — deliberate, see the header comment in `omarchy/Panel.qml`.
- Theme tests must never read the real environment: go through `load_from`/`dir_from`/`candidate_paths`, and pin `HOME` + the XDG dirs in `tests/cli.rs`.
- Build: `make build`; install: `make install PREFIX=~/.local`. Lint: `cargo clippy`; format `cargo fmt`.

## Release

A release is automated by pushing a tag — do NOT build or upload the binary by hand:

1. Bump `version` in `Cargo.toml` + `Cargo.lock`; commit `chore: release X.Y.Z`.
2. `git tag vX.Y.Z && git push origin master --tags`.
3. The tag push triggers `.github/workflows/release.yml`, which builds and publishes the GitHub release with the asset `printbar-X.Y.Z-x86_64-linux` (consumed by the `printbar-bin` AUR package; its other files — `printbar-watch`, the `.service`, `config.example.toml` — are pulled from the tag, not the release).
4. Only after the release exists, bump both AUR repos (`aur/printbar` source + `aur/printbar-bin`) per the workspace `AGENTS.md`. Order matters: `updpkgsums` fetches the tag tarball AND the release asset, so both must already be live.
