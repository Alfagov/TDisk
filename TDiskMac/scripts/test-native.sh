#!/bin/sh
set -eu
TDISK_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TDISK_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp/}tdisk-native-tests.XXXXXX")"
trap 'rm -rf "$TDISK_TEST_DIR"' EXIT
cd "$TDISK_ROOT"
"${TDISK_CARGO:-$HOME/.cargo/bin/cargo}" build --offline --release --lib
xcrun swiftc -swift-version 6 -parse-as-library -module-cache-path "$TDISK_TEST_DIR/modules" \
    -import-objc-header TDiskMac/Bridge/TDiskCore.h \
    TDiskMac/TDiskMac/ScanBackend.swift TDiskMac/TDiskMac/DiskModel.swift TDiskMac/Tests/NativeSmoke.swift \
    -L target/release -ltdisk_core -framework AppKit -framework Foundation -lobjc \
    -o "$TDISK_TEST_DIR/NativeSmoke"
"$TDISK_TEST_DIR/NativeSmoke"
