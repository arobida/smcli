SWIFT ?= /usr/bin/swift
SWIFTC ?= $(SWIFT)c
PYTHON ?= python3
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh test clean run baseline ranking fff fff-universal x86_64 fff-build fff-smoke rs-build rs-test rs-ranking rs-linux

# smcli-service links the pinned fff statically, so it must be built before anything that links the service.
# Cached after the first run (cargo no-ops when nothing changed).
fff:
	bash scripts/build-fff.sh

# Universal (arm64 + x86_64) fff archive for cross-architecture builds; needs `rustup target add x86_64-apple-darwin`.
fff-universal:
	FFF_ARCHS="arm64 x86_64" bash scripts/build-fff.sh

# Cross-compiles the Intel macOS binaries into .build/x86_64 (run them with `arch -x86_64`), then restores the native fff.
x86_64:
	FFF_ARCHS=x86_64 bash scripts/build-fff.sh
	$(SWIFT) build -c release --arch x86_64 --scratch-path .build/x86_64-scratch; status=$$?; bash scripts/build-fff.sh; exit $$status

# Debug build.
build:
	cargo build --workspace

# Optimized build, copied to dist/ as a timestamped binary plus a dist/smcli symlink to the latest.
release:
	cargo build --workspace --release
	@mkdir -p $(DIST)
	cp target/release/smcli $(DIST)/smcli-$(STAMP)
	cp target/release/smcli-service $(DIST)/smcli-service-$(STAMP)
	ln -sf smcli-$(STAMP) $(DIST)/smcli
	ln -sf smcli-service-$(STAMP) $(DIST)/smcli-service
	@echo "Built $(DIST)/smcli-$(STAMP) (latest: $(DIST)/smcli)"
	@echo ""
	@echo "Try it in this shell (loads the zsh hooks from the new build):"
	@echo "  eval \"\$$(\"$(CURDIR)/$(DIST)/smcli\" shell-init zsh)\""
	@echo "  \"$(CURDIR)/$(DIST)/smcli\" history"

# Clean rebuild from scratch, for testing. Sequential even under -j: clean must finish before the build starts.
fresh:
	$(MAKE) clean
	$(MAKE) release

test: fff
	bash scripts/test.sh

# Scores suggest and search ranking on the fixed query set (MRR, top-1, top-3) and lists the misses.
# Fails when the tests fail; the filtered report is printed either way.
ranking: fff
	@log=$$(mktemp); $(SWIFT) test --filter RankingQualityTests > "$$log" 2>&1; status=$$?; \
	grep -E "^ranking|^  (suggest|search|wanted)|^      why|error:|✘|Test run" "$$log"; rm -f "$$log"; exit $$status

# Black-box baseline of the current CLI and generated hooks; never reads real history.
baseline: release
	SWIFT="$(SWIFT)" $(PYTHON) scripts/bench-baseline.py --binary "$(CURDIR)/$(DIST)/smcli" $(BENCH_ARGS)

# Isolated C API smoke test of the wrapper's ownership rules, outside the package.
fff-build:
	bash scripts/build-fff.sh
	$(SWIFTC) -swift-version 6 -O -I .build/fff/include -L .build/fff/target/release \
		-lfff_c -Xlinker -rpath -Xlinker "$(CURDIR)/.build/fff/target/release" \
		tools/fff/smoke.swift -o .build/fff/smoke

fff-smoke: fff-build
	.build/fff/smoke

# Rust port (.plans/rust-port.md): Cargo workspace in crates/, output in target/.
rs-build:
	cargo build --workspace

rs-test:
	cargo test --workspace

# The Rust port's ranking scores on the same fixed query set as `make ranking`.
rs-ranking:
	cargo test --release -p smcli-search --test ranking -- --nocapture

# Builds and tests the Rust workspace on Linux in Docker (needs a running daemon); Linux output stays in a volume.
rs-linux:
	docker run --rm -v "$(CURDIR)":/src -w /src -v smcli-cargo:/usr/local/cargo/registry -v smcli-target:/target \
		-e CARGO_TARGET_DIR=/target rust:latest sh -c 'cargo build --workspace && cargo test --workspace && /target/debug/smcli-service'

run:
	cargo run -p smcli -- $(ARGS)

# Removes build output, keeping the newest dist/ CLI and service pair and their symlinks (shell hooks may use them).
clean:
	cargo clean
	$(SWIFT) package clean
	rm -rf .build/release
	@bash scripts/clean-dist.sh $(DIST)
