#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Build mediaflow-proxy-light for Android ABIs and (optionally) drop the
# stripped binaries directly into a sibling `mediaflow-android` project's
# `jniLibs/` tree so `./gradlew assembleRelease` just works afterwards.
#
# On Android 10+ binaries cannot be executed from app-writable paths, so we
# package the proxy as `libmediaflow-proxy.so` inside `jniLibs/<abi>/` — the
# Android packager extracts those to `nativeLibraryDir` at install time,
# which IS executable.  The companion app's `ProxyManager` then spawns the
# binary from there via `ProcessBuilder`.
#
# Requirements:
#   - Android NDK installed (set ANDROID_NDK_HOME)
#   - Rust targets:
#       rustup target add aarch64-linux-android armv7-linux-androideabi \
#                        x86_64-linux-android i686-linux-android
#
# Usage:
#   export ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/<version>
#   ./tools/build-android.sh
#
# Environment overrides:
#   ANDROID_APP_DIR — path to the sibling Android project (defaults to
#                     `../mediaflow-android`).  Set to empty string to skip
#                     the auto-copy step.
#   ABIS           — space-separated list of ABIs to build
#                    (default: all four).  Useful for faster iteration.
# ---------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ANDROID_APP_DIR="${ANDROID_APP_DIR-$PROJECT_DIR/../mediaflow-android}"
API_LEVEL=21

: "${ANDROID_NDK_HOME:?'ANDROID_NDK_HOME must be set to your NDK installation directory'}"

# Detect NDK toolchain bin directory (host-specific)
NDK_TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt"
case "$(uname)" in
    Darwin)
        NDK_BIN="$NDK_TOOLCHAIN/darwin-x86_64/bin"
        [[ -d "$NDK_BIN" ]] || NDK_BIN="$NDK_TOOLCHAIN/darwin-arm64/bin"
        ;;
    Linux)
        NDK_BIN="$NDK_TOOLCHAIN/linux-x86_64/bin"
        ;;
    *)
        echo "Unsupported host OS: $(uname)" >&2; exit 1;;
esac
[[ -d "$NDK_BIN" ]] || { echo "NDK toolchain not found at $NDK_BIN" >&2; exit 1; }
export PATH="$NDK_BIN:$PATH"

# Feature set — mirrors the Android release CI.  Keep in sync with the
# `mediaflow-android` release workflow.
# - `tls-rustls` instead of `tls-native`: avoids linker conflicts between
#   system OpenSSL and the BoringSSL shipped by rquest's `boring-sys2`.
# - `web-ui` included so the on-device web UI (served on localhost) works
#   when the user opens the configurator from a browser.
FEATURES="hls,mpd,drm,xtream,extractors,base64-url,tls-rustls,acestream,web-ui"

# All ABIs supported by default
ALL_TARGETS=(
    "aarch64-linux-android|aarch64-linux-android${API_LEVEL}-clang|arm64-v8a"
    "armv7-linux-androideabi|armv7a-linux-androideabi${API_LEVEL}-clang|armeabi-v7a"
    "x86_64-linux-android|x86_64-linux-android${API_LEVEL}-clang|x86_64"
    "i686-linux-android|i686-linux-android${API_LEVEL}-clang|x86"
)

# Filter by ABIS env var if set
if [[ -n "${ABIS:-}" ]]; then
    FILTERED=()
    for wanted in $ABIS; do
        for entry in "${ALL_TARGETS[@]}"; do
            IFS='|' read -r _ _ ABI <<<"$entry"
            [[ "$ABI" == "$wanted" ]] && FILTERED+=("$entry")
        done
    done
    TARGETS=("${FILTERED[@]}")
else
    TARGETS=("${ALL_TARGETS[@]}")
fi

echo "==> Building ${#TARGETS[@]} ABI(s):"
for entry in "${TARGETS[@]}"; do
    IFS='|' read -r _ _ ABI <<<"$entry"
    echo "     - $ABI"
done

for entry in "${TARGETS[@]}"; do
    IFS='|' read -r RUST_TARGET LINKER ABI <<<"$entry"
    echo ""
    echo "==> $ABI ($RUST_TARGET)"

    TARGET_UPPER="$(echo "$RUST_TARGET" | tr 'a-z-' 'A-Z_')"
    export "CARGO_TARGET_${TARGET_UPPER}_LINKER=$NDK_BIN/$LINKER"
    export "CC_${RUST_TARGET}=$NDK_BIN/$LINKER"
    export "AR_${RUST_TARGET}=$NDK_BIN/llvm-ar"
    # NDK r23+ dropped legacy `<target>-ranlib` shims; vendored-openssl's
    # Makefile hard-codes that name.  Point it at `llvm-ranlib` instead.
    export "RANLIB_${RUST_TARGET}=$NDK_BIN/llvm-ranlib"

    cargo build \
        --release \
        --target "$RUST_TARGET" \
        --no-default-features \
        --features "$FEATURES" \
        --manifest-path "$PROJECT_DIR/Cargo.toml"

    OUT="$PROJECT_DIR/target/$RUST_TARGET/release/mediaflow-proxy-light"
    STRIPPED="$PROJECT_DIR/target/$RUST_TARGET/release/libmediaflow-proxy.so"

    "$NDK_BIN/llvm-strip" -o "$STRIPPED" "$OUT"
    echo "    built + stripped: $STRIPPED ($(du -sh "$STRIPPED" | cut -f1))"

    # Auto-copy into the sibling Android project's jniLibs/ tree if present.
    if [[ -n "$ANDROID_APP_DIR" && -d "$ANDROID_APP_DIR/app/src/main" ]]; then
        DEST_DIR="$ANDROID_APP_DIR/app/src/main/jniLibs/$ABI"
        mkdir -p "$DEST_DIR"
        cp "$STRIPPED" "$DEST_DIR/libmediaflow-proxy.so"
        echo "    → installed to $DEST_DIR/libmediaflow-proxy.so"
    fi
done

echo ""
if [[ -n "$ANDROID_APP_DIR" && -d "$ANDROID_APP_DIR/app/src/main" ]]; then
    echo "==> Done.  Binaries installed into $ANDROID_APP_DIR/app/src/main/jniLibs/"
    echo "    Next: cd $ANDROID_APP_DIR && ./gradlew assembleRelease"
else
    echo "==> Done.  Stripped binaries available at:"
    for entry in "${TARGETS[@]}"; do
        IFS='|' read -r RUST_TARGET _ ABI <<<"$entry"
        echo "     $ABI:  target/$RUST_TARGET/release/libmediaflow-proxy.so"
    done
    echo ""
    echo "    ANDROID_APP_DIR not set or not found — copy manually into"
    echo "    <android-project>/app/src/main/jniLibs/<abi>/libmediaflow-proxy.so"
fi
