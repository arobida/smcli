SWIFT ?= /usr/bin/swift
SWIFTC ?= $(SWIFT)c
PYTHON ?= python3
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh test clean run baseline fff-build fff-smoke

# Debug build.
build:
	$(SWIFT) build

# Optimized build, copied to dist/ as a timestamped binary plus a dist/smcli symlink to the latest.
release:
	$(SWIFT) build -c release
	@mkdir -p $(DIST)
	cp "$$($(SWIFT) build -c release --show-bin-path)/smcli" $(DIST)/smcli-$(STAMP)
	ln -sf smcli-$(STAMP) $(DIST)/smcli
	@echo "Built $(DIST)/smcli-$(STAMP) (latest: $(DIST)/smcli)"
	@echo ""
	@echo "Try it in this shell (loads the zsh hooks from the new build):"
	@echo "  eval \"\$$(\"$(CURDIR)/$(DIST)/smcli\" shell-init zsh)\""
	@echo "  \"$(CURDIR)/$(DIST)/smcli\" history"

# Clean rebuild from scratch, for testing.
fresh: clean release

test:
	bash scripts/test.sh

# Black-box baseline of the current CLI and generated hooks; never reads real history.
baseline: release
	SWIFT="$(SWIFT)" $(PYTHON) scripts/bench-baseline.py --binary "$(CURDIR)/$(DIST)/smcli" $(BENCH_ARGS)

# Optional, isolated C API experiment. Neither smcli nor its tests link fff.
fff-build:
	bash scripts/build-fff.sh
	$(SWIFTC) -swift-version 6 -O -I .build/fff/include -L .build/fff/target/release \
		-lfff_c -Xlinker -rpath -Xlinker "$(CURDIR)/.build/fff/target/release" \
		tools/fff/smoke.swift -o .build/fff/smoke

fff-smoke: fff-build
	.build/fff/smoke

run: build
	$(SWIFT) run smcli $(ARGS)

clean:
	$(SWIFT) package clean
	rm -rf .build/release
