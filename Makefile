PREFIX  ?= /usr/local
BIN     := $(PREFIX)/bin
# Local installs: ~/.config/systemd/user always works for `systemctl --user`.
# AUR/system packaging overrides UNITDIR=/usr/lib/systemd/user.
UNITDIR ?= $(HOME)/.config/systemd/user

build:
	cargo build --release

install: build
	install -Dm755 target/release/printbar "$(BIN)/printbar"
	install -Dm755 printbar-watch "$(BIN)/printbar-watch"
	install -d "$(UNITDIR)"
	sed 's|@BIN@|$(BIN)|' printbar-watch.service > "$(UNITDIR)/printbar-watch.service"
	chmod 644 "$(UNITDIR)/printbar-watch.service"

uninstall:
	rm -f "$(BIN)/printbar" "$(BIN)/printbar-watch" "$(UNITDIR)/printbar-watch.service"

# Omarchy shell (Quickshell) plugin. Symlinked into the plugins dir; note the
# shell does NOT watch files under a symlinked plugin dir — after editing, run
# `omarchy restart shell` to reload. Needs the printbar binary on
# PATH (make install PREFIX=~/.local).
OMARCHY_PLUGIN := $(HOME)/.config/omarchy/plugins/mryll.printbar

install-omarchy:
	@command -v printbar >/dev/null 2>&1 || \
		echo "warning: printbar is not on PATH — the widget will show an explicit error until the binary is installed (make install PREFIX=~/.local)"
	install -d "$(HOME)/.config/omarchy/plugins"
	ln -sfT "$(abspath .)" "$(OMARCHY_PLUGIN)"
	@echo 'Plugin linked. Add { "id": "mryll.printbar" } to a bar layout section in ~/.config/omarchy/shell.json'
	@echo '(editing plugin files needs: omarchy restart shell — a rescan does not recompile QML)'

uninstall-omarchy:
	rm -f "$(OMARCHY_PLUGIN)"

.PHONY: build install uninstall install-omarchy uninstall-omarchy
