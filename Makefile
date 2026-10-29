PYTHON ?= python3
DIST := dist
STAMP := $(shell date +%Y%m%d-%H%M%S)-$(shell git rev-parse --short HEAD 2>/dev/null || echo nogit)

.PHONY: build release fresh install uninstall test test-rust test-cli test-picker test-shells test-service test-install clean run baseline ranking linux docs notices

PKG ?=
FILTER ?=

# Debug build.
build:
	cargo build --workspace

# Optimized build, copied to dist/ as a timestamped binary plus a dist/smcli symlink to the latest.
release:
	cargo build --workspace --release
	@mkdir -p $(DIST)
	cp $${CARGO_TARGET_DIR:-target}/release/smcli $(DIST)/smcli-$(STAMP)
	cp $${CARGO_TARGET_DIR:-target}/release/smcli-service $(DIST)/smcli-service-$(STAMP)
	ln -sf smcli-$(STAMP) $(DIST)/smcli
	ln -sf smcli-service-$(STAMP) $(DIST)/smcli-service
	rm -rf $(DIST)/potion-shell-16m && cp -R Models/potion-shell-16m $(DIST)/potion-shell-16m
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

# One slice of the gate, for the edit/test loop. Each assumes `make build` output.
test-rust:
	cargo test $(if $(PKG),-p $(PKG),--workspace) $(FILTER)

# Black-box CLI smoke: usage, history, search, init --keys, shell-init, clean-dist, @-completion.
test-cli: build
	bash scripts/test-cli.sh "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)/smcli"

# The interactive pickers (files and aliases), for picker/menu/UI changes.
test-picker: build
	$(PYTHON) scripts/test-file-picker.py "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)/smcli"
	$(PYTHON) scripts/test-alias-picker.py "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)/smcli"

test-shells: build
	$(PYTHON) scripts/test-shells.py "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)/smcli"

test-service: build
	$(PYTHON) scripts/test-service.py "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)/smcli"

# Installs the release build on this machine: binaries and model to ~/.local/bin, shell hooks, keys, service.
# Pass installer flags with ARGS, e.g. make install ARGS="--shells all --yes".
install: release
	bash scripts/install.sh --from $(DIST) $(ARGS)

# Uninstalls the release build on this machine, using the installed uninstaller when present.
# Pass uninstaller flags with ARGS, e.g. make uninstall ARGS="--yes".
uninstall:
	if [ -x $(HOME)/.local/bin/smcli-uninstall ]; then $(HOME)/.local/bin/smcli-uninstall $(ARGS); else bash scripts/uninstall.sh $(ARGS); fi

test-install: build
	bash scripts/test-install.sh "$$(cd "$${CARGO_TARGET_DIR:-target}/debug" && pwd)"

# Scores suggest and search ranking on the fixed query set (MRR, top-1, top-3) and lists the misses.
ranking:
	cargo test --release -p smcli-search --test ranking -- --nocapture

# Black-box baseline of the current CLI and generated hooks; never reads real history.
baseline:
	cargo build --workspace --release
	$(PYTHON) scripts/bench-baseline.py --binary "$$(cd "$${CARGO_TARGET_DIR:-target}/release" && pwd)/smcli" $(BENCH_ARGS)

# Builds and runs the full test suite on Linux in Docker (needs a running daemon); Linux output stays in a volume.
linux:
	docker run --rm -v "$(CURDIR)":/src -w /src -v smcli-cargo:/usr/local/cargo/registry -v smcli-cargo-git:/usr/local/cargo/git -v smcli-target:/target \
		-e CARGO_TARGET_DIR=/target rust:1.99 sh -c 'apt-get update -qq && apt-get install -y -qq zsh fish python3 sqlite3 git > /dev/null && git config --global --add safe.directory "*" && bash scripts/test.sh'

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
