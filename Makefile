# veloasm workspace tasks
#
# Default budget for the counter-spa gzipped wasm (kB). Keep DX additions
# from silently bloating the shipped binary — raise only deliberately.
#
# `make check` is the CI gate (check + test + size): always green on a clean
# checkout. `make fmt-check` and `make lint` are *informational* diagnostics;
# the codebase carries pre-existing rustfmt / clippy debt, so they do NOT gate
# CI. See issue/debt note before running them against a clean tree.

BUDGET_KB ?= 60

.PHONY: all check test build docs size fmt-check lint browser-test

all: check

## Workspace compiles for wasm without warnings/errors
check-wasm:
	cargo check --workspace --all-targets --target wasm32-unknown-unknown

## Run host tests
test:
	cargo test --workspace

## CI gate: wasm check + tests (stays green on a clean checkout)
check: check-wasm test

## Build workspace for wasm (debug) + the size-benchmark crate (release)
build:
	cargo build --workspace --target wasm32-unknown-unknown
	cargo build -p counter-spa --release --target wasm32-unknown-unknown

## Build the mdBook docs
docs:
	mdbook build docs

## Drive the shared-AST client in a real browser and assert on the results
## (the page runs its own assertions against the live DOM; this reads them back)
browser-test:
	pwsh -NoProfile -File scripts/browser-test.ps1


## Assert the counter-spa gzipped wasm stays under the size budget
size:
	cd examples/counter-spa && trunk build --release
	python3 scripts/check-wasm-size.py $(BUDGET_KB)

## Informational: rustfmt check (pre-existing diffs; not gating)
fmt-check:
	cargo fmt --all -- --check

## Informational: clippy (pre-existing warnings; not gating)
lint:
	cargo clippy --workspace --all-targets
