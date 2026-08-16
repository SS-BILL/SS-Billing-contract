CONTRACT_DIR := subscription-engine
WASM := $(CONTRACT_DIR)/target/wasm32-unknown-unknown/release/subscription_engine.wasm
NETWORK ?= testnet

.PHONY: all build test fmt fmt-check lint check optimize clean deploy

all: check

## Compile the contract to WASM
build:
	cd $(CONTRACT_DIR) && cargo build --target wasm32-unknown-unknown --release

## Run the test suite
test:
	cd $(CONTRACT_DIR) && cargo test

fmt:
	cd $(CONTRACT_DIR) && cargo fmt

fmt-check:
	cd $(CONTRACT_DIR) && cargo fmt --check

lint:
	cd $(CONTRACT_DIR) && cargo clippy --all-targets -- -D warnings

## Everything CI runs, in the order CI runs it
check: fmt-check lint test build

## Strip and shrink the WASM before deployment
optimize: build
	stellar contract optimize --wasm $(WASM)

## Deploy to $(NETWORK). Requires SOURCE to name a configured identity.
deploy: optimize
	@test -n "$(SOURCE)" || (echo "SOURCE is required, e.g. make deploy SOURCE=alice" && exit 1)
	stellar contract deploy \
		--wasm $(WASM) \
		--source $(SOURCE) \
		--network $(NETWORK)

clean:
	cd $(CONTRACT_DIR) && cargo clean
