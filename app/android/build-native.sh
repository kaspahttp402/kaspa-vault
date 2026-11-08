#!/usr/bin/env bash
# Builds the wallet engine (testnet-client as a cdylib) for the phone and drops libvault_core.so into jniLibs/arm64-v8a/,
# where build.sh picks it up. Separate from build.sh on purpose: the APK build never runs cargo.
#   Profile `phone` (testnet-client/Cargo.toml): release + opt-level z + lto + strip.
#   cargo ndk 4.1.2: --platform is the Android API level (-p would mean the cargo package and panics "unknown package: 33").
#   CARGO_TARGET_DIR is a Windows path because cargo is a Windows exe; it keeps the phone build off vendor/silverscript/target's lock.
set -e
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
SDK="$LOCALAPPDATA/Android/Sdk"
if [ -z "${ANDROID_NDK_HOME:-}" ]; then
  NEWEST="$(ls "$SDK/ndk" 2>/dev/null | sort -V | tail -1)"
  [ -n "$NEWEST" ] || { echo "No NDK under $SDK/ndk: install one from Android Studio's SDK Manager (SDK Tools > NDK)." >&2; exit 1; }
  export ANDROID_NDK_HOME="$SDK/ndk/$NEWEST"
fi
export CARGO_TARGET_DIR='C:\tmp\kv-android-target'
OUT="$(pwd)/jniLibs"
OUT_WIN="$(cygpath -w "$OUT" 2>/dev/null || echo "$OUT")"
command -v cargo >/dev/null || { echo "cargo is not on PATH (looked in $HOME/.cargo/bin)." >&2; exit 1; }
echo "NDK: $ANDROID_NDK_HOME"
echo "target dir: $CARGO_TARGET_DIR"
echo "output: $OUT"
CRATE="${KV_CRATE_DIR:-../../testnet-client}"   # the crate beside app/; KV_CRATE_DIR overrides it for a build from a copy of app/
[ -d "$CRATE" ] || { echo "The engine crate is not at $CRATE (set KV_CRATE_DIR to testnet-client)." >&2; exit 1; }
(cd "$CRATE" && cargo ndk -t arm64-v8a --platform 33 -o "$OUT_WIN" build --profile phone --no-default-features)
SO="$OUT/arm64-v8a/libvault_core.so"
# cargo ndk also copies every dependency that happens to be a cdylib (workflow-*, kaspa_wrpc_client): they are already linked
# into libvault_core.so, and build.sh packs every .so it finds, so only ours may stay.
STRAY=0
for f in "$OUT"/arm64-v8a/*.so; do
  [ -e "$f" ] || continue
  [ "$(basename "$f")" = libvault_core.so ] && continue
  rm -f "$f"; STRAY=$((STRAY + 1))
done
[ "$STRAY" = 0 ] || echo "dropped $STRAY dependency .so files that cargo ndk copied beside the library"
if [ -f "$SO" ]; then
  echo "BUILT: $SO ($(stat -c %s "$SO") bytes)"
  echo "native library libvault_core.so SHA-256: $(sha256sum "$SO" | cut -d' ' -f1)"
else
  echo "No libvault_core.so was produced: the crate has no cdylib target yet (testnet-client/Cargo.toml [lib] needs crate-type = [\"rlib\", \"cdylib\"]). The build itself passed."
fi
