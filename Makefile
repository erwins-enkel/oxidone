# Build & install DX. Every target is phony; `make` with no args lists them.
# `make install` builds and drops oxidone on your PATH; `make update` pulls first.
# `make gate` is the single source of truth for the CI gate (see .githooks and ci.yml).

PREFIX  ?= $(HOME)/.local
PROFILE ?= release
# `--profile dev` emits into target/debug, not target/dev — map, don't substitute.
PROFILE_DIR := $(if $(filter dev,$(PROFILE)),debug,$(PROFILE))
# Honor CARGO_TARGET_DIR — hardcoding `target/` would break silently under it.
BIN := $(or $(CARGO_TARGET_DIR),target)/$(PROFILE_DIR)/oxidone

.DEFAULT_GOAL := help
.PHONY: help check build install uninstall update gate config dev-tools

help:       ## list targets
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) \
	  | awk -F':.*##' '{printf "%-12s %s\n", $$1, $$2}'

check:      ## fast type-check — the inner loop
	cargo check --all-targets --all-features

build:      ## compile the binary (PROFILE=release by default)
	cargo build --profile $(PROFILE) --locked

install: build  ## build, then install to PREFIX/bin (default ~/.local/bin)
	install -d "$(PREFIX)/bin"
	install -m755 "$(BIN)" "$(PREFIX)/bin/oxidone"
	@echo "installed $(PREFIX)/bin/oxidone"

uninstall:  ## remove the installed binary
	rm -f "$(PREFIX)/bin/oxidone"

update:     ## git pull, then reinstall
	git pull --ff-only
	$(MAKE) install

gate:       ## fmt · clippy · test · unused deps (what CI runs)
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features -- -D warnings
	cargo test --all-features
	cargo machete

config: build  ## write a starter config.toml if none exists
	@cfg="$$("$(BIN)" --print-config-path)"; \
	[ -n "$$cfg" ] || { echo "no home dir; cannot locate config" >&2; exit 1; }; \
	if [ -e "$$cfg" ]; then echo "config exists, leaving it alone: $$cfg"; \
	else install -d "$$(dirname "$$cfg")"; \
	     install -m644 config.example.toml "$$cfg"; \
	     echo "wrote $$cfg"; fi

dev-tools:  ## install the tools gate needs (cargo-machete)
	# Lands in cargo's bin dir; `cargo machete` resolves cargo-* subcommands
	# from there independently of PATH, so it works even if that dir is unlisted.
	cargo install cargo-machete --locked
