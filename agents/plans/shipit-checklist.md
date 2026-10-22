# Ship-it checklist: licensing

Every box must be checked before smcli ships (any public binary, tap or download).

## smcli's own license

- [ ] Choose smcli's license and add `LICENSE` at the repo root.
- [ ] Check that the chosen license is compatible with everything below. LGPL-2.1+ (xdiff) is the constraint.
- [ ] Put the license name in `Cargo.toml`/README and in `smcli --version` or `smcli licenses` output.

## xdiff (LGPL-2.1+, inside libgit2, statically linked into `smcli-service`)

- [ ] Decide how to comply. Pick one:
  - [ ] Publish smcli's source under a compatible license, **or**
  - [ ] Ship relinkable object files for `smcli-service` (plus build instructions), **or**
  - [ ] Remove the dependency: build fff/libgit2 without xdiff, or link libgit2 dynamically.
- [ ] Include the LGPL-2.1 text and xdiff's copyright notice in the distribution.
- [ ] Provide a written offer or a link to the exact libgit2/xdiff source used (pinned version).

## Third-party notices

- [ ] Fetch license texts for the 10 Rust crates that ship none (listed in `THIRD_PARTY_NOTICES.md`) and add them.
- [ ] Re-run `scripts/gen-notices.py` against the release fff build and commit the regenerated `THIRD_PARTY_NOTICES.md`.
- [ ] Confirm the notices cover everything in each shipped binary: fff and its crates, libgit2, LMDB, TalaDB, the remaining Rust crates, and the bundled Potion model (`potion-code-16M-v2`: check its model license and attribution).
- [ ] Ship `THIRD_PARTY_NOTICES.md` with every artifact: tarball, Homebrew formula, and universal and x86_64 builds.
- [ ] Re-check `../reference/fff-licenses.md` against the pinned fff version. Update the pin if it moved.

## Release gate

- [ ] Every architecture you release (arm64, x86_64, universal) is built from the same pinned fff and libgit2 as the notices.
- [ ] Someone other than the author has reviewed this list.
