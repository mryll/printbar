# AGENTS.md — printbar

Generic Waybar printer widget. One-shot Rust binary: collect (IPP + SNMP) → merge → print Waybar JSON, exit 0.

- MUST exit 0 with valid Waybar JSON (`{"text","tooltip","class":[..],"alt"}`) on EVERY path, including errors (see `error_output`).
- Blocking only — no async runtime. Sources run on std threads with `recv_timeout`.
- Tooltip uses Pango markup (not HTML), framed/themed like meteobar/tickerbar.
- Build: `make build`; install: `make install PREFIX=~/.local`. Lint: `cargo clippy`; format `cargo fmt`.
- Design spec: `docs/2026-06-08-printbar-design.md`. Plan: `docs/2026-06-08-printbar-implementation-plan.md`.
