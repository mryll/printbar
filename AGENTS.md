# AGENTS.md — printbar

Generic Waybar printer widget. One-shot Rust binary: collect (IPP + SNMP) → merge → print Waybar JSON, exit 0.

- MUST exit 0 with valid Waybar JSON (`{"text","tooltip","class":[..],"alt"}`) on EVERY path, including errors (see `error_output`).
- Blocking only — no async runtime. Sources run on std threads with `recv_timeout`.
- Tooltip uses Pango markup (not HTML), framed/themed like meteobar/tickerbar.
- Build: `make build`; install: `make install PREFIX=~/.local`. Lint: `cargo clippy`; format `cargo fmt`.
- Design spec: `docs/2026-06-08-printbar-design.md`. Plan: `docs/2026-06-08-printbar-implementation-plan.md`.

## Release

A release is automated by pushing a tag — do NOT build or upload the binary by hand:

1. Bump `version` in `Cargo.toml` + `Cargo.lock`; commit `chore: release X.Y.Z`.
2. `git tag vX.Y.Z && git push origin master --tags`.
3. The tag push triggers `.github/workflows/release.yml`, which builds and publishes the GitHub release with the asset `printbar-X.Y.Z-x86_64-linux` (consumed by the `printbar-bin` AUR package; its other files — `printbar-watch`, the `.service`, `config.example.toml` — are pulled from the tag, not the release).
4. Only after the release exists, bump both AUR repos (`aur/printbar` source + `aur/printbar-bin`) per the workspace `AGENTS.md`. Order matters: `updpkgsums` fetches the tag tarball AND the release asset, so both must already be live.
