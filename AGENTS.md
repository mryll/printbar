# AGENTS.md — printbar

Generic Waybar printer widget. One-shot Rust binary: collect (IPP + SNMP) → merge → print Waybar JSON, exit 0.

- MUST exit 0 with valid Waybar JSON (`{"text","tooltip","class":[..],"alt"}`) on EVERY path, including errors (see `error_output`).
- Blocking only — no async runtime. Sources run on std threads with `recv_timeout`.
- Tooltip uses Pango markup (not HTML), framed/themed like meteobar/tickerbar. Escape every string that came off the wire — on the bar and the error paths too, not just the tooltip.
- Theme chain (`theme.rs::load_from`): Omarchy `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` (legacy `~/.config/omarchy/...` as fallback) → pywal `$XDG_CACHE_HOME/wal/colors.json` → built-in One Dark. An empty XDG var means unset. Every field degrades on its own, and values are validated DURING selection so an invalid semantic key falls through to its legacy alias.
- `palette.rs` owns every color printbar itself defines (severity from the theme, ink per colorant) and the supply ramp's stops. `supply_state` classifies against `supply_stops`, and `--json` publishes both, so the QML panel never keeps a second copy. The panel's own chrome still uses the shell's live `Color` tokens — deliberate, see the header comment in `omarchy/Panel.qml`.
- Theme tests must never read the real environment: go through `load_from`/`dir_from`/`candidate_paths`, and pin `HOME` + the XDG dirs in `tests/cli.rs`.
- Build: `make build`; install: `make install PREFIX=~/.local`. Lint: `cargo clippy`; format `cargo fmt`.
- **A tooltip meter is PARKED, not rendered in place.** `build_tooltip` pushes a `METER<i>` sentinel row plus a `MeterRow` into `meters`, and the width pass resolves them. The bar has to reach the tooltip's right edge, and that edge is the widest TEXT row — which does not exist yet while the supplies are being built. The width pass MUST skip `SEP` and `METER` rows, or the measurement is circular. Every meter in one tooltip gets the SAME bar length: they stack, so a reader compares them against each other.
- **`screenshots/demo/demo-data` RE-IMPLEMENTS the tooltip renderer in bash.** The README screenshots are made from it, so a change to `build_tooltip`'s geometry has to be mirrored there in the same commit, or the published screenshots stop showing the product.
- **Quickshell emits NEITHER `started` NOR `exited` when the command does not exist** — `running` just drops back to false. That is the only signal a failed start gives. Anything that waits on `onExited` to leave a loading state hangs for ever when the CLI is not installed, which is the first run of everyone who installs the plugin from the marketplace: the plugin is a git clone, the CLI is a package, and nothing installs the second for you. The `onRunningChanged` guard in the panel's `Process` is what makes the not-installed message reachable — verified against a running shell, not assumed.

## Release

A release is automated by pushing a tag — do NOT build or upload the binary by hand:

1. Bump `version` in `Cargo.toml` + `Cargo.lock` AND in `manifest.json` (the marketplace shows the manifest's version; it must equal the tag); commit `chore: release X.Y.Z` on `develop` and push.
2. Move master to the release — master only advances here: `git push origin develop:master`. Then `git tag vX.Y.Z && git push origin --tags`.
3. The tag push triggers `.github/workflows/release.yml`, which builds and publishes the GitHub release with the asset `printbar-X.Y.Z-x86_64-linux` (consumed by the `printbar-bin` AUR package; its other files — `printbar-watch`, the `.service`, `config.example.toml` — are pulled from the tag, not the release).
4. Only after the release exists, bump both AUR repos (`aur/printbar` source + `aur/printbar-bin`) per the workspace `AGENTS.md`. Order matters: `updpkgsums` fetches the tag tarball AND the release asset, so both must already be live.
