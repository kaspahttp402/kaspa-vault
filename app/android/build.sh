#!/usr/bin/env bash
# Builds KaspaVault without Gradle: aapt2 -> javac -> d8 -> zipalign -> apksigner
# A native library in jniLibs/arm64-v8a/ (made by build-native.sh, never by this script) is packed as lib/arm64-v8a/ when
# present; without it the build is the laptop-bridge shape and says so.
#
# Signing. The key never lives in this file. Either set KV_KEYSTORE, KV_KEY_ALIAS, KV_STORE_PASS, KV_KEY_PASS in the
# environment, or put these lines in ~/.kaspavault/release.properties (outside the tree; make the file owner-only):
#   keystore=/path/to/release.jks
#   alias=kaspavault
#   storePass=...
#   keyPass=...          (optional; the store password when left out)
# With neither, the build is signed with the auto-made debug key beside this script and says so loudly. That key stays the
# default on purpose: the phone has the app installed under it, and a different key means an uninstall first.
# Every build bumps android:versionCode in AndroidManifest.xml by one and prints the signing certificate's SHA-256.
set -e
cd "$(dirname "$0")"
SDK="$LOCALAPPDATA/Android/Sdk"; BT="$SDK/build-tools/36.1.0"; AJ="$SDK/platforms/android-36/android.jar"
export PATH="/c/Program Files/Android/Android Studio/jbr/bin:$PATH"

# ---- signing settings: environment first, then the properties file, else the debug key ----
PROPS="${KV_RELEASE_PROPS:-$HOME/.kaspavault/release.properties}"
if [ -z "${KV_KEYSTORE:-}" ] && [ -f "$PROPS" ]; then
  prop() { sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$PROPS" | tail -1 | tr -d '\r'; }
  KV_KEYSTORE="$(prop keystore)"
  KV_KEY_ALIAS="${KV_KEY_ALIAS:-$(prop alias)}"
  KV_STORE_PASS="${KV_STORE_PASS:-$(prop storePass)}"
  KV_KEY_PASS="${KV_KEY_PASS:-$(prop keyPass)}"
fi
if [ -n "${KV_KEYSTORE:-}" ]; then
  case "$KV_KEYSTORE" in ~*) KV_KEYSTORE="$HOME${KV_KEYSTORE#\~}";; esac
  [ -f "$KV_KEYSTORE" ] || { echo "The keystore named in KV_KEYSTORE or $PROPS is not there: $KV_KEYSTORE. Point it at the release keystore file." >&2; exit 1; }
  if [ -z "${KV_KEY_ALIAS:-}" ] || [ -z "${KV_STORE_PASS:-}" ]; then
    echo "Give the key alias and the store password too (KV_KEY_ALIAS and KV_STORE_PASS, or alias= and storePass= in $PROPS)." >&2; exit 1
  fi
  KV_KEY_PASS="${KV_KEY_PASS:-$KV_STORE_PASS}"
  SIGNING="release key $KV_KEYSTORE, alias $KV_KEY_ALIAS"
else
  KV_KEYSTORE=kaspavault-debug.keystore; KV_KEY_ALIAS=kaspavault; KV_STORE_PASS=kaspavault; KV_KEY_PASS=kaspavault
  [ -f "$KV_KEYSTORE" ] || keytool -genkeypair -keystore "$KV_KEYSTORE" -storepass "$KV_STORE_PASS" -keypass "$KV_KEY_PASS" -alias "$KV_KEY_ALIAS" -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=KaspaVault Debug"
  SIGNING="DEBUG"
fi
export KV_STORE_PASS KV_KEY_PASS   # read by apksigner from the environment, never shown
# android:debuggable is OPT-IN: KV_DEBUGGABLE=1 bash build.sh (debug key only) passes aapt2 --debug-mode so adb run-as can push test
# data and read the app files. Never the default: a debuggable APK lets anyone with adb read SharedPreferences and the wrapped blobs.
if [ "${KV_DEBUGGABLE:-0}" = 1 ] && [ "$SIGNING" = DEBUG ]; then LINK_DEBUG="--debug-mode"; echo "DEBUGGABLE build (KV_DEBUGGABLE=1): for test pushes only"; else LINK_DEBUG=""; fi

# ---- version code: the manifest holds the last built one; this build is the next ----
CUR="$(sed -n 's/.*android:versionCode="\([0-9][0-9]*\)".*/\1/p' AndroidManifest.xml | head -1)"
[ -n "$CUR" ] || { echo "AndroidManifest.xml has no android:versionCode; add one to the <manifest> tag." >&2; exit 1; }
NEXT=$((CUR + 1))
# The bump goes into the manifest before linking (aapt2's --version-code only fills a missing one); a failed build puts it back.
sed -i "s/android:versionCode=\"$CUR\"/android:versionCode=\"$NEXT\"/" AndroidManifest.xml
OK=0; trap '[ "$OK" = 1 ] || sed -i "s/android:versionCode=\"$NEXT\"/android:versionCode=\"$CUR\"/" AndroidManifest.xml' EXIT

