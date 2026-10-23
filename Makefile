PYTHON ?= python3
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh test clean run baseline ranking linux docs notices

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

test:
	bash scripts/test.sh

# Scores suggest and search ranking on the fixed query set (MRR, top-1, top-3) and lists the misses.
ranking:
	cargo test --release -p smcli-search --test ranking -- --nocapture

# Black-box baseline of the current CLI and generated hooks; never reads real history.
baseline: release
	$(PYTHON) scripts/bench-baseline.py --binary "$(CURDIR)/$(DIST)/smcli" $(BENCH_ARGS)

# Builds and runs the full test suite on Linux in Docker (needs a running daemon); Linux output stays in a volume.
linux:
	docker run --rm -v "$(CURDIR)":/src -w /src -v smcli-cargo:/usr/local/cargo/registry -v smcli-target:/target \
		-e CARGO_TARGET_DIR=/target rust:latest sh -c 'apt-get update -qq && apt-get install -y -qq zsh fish python3 > /dev/null && git config --global --add safe.directory "*" && bash scripts/test.sh'

# Formats every hand-written markdown doc with iwe. Generated docs are skipped: regenerate them instead.
GENERATED_DOCS := THIRD_PARTY_NOTICES
docs:
	iwe normalize $$(iwe find --limit 0 -f keys | grep -vxF $(addprefix -e ,$(GENERATED_DOCS)) | sed 's/^/-k /')

notices:
	$(PYTHON) scripts/gen-notices.py

run:
	cargo run -p smcli -- $(ARGS)

# Removes build output, keeping the newest dist/ CLI and service pair and their symlinks (shell hooks may use them).
clean:
	cargo clean
	@bash scripts/clean-dist.sh $(DIST)
