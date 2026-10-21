# fff transitive license audit

Audited 2026-10-19 for the pinned fff `v0.11.0` (`95fd777c`), with default features
(ripgrep, vendored libgit2, no `zlob`, no https/ssh). The Rust workspace now links
`fff-search` directly into `smcli-service`, without the former C wrapper. The figures
below record the original audit; regenerate `THIRD_PARTY_NOTICES.md` for the current
locked dependency set. This is not legal advice.

**Verdict:** shippable under permissive terms, with notice obligations. Nothing is
GPL-incompatible with linking, but three items need action before distribution (see the end).

## How it was checked

```sh
cargo tree --locked -p fff-search \
    -e normal --prefix none --target <triple> -f '{p} {l}'
```

`-e normal` follows only linked dependencies (no build or dev dependencies); proc-macros still
appear and are counted, which overstates the shipped set slightly. The same was repeated with
`cargo metadata --filter-platform` for `aarch64-apple-darwin` (100 crates), `x86_64-apple-darwin`
(101) and `x86_64-unknown-linux-gnu` (102). Bundled C sources inside `-sys` crates were read
directly, since Cargo's `license` field doesn't cover them. The original audit also
confirmed LMDB, PCRE2 and libgit2 code in the former C-wrapper binary using `strings`.

## Rust crates

| License expression | Crates | Notes |
|---|---:|---|
| MIT OR Apache-2.0 (incl. `MIT/Apache-2.0`) | ~61 | |
| MIT | 20 | fff's own crates, heed, tracing, neo_frizbee, bincode, … |
| Unlicense OR MIT | 7 | BurntSushi crates (memchr, ignore, globset, walkdir, …) |
| Zlib OR Apache-2.0 OR MIT | 2 | objc2-core-* (macOS only) |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 | zerocopy |
| BSD-2-Clause | 1 | arrayref |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 | constant_time_eq |
| CC0-1.0 OR Apache-2.0 (± LLVM exception) | 1 | blake3 |
| CC0-1.0 | 1 | notify 9.0.0-rc.4 (a release candidate, not a license issue) |
| BSL-1.0 | 1 | xxhash-rust; no notice needed in binaries |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 | unicode-ident; Unicode notice required |
| MPL-2.0 | 1 | option-ext; file-level copyleft, unmodified use is fine |
| ISC | 2 | inotify, inotify-sys (Linux only) |
| Apache-2.0 | 1 | lmdb-master-sys (Rust binding; C code below) |

No crate lacks a license, and none is GPL/LGPL/AGPL at the Rust level.

## Bundled C code

| Component | Via | License | Obligation |
|---|---|---|---|
| libgit2 1.9.6 | libgit2-sys `vendored` | GPL-2.0 **with linking exception** | Linking into a binary is unrestricted; modifying libgit2 itself is GPL. |
| xdiff (inside libgit2) | libgit2-sys, always compiled | LGPL-2.1-or-later | Static link: allow users to relink (ship object files or keep smcli's source available) and include the LGPL text. |
| PCRE2 (inside libgit2) | libgit2-sys, always compiled | BSD-3-Clause | Reproduce notice. |
| llhttp, SHA1DC (inside libgit2) | libgit2-sys | MIT | Reproduce notice. |
| LMDB | lmdb-master-sys | OpenLDAP Public License 2.8 | Reproduce copyright and license in binary distributions. |
| zlib | libz-sys | Zlib | macOS links the system `/usr/lib/libz.1.dylib`; nothing bundled there. Recheck on Linux. |

libgit2's `COPYING` is the authoritative list of its bundled pieces and their texts.

## Required before shipping a binary that links fff

1. **Choose and add a license for smcli.** The repository has no `LICENSE` file; the xdiff
   (LGPL) relinking route and every notice obligation assume smcli's own terms are known.
2. **Ship a third-party notices file** with the service binary: MIT/BSD/ISC/Zlib/Unicode
   notices for the crates above, plus libgit2's `COPYING`, LMDB's `LICENSE`, and the
   MPL-2.0 source pointer for option-ext. A generator such as `cargo about` can produce it
   from the locked graph; rerun it whenever the fff pin changes.
3. **xdiff:** either keep smcli's source public (simplest LGPL compliance for a static link),
   or ship relinkable object files.

Re-audit whenever the workspace's `fff-search` dependency changes its pin or feature set; enabling `zlob`,
`https` or `ssh` adds Zig-built code, OpenSSL or libssh2.
