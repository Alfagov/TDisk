#!/bin/sh
set -eu
TDISK_REPO_ROOT="$(cd "$PROJECT_DIR/.." && pwd)"
TDISK_CARGO="${TDISK_CARGO:-$HOME/.cargo/bin/cargo}"
TDISK_OUTPUT="$DERIVED_FILE_DIR/Rust"
mkdir -p "$TDISK_OUTPUT"
for TDISK_ARCH in $ARCHS; do
    case "$TDISK_ARCH" in
        arm64) TDISK_TARGET=aarch64-apple-darwin ;;
        x86_64) TDISK_TARGET=x86_64-apple-darwin ;;
        *) echo "error: Unsupported Mac architecture: $TDISK_ARCH"; exit 1 ;;
    esac
    "$TDISK_CARGO" build --offline --release --lib --manifest-path "$TDISK_REPO_ROOT/Cargo.toml" --target "$TDISK_TARGET" --target-dir "$TDISK_REPO_ROOT/target"
    TDISK_LIBRARY="$TDISK_REPO_ROOT/target/$TDISK_TARGET/release/libtdisk_core.a"
    cp "$TDISK_LIBRARY" "$TDISK_OUTPUT/$TDISK_ARCH.a"
done
# All paths are quoted individually; don't interpolate a list of shell paths.
if [ -f "$TDISK_OUTPUT/arm64.a" ] && [ -f "$TDISK_OUTPUT/x86_64.a" ] && [ "$(echo "$ARCHS" | wc -w | tr -d ' ')" -gt 1 ]; then
    /usr/bin/lipo -create "$TDISK_OUTPUT/arm64.a" "$TDISK_OUTPUT/x86_64.a" -output "$TDISK_OUTPUT/libtdisk_core.a"
else
    cp "$TDISK_OUTPUT/$TDISK_ARCH.a" "$TDISK_OUTPUT/libtdisk_core.a"
fi
