#!/bin/bash
# Builds the pinned fff C library and publishes it where Package.swift links it: .build/fff/lib/libfff_c.a.
# By default that is a thin archive for this machine's architecture. On macOS, FFF_ARCHS="arm64 x86_64" (or either
# one; `make fff-universal`) cross-builds each with Cargo's --target (the rustup target must be installed:
# `rustup target add x86_64-apple-darwin`) and joins them with lipo, so SwiftPM's --arch/--triple can pick the slice.
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

# Match Package.swift's macOS 14 floor, or every object in the static library warns at link time.
[[ $(uname) != Darwin ]] || export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

# Stay at the project root: use the caller's Rust toolchain rather than letting
# upstream's rust-toolchain.toml update "stable" and install optional components.
# The default ripgrep feature needs no Zig. Cargo.lock pins transitive dependencies.
mkdir -p "$base/lib"
if [[ $(uname) == Darwin && -n "${FFF_ARCHS:-}" ]]; then
    libs=()
    for arch in $FFF_ARCHS; do
        case "$arch" in
            arm64) triple=aarch64-apple-darwin ;;
            x86_64) triple=x86_64-apple-darwin ;;
            *) echo "FFF_ARCHS: unknown architecture $arch (use arm64 or x86_64)" >&2; exit 1 ;;
        esac
        cargo build --manifest-path "$base/source/Cargo.toml" --target-dir "$base/target" \
            --locked --release -p fff-c --target "$triple"
        libs+=("$base/target/$triple/release/libfff_c.a")
    done
    lipo -create "${libs[@]}" -output "$base/lib/libfff_c.a"
else
    cargo build --manifest-path "$base/source/Cargo.toml" --target-dir "$base/target" \
        --locked --release -p fff-c
    cp "$base/target/release/libfff_c.a" "$base/lib/libfff_c.a"
fi
mkdir -p "$base/include"
cp "$base/source/crates/fff-c/include/fff.h" "$base/include/fff.h"
cp tools/fff/module.modulemap "$base/include/module.modulemap"
cp "$base/source/LICENSE" "$base/LICENSE"
