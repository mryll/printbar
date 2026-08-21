# Demo fixture

`demo-data` (and the `printbar` alias beside it — a real file, not a symlink: `omarchy-plugin-validate` rejects symlinks anywhere inside a plugin folder, and this repo root *is* the plugin folder) impersonate the CLI and always report the same fictional printer: a `Color Laser MFP 4300` at `printer.local`, printing 2 jobs with Tray 1 out of paper, four supplies spread across the whole gauge (Black 82%, Cyan 46%, Magenta 11%, Yellow 4%) and its own front-panel words on screen. It speaks both output modes and honors `--no-color[=all|bar|tooltip]` and `NO_COLOR` exactly like the real binary, so the monochrome shots go through it too.

```bash
PATH="$PWD/screenshots/demo:$PATH" printbar office          # waybar JSON
PATH="$PWD/screenshots/demo:$PATH" printbar office --json   # structured mode
```

For a widget screenshot, put that `PATH` in front of Waybar or the Omarchy shell so the fixture shadows the installed binary. It is documentation tooling only — nothing in the build, the install target or the test suite touches it. The waybar tooltip picks up the active Omarchy theme the same way the real renderer does; the plugin's "Updated HH:MM" is stamped by the panel when it parses the document.
