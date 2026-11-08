#!/usr/bin/env bash
# Fetch the two vendored dependencies the Rust crates build against.
#   vendor/silverscript          kaspanet/silverscript at a pinned commit, plus tools/silverscript-443d519.patch
#                                (kaspa-hashes without its assembly, a crates.io patch, and an optional zk feature)
#   vendor/kaspa-hashes-patched  kaspa-hashes 2.1.0 whose build.rs honours the no-asm feature (shipped in this repo)
# Run once from the repository root: bash tools/fetch_vendor.sh
set -euo pipefail
cd "$(dirname "$0")/.."
REV=443d519e6688
if [ ! -d vendor/silverscript/.git ]; then
  git clone https://github.com/kaspanet/silverscript.git vendor/silverscript
fi
git -C vendor/silverscript fetch --quiet origin
git -C vendor/silverscript checkout --quiet "$REV"
if git -C vendor/silverscript apply --check ../../tools/silverscript-443d519.patch 2>/dev/null; then
  git -C vendor/silverscript apply ../../tools/silverscript-443d519.patch
  echo "silverscript at $REV with the patch applied"
else
  echo "silverscript at $REV (patch already applied)"
fi
test -f vendor/kaspa-hashes-patched/Cargo.toml || { echo "vendor/kaspa-hashes-patched is missing from this checkout" >&2; exit 1; }
echo "vendor ready"
