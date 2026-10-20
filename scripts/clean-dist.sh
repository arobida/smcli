#!/bin/bash
# Removes old dist/ builds, keeping the pair the dist/smcli and dist/smcli-service symlinks point to (shell hooks may
# use them) and the symlinks themselves. Usage: clean-dist.sh <dist directory>
set -u
dist=${1:?usage: clean-dist.sh <dist directory>}
keep_cli=$(readlink "$dist/smcli" 2>/dev/null)
keep_service=$(readlink "$dist/smcli-service" 2>/dev/null)
for f in "$dist"/smcli-*; do
    [[ -e $f || -L $f ]] || continue
    name=$(basename "$f")
    [[ $name == smcli-service || $name == "$keep_cli" || $name == "$keep_service" ]] && continue
    rm -f "$f"
done
