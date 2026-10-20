SWIFT ?= /usr/bin/swift
SWIFTC ?= $(SWIFT)c
PYTHON ?= python3
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh test clean run baseline ranking fff fff-universal x86_64 fff-build fff-smoke

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
build: fff
	$(SWIFT) build

# Optimized build, copied to dist/ as a timestamped binary plus a dist/smcli symlink to the latest.
release: fff
	$(SWIFT) build -c release
	@mkdir -p $(DIST)
	cp "$$($(SWIFT) build -c release --show-bin-path)/smcli" $(DIST)/smcli-$(STAMP)
	cp "$$($(SWIFT) build -c release --show-bin-path)/smcli-service" $(DIST)/smcli-service-$(STAMP)
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
ranking: fff
	$(SWIFT) test --filter RankingQualityTests 2>&1 | grep -E "^ranking|^  (suggest|search|wanted)|^      why|error:|✘|Test run" || true

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

run: build
	$(SWIFT) run smcli $(ARGS)

# Removes build output, keeping the newest dist/ binary that dist/smcli points to (shell hooks may use it).
clean:
	$(SWIFT) package clean
	rm -rf .build/release
	@latest=$$(readlink $(DIST)/smcli 2>/dev/null); \
	for f in $(DIST)/smcli-*; do [ -e "$$f" ] && [ "$$(basename $$f)" != "$$latest" ] && rm -f "$$f"; done; true
