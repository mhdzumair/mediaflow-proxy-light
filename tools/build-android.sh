#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Build the MediaFlow Proxy Light binary for Android ABIs.
#
# Requirements:
#   - Android NDK installed (set ANDROID_NDK_HOME)
#   - Rust targets installed:
#       rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
#
# Usage:
#   export ANDROID_NDK_HOME=$HOME/Library/Android/sdk/ndk/<version>
#   ./tools/build-android.sh
#
# Output:
#   target/<abi>/release/mediaflow-proxy-light   (stripped binary)
# ---------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

: "${ANDROID_NDK_HOME:?'ANDROID_NDK_HOME must be set to your NDK installation directory'}"

# Minimum API level
API_LEVEL=21

# Detect NDK toolchain bin directory
NDK_TOOLCHAIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt"
if [[ "$(uname)" == "Darwin" ]]; then
    NDK_BIN="$NDK_TOOLCHAIN/darwin-x86_64/bin"
    if [[ ! -d "$NDK_BIN" ]]; then
        # Apple Silicon NDK path
        NDK_BIN="$NDK_TOOLCHAIN/darwin-arm64/bin"
    fi
else
    NDK_BIN="$NDK_TOOLCHAIN/linux-x86_64/bin"
fi

if [[ ! -d "$NDK_BIN" ]]; then
    echo "ERROR: NDK toolchain not found at $NDK_BIN" >&2
    exit 1
fi

export PATH="$NDK_BIN:$PATH"

FEATURES="hls,mpd,drm,xtream,extractors,web-ui,redis,acestream"

TARGETS=(
    "aarch64-linux-android:aarch64-linux-android${API_LEVEL}-clang:arm64-v8a"
    "armv7-linux-androideabi:armv7a-linux-androideabi${API_LEVEL}-clang:armeabi-v7a"
    "x86_64-linux-android:x86_64-linux-android${API_LEVEL}-clang:x86_64"
)

for entry in "${TARGETS[@]}"; do
    IFS=':' read -r RUST_TARGET LINKER ABI <<<"$entry"
    echo ""
    echo "==> Building $ABI ($RUST_TARGET)"

    # Set linker via environment variable
    CARGO_LINKER_VAR="CARGO_TARGET_${RUST_TARGET//-/_}_LINKER"
    CARGO_LINKER_VAR="${CARGO_LINKER_VAR^^}"
    export "$CARGO_LINKER_VAR"="$LINKER"

    cargo build \
        --release \
        --target "$RUST_TARGET" \
        --features "$FEATURES" \
        --manifest-path "$PROJECT_DIR/Cargo.toml"

    OUT="$PROJECT_DIR/target/$RUST_TARGET/release/mediaflow-proxy-light"
    STRIPPED="$PROJECT_DIR/target/$RUST_TARGET/release/mediaflow-proxy-light-stripped"

    # Strip the binary if llvm-strip is available
    if command -v llvm-strip &>/dev/null; then
        llvm-strip -o "$STRIPPED" "$OUT"
        echo "    Stripped: $STRIPPED ($(du -sh "$STRIPPED" | cut -f1))"
    else
        cp "$OUT" "$STRIPPED"
        echo "    Output:   $STRIPPED ($(du -sh "$STRIPPED" | cut -f1)) [not stripped — llvm-strip not found]"
    fi
done

echo ""
echo "==> Done. Copy stripped binaries to the Android app:"
echo ""
for entry in "${TARGETS[@]}"; do
    IFS=':' read -r RUST_TARGET _ ABI <<<"$entry"
    echo "    android/app/src/main/assets/binaries/$ABI/mediaflow-proxy"
    echo "    <- target/$RUST_TARGET/release/mediaflow-proxy-light-stripped"
done
echo ""
