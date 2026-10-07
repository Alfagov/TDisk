# TDisk for macOS

The native SwiftUI app implements the design in `../design/TDisk-macOS-design.md`. It uses the same Rust scanner and deletion protections as the TUI. Requires macOS **14.4+**, Xcode with Swift 6, and Rust/Cargo.

## Run in Xcode

1. Install Rust if needed using its official installer. Run `cargo fetch` once from the repository root to download dependencies. The Xcode build itself runs Cargo offline.
2. Open **TDiskMac.xcodeproj** in this directory.
3. Select the **TDiskMac** scheme and **My Mac**, then press **⌘R**.

Your existing bundle identifier and development team are retained. For a local unsigned command-line build:

```sh
xcodebuild -project TDiskMac/TDiskMac.xcodeproj -scheme TDiskMac \
  -configuration Debug -derivedDataPath /tmp/TDisk-Mac-Build \
  CODE_SIGNING_ALLOWED=NO build
```

Run this command from the repository root. The app is `/tmp/TDisk-Mac-Build/Build/Products/Debug/TDisk.app`.

The **Build Rust core** phase builds an optimized static library for the architectures selected by Xcode. For an Intel or universal build, install the appropriate Rust standard library first:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

Then build Release with `ARCHS="arm64 x86_64" ONLY_ACTIVE_ARCH=NO`. No Rust executable or runtime needs to be installed on the end user's Mac. `TDISK_CARGO` can override the Cargo executable path. Dependency versions are pinned by the repository's Cargo.lock.

## Use

The welcome screen waits for an explicit location choice. Select a device, favorite, recent location, **Scan Startup Disk**, or **Choose Folder**. System volume usage appears independently of traversal. Recursive folder allocations appear progressively and are marked as lower bounds while pending.

Double-click a directory or use **⌘↓** to open it. Back/Forward preserve selection; **⌘↑** opens its parent within the current scan. Sort by Name or Allocated Size. Ordering remains stable as existing row sizes update and is sorted again when scanning stops. Share percentages are withheld during scanning. View controls hide Share/Status, toggle sidebar/inspector, and select System/Light/Dark appearance. The inspector folds away at narrow window widths.

**⌘.** stops scanning and retains partial results; **⌘R** rescans the original root. Choosing another location cancels the previous scan. Devices are scanned separately. Disconnecting a volume retains a snapshot and disables filesystem actions.

**⌘⌫**, the inspector, or the row context menu opens a Move to Trash warning with the resolved path and a folder-contents warning. Escape cancels; Return alone does not confirm. Confirming invokes macOS FileManager's native Trash API. There is no permanent-delete fallback or Empty Trash command. Library contents and other ordinary subfolders are eligible where macOS permits; important system/home containers themselves, mount roots, the scan root, ancestors, symlinks, and excluded entries are protected. The backend rechecks identity, mounts and protections at execution. After a successful move, the row and ancestor allocations update, remaining sizes are labeled as a snapshot, and volume usage is queried again independently.

Errors remain visible as partial results, with affected-location details and a shortcut to macOS privacy settings. Full Disk Access can improve privacy-limited scans, but does not override every filesystem or system protection.

## Architecture and permissions

`ScanBackend.swift` serializes calls into `Bridge/TDiskCore.h` through a Swift actor. Rust owns the tree and sends only the requested folder and incremental row updates. Stable IDs encode the original pathname bytes. The main actor owns UI state; polling runs every 200 ms while scanning and stops at completion. Traversal uses cooperative cancellation. Trash operations run off the UI actor.

The app intentionally runs **without App Sandbox** as a whole-disk utility. macOS privacy permissions, filesystem permissions, SIP, and Hardened Runtime remain in effect. The build script sandbox is disabled so Cargo can read its cache and compile the repository. There are no elevated privileges, permission-granting operations, network services, or destructive-delete APIs. Shipping outside local development still requires your signing/notarization/distribution process; this repository does not configure App Store distribution.

Exact arbitrary-folder totals require traversal. APFS clones, snapshots and inaccessible files mean scanned file allocation is not equivalent to physical volume usage. The app never presents their difference as missing storage or promises that moving allocated bytes to Trash frees that amount.

## Validation

From the repository root:

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
TDiskMac/scripts/test-native.sh
```

The native smoke checks use generated temporary fixtures. They cover bridge decoding, allocation, Unicode paths, scan-generation isolation, navigation, selection restoration, protected roots, non-destructive Trash preparation/cancellation, disconnect/reconnect state, and failed scans. They do not move user data to Trash. The existing ignored Rust native Trash integration check is separate and uses a disposable fixture.

The build-time procedural macro profile retains debug information to avoid the [macOS 27 linker/dyld string-pool alignment issue](https://github.com/rust-lang/rust/issues/157750); shipping scanner optimization remains enabled.