rm -rf build && mkdir -p build/gen build/classes build/dex build/assets
cp -r ../web build/assets/web
# The desktop sample demo and the page notes stay on the laptop: the phone carries only what index.html runs.
rm -f build/assets/web/demo.js build/assets/web/demo.html build/assets/web/README.md
# What went in: one hash per bundled page file (the code reader jsqr.js among them), kept beside the APK.
(cd build/assets/web && find . -type f | LC_ALL=C sort | xargs sha256sum) > build/assets.sha256
"$BT/aapt2.exe" compile --dir res -o build/res.zip
"$BT/aapt2.exe" link $LINK_DEBUG -I "$AJ" --manifest AndroidManifest.xml -o build/base.apk build/res.zip \
  --java build/gen --min-sdk-version 33 --target-sdk-version 36
javac -encoding UTF-8 -nowarn --release 11 -classpath "$AJ" -d build/classes $(find src build/gen -name "*.java" | LC_ALL=C sort)
"$BT/d8.bat" --min-api 33 --lib "$AJ" --output build/dex $(find build/classes -name '*.class' | LC_ALL=C sort)
cp build/base.apk build/unsigned.apk
# jar entries get one fixed date so two builds of the same sources and version code give the same bytes (compare build/unsigned.sha256).
(cd build/dex && jar --update --date=2000-01-01T00:00:00Z --file=../unsigned.apk classes.dex)
(cd build && jar --update --date=2000-01-01T00:00:00Z --file=unsigned.apk assets)  # not aapt2 -A: on Windows it writes subfolder paths with backslashes
# ---- the wallet engine (optional): any .so in jniLibs/arm64-v8a goes in as lib/arm64-v8a; the platform extracts it at install ----
NATIVE_SHA=""
if ls jniLibs/arm64-v8a/*.so >/dev/null 2>&1; then
  mkdir -p build/lib/arm64-v8a
  cp jniLibs/arm64-v8a/*.so build/lib/arm64-v8a/
  (cd build && jar --update --date=2000-01-01T00:00:00Z --file=unsigned.apk lib)
  [ -f build/lib/arm64-v8a/libvault_core.so ] && NATIVE_SHA="$(sha256sum build/lib/arm64-v8a/libvault_core.so | cut -d' ' -f1)"
fi
"$BT/zipalign.exe" -p -f 4 build/unsigned.apk build/aligned.apk
sha256sum build/aligned.apk | cut -d' ' -f1 > build/unsigned.sha256
"$BT/apksigner.bat" sign --ks "$KV_KEYSTORE" --ks-key-alias "$KV_KEY_ALIAS" --ks-pass env:KV_STORE_PASS --key-pass env:KV_KEY_PASS --out build/KaspaVault.apk build/aligned.apk
CERT="$("$BT/apksigner.bat" verify --print-certs build/KaspaVault.apk | tr -d '\r' | sed -n 's/.*certificate SHA-256 digest: *//p' | head -1)"
OK=1

echo "code reader jsqr.js SHA-256: $(grep -E ' \*?\./jsqr\.js$' build/assets.sha256 | cut -d' ' -f1)"
echo "unsigned APK SHA-256 (build id): $(cat build/unsigned.sha256)"
if [ -n "$NATIVE_SHA" ]; then
  echo "native library libvault_core.so SHA-256: $NATIVE_SHA"
elif ls jniLibs/arm64-v8a/*.so >/dev/null 2>&1; then
  echo "native library packed: $(ls jniLibs/arm64-v8a/*.so | xargs -n1 basename | tr '\n' ' ')(no libvault_core.so among them)"
else
  echo "no native library (jniLibs/arm64-v8a is empty): laptop-bridge build"
fi
if [ "$SIGNING" = DEBUG ]; then
  echo "DEBUG-SIGNED, not for release: signed with the auto-made debug key beside this script. For a release, set KV_KEYSTORE, KV_KEY_ALIAS, KV_STORE_PASS, KV_KEY_PASS or fill $PROPS."
else
  echo "signed with $SIGNING"
fi
echo "signing certificate SHA-256: $CERT"
echo "BUILT: $(pwd)/build/KaspaVault.apk (versionCode $NEXT, $(stat -c %s build/KaspaVault.apk) bytes)"
