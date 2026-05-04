BINARY  := lore
FEATURES := --all-features

# Stricter warnings for CI / check target
export RUSTFLAGS ?= -W dead_code -W unused_imports -W unused_variables -W unused_mut

.DEFAULT_GOAL := help

.PHONY: build install update fmt fmt-fix lint-conventions lint test-quick test fuzz doc generate-docs check-docs check ci-local setup clean help

# Prerequisites:
#   ocr feature: cmake must be installed (used by kreuzberg-tesseract)

## build: Build the binary (release, all features)
build:
	cargo build --release --package lore-cli $(FEATURES)

## install: Install the binary via cargo install
install:
	cargo install --force --path cli $(FEATURES)

## update: Update all Cargo dependencies
update:
	cargo update

## fmt: Check formatting (fails on diff)
fmt:
	cargo fmt -- --check

## fmt-fix: Fix formatting (destructive)
fmt-fix:
	cargo fmt

## lint-conventions: Check code conventions (unwrap, import order, pub visibility)
lint-conventions:
	scripts/lint-conventions.sh

## lint: Run clippy (all feature combos)
lint:
	cargo clippy --all-targets $(FEATURES) -- -D warnings
	cargo clippy --all-targets --no-default-features -- -D warnings
	cargo clippy --all-targets -- -D warnings

## test-quick: Run tests (default features only)
test-quick:
	cargo test

## test: Run tests (all feature combos)
test:
	cargo test $(FEATURES)
	cargo test --no-default-features
	cargo test

## fuzz: Run all fuzz targets for 60 seconds each (requires cargo-fuzz and nightly)
fuzz:
	@command -v rustup >/dev/null 2>&1 || { echo "error: rustup is required for fuzz (cargo +nightly). Install from https://rustup.rs"; exit 1; }
	@rustup run nightly cargo --version >/dev/null 2>&1 || { echo "error: nightly toolchain not installed. Run: rustup toolchain install nightly"; exit 1; }
	@rustup run nightly cargo fuzz --version >/dev/null 2>&1 || { echo "error: cargo-fuzz not installed. Run: cargo install cargo-fuzz"; exit 1; }
	$(eval NIGHTLY_BIN := $(shell rustup run nightly rustc --print sysroot)/bin)
	PATH="$(NIGHTLY_BIN):$(PATH)" cargo fuzz run fuzz_sanitize_query -- -max_total_time=60
	PATH="$(NIGHTLY_BIN):$(PATH)" cargo fuzz run fuzz_visible_width -- -max_total_time=60
	PATH="$(NIGHTLY_BIN):$(PATH)" cargo fuzz run fuzz_config_parse -- -max_total_time=60

## doc: Build docs (fails on warnings)
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps $(FEATURES)

## generate-docs: Regenerate doc tables from code annotations
generate-docs:
	cargo run --package xtask -- generate-docs

## check-docs: Verify doc tables are up to date
check-docs: generate-docs
	@git diff --exit-code -- docs/ skills/ \
		|| { echo "Doc tables are stale. Run: cargo xtask generate-docs"; exit 1; }

## check: Run all quality gates (fmt lint-conventions lint doc test check-docs deny)
check: fmt lint-conventions lint doc test check-docs
	@if command -v cargo-deny >/dev/null 2>&1; then \
		cargo deny check; \
	else \
		echo "note: cargo-deny not installed; skipping license/advisory check (run: cargo install cargo-deny)"; \
	fi

## ci-local: Run CI workflow locally via act (ubuntu matrix only)
ci-local:
	@command -v act >/dev/null 2>&1 || { echo "error: act not installed. Run: brew install act"; exit 1; }
	act push -W .github/workflows/ci.yml

## setup: Install git hooks (run once after cloning)
setup:
	scripts/install-hooks.sh

## clean: Remove build artifacts
clean:
	cargo clean

## help: Show this help
help:
	@echo "lore Makefile"
	@echo ""
	@grep -E '^## ' $(MAKEFILE_LIST) | sed 's/## //' | column -t -s ':'
