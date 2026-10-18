SWIFT ?= /usr/bin/swift
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh test clean run

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
	scripts/test.sh

run: build
	$(SWIFT) run smcli $(ARGS)

clean:
	$(SWIFT) package clean
	rm -rf .build/release
