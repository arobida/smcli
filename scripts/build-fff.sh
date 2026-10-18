#!/bin/bash
# Optional dependency experiment, not part of normal builds.
set -euo pipefail
cd "$(dirname "$0")/.."

# fff v0.11.0, peeled release commit. Never download a moving branch or binary.
revision=95fd777c2529fc7b4d7572dabff64cc07268f2c5
sha256=d2d96d7cc472571b23476b502a0b96f974f0e968c0f290b22078cc300bea6c64
base="$PWD/.build/fff"
mkdir -p "$base"

if [[ ! -f "$base/source/.smcli-revision" || $(cat "$base/source/.smcli-revision") != "$revision" ]]; then
    staging="$(mktemp -d "$base/download.XXXXXX")"
    trap 'rm -rf "$staging"' EXIT
    curl --fail --location --silent --show-error --connect-timeout 15 --max-time 120 \
        "https://codeload.github.com/dmtrKovalenko/fff/tar.gz/$revision" -o "$staging/source.tar.gz"
    printf '%s  %s\n' "$sha256" "$staging/source.tar.gz" | shasum -a 256 --check
    mkdir "$staging/source"
    tar -xzf "$staging/source.tar.gz" -C "$staging/source" --strip-components=1
    printf '%s\n' "$revision" > "$staging/source/.smcli-revision"
    rm -rf "$base/source"
    mv "$staging/source" "$base/source"
fi

# Stay at the project root: use the caller's Rust toolchain rather than letting
# upstream's rust-toolchain.toml update "stable" and install optional components.
# The default ripgrep feature needs no Zig. Cargo.lock pins transitive dependencies.
cargo build --manifest-path "$base/source/Cargo.toml" --target-dir "$base/target" \
    --locked --release -p fff-c
mkdir -p "$base/include"
cp "$base/source/crates/fff-c/include/fff.h" "$base/include/fff.h"
cp tools/fff/module.modulemap "$base/include/module.modulemap"
cp "$base/source/LICENSE" "$base/LICENSE"
