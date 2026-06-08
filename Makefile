PREFIX  ?= /usr/local
BIN     := $(PREFIX)/bin
# Local installs: ~/.config/systemd/user always works for `systemctl --user`.
# AUR/system packaging overrides UNITDIR=/usr/lib/systemd/user.
UNITDIR ?= $(HOME)/.config/systemd/user

build:
	cargo build --release

install: build
	install -Dm755 target/release/printbar $(BIN)/printbar
	install -Dm755 printbar-watch $(BIN)/printbar-watch
	install -d $(UNITDIR)
	sed 's|@BIN@|$(BIN)|' printbar-watch.service > $(UNITDIR)/printbar-watch.service
	chmod 644 $(UNITDIR)/printbar-watch.service

uninstall:
	rm -f $(BIN)/printbar $(BIN)/printbar-watch $(UNITDIR)/printbar-watch.service

.PHONY: build install uninstall
