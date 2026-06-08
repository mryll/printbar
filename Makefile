PREFIX ?= /usr/local
BIN := $(PREFIX)/bin

build:
	cargo build --release

install: build
	install -Dm755 target/release/printbar $(BIN)/printbar

uninstall:
	rm -f $(BIN)/printbar

.PHONY: build install uninstall
