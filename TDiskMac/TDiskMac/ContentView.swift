import AppKit
import SwiftUI

struct ContentView: View {
  @Bindable var model: DiskModel
  var body: some View {
    GeometryReader { geometry in
      NavigationSplitView(columnVisibility: $model.columnVisibility) {
        locations
          .navigationSplitViewColumnWidth(min: 170, ideal: 205, max: 290)
      } detail: {
        Group {
          if model.rootPath == nil { WelcomeView(model: model) } else { FolderView(model: model) }
        }
        .inspector(
          isPresented: Binding(
            get: { model.inspectorVisible && model.windowWidth >= 980 && model.rootPath != nil },
            set: {
              if model.windowWidth >= 980 && model.rootPath != nil { model.inspectorVisible = $0 }
            })
        ) {
          InspectorView(model: model).inspectorColumnWidth(min: 230, ideal: 250, max: 330)
        }
      }
      .navigationSplitViewStyle(.balanced)
      .onAppear { model.windowWidth = geometry.size.width }
      .onChange(of: geometry.size.width) { _, value in model.windowWidth = value }
    }
    .frame(minWidth: 760, minHeight: 520)
    .navigationTitle(model.folderName)
    .toolbar { navigationToolbar }
    .preferredColorScheme(
      model.appearance == "dark" ? .dark : model.appearance == "light" ? .light : nil
    )
    .sheet(item: $model.trashConfirmation) { reply in
      TrashConfirmationView(model: model, reply: reply)
    }
    .sheet(item: $model.messageSheet) { message in
      MessageView(message: message) { model.messageSheet = nil }
    }
    .sheet(isPresented: $model.showIssues) { IssuesView(model: model) }
    .sheet(isPresented: $model.showHelp) { HelpView() }
    .onReceive(
      NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.didMountNotification)
    ) { model.handleMount($0.userInfo?[NSWorkspace.volumeURLUserInfoKey] as? URL) }
    .onReceive(
      NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.didUnmountNotification)
    ) { model.handleUnmount($0.userInfo?[NSWorkspace.volumeURLUserInfoKey] as? URL) }
  }
  private var locations: some View {
    List(selection: $model.locationSelection) {
      Section("Devices") { ForEach(model.devices) { location in locationRow(location) } }
      Section("Favorites") { ForEach(model.favorites) { location in locationRow(location) } }
      if !model.recents.isEmpty {
        Section("Recent Locations") {
          ForEach(model.recents) { location in
            locationRow(location).contextMenu {
              Button("Remove from Recent Locations") { model.removeRecent(location.path) }
            }
          }
        }
      }
    }
    .listStyle(.sidebar)
    .disabled(model.navigationLocked)
    .onChange(of: model.locationSelection) { _, path in
      if let path, path != model.rootPath || model.offline { model.start(path) }
    }
    .safeAreaInset(edge: .bottom) {
      Text("Each disk is scanned separately.")
        .font(.caption).foregroundStyle(.secondary).padding(16).frame(
          maxWidth: .infinity, alignment: .leading)
    }
  }
  private func locationRow(_ location: DiskLocation) -> some View {
    Label(location.name, systemImage: location.symbol).tag(location.path).help(location.path)
  }
  @ToolbarContentBuilder private var navigationToolbar: some ToolbarContent {
    ToolbarItemGroup(placement: .navigation) {
      Button {
        model.goBack()
      } label: {
        Label("Back", systemImage: "chevron.left")
      }.disabled(!model.canGoBack).help("Back (⌘[)")
      Button {
        model.goForward()
      } label: {
        Label("Forward", systemImage: "chevron.right")
      }.disabled(!model.canGoForward).help("Forward (⌘])")
    }
    ToolbarItem(placement: .primaryAction) {
      Button("Choose Folder…", systemImage: "folder.badge.plus") { model.chooseFolder() }.disabled(
        model.navigationLocked)
    }
    ToolbarItemGroup(placement: .automatic) {
      if model.isScanning {
        Button {
          model.stop()
        } label: {
          Label("Stop Scan", systemImage: "stop.circle")
        }.disabled(model.isStopping).help("Stop scanning (⌘.)")
      } else {
        Button {
          model.rescan()
        } label: {
          Label("Rescan", systemImage: "arrow.clockwise")
        }.disabled(model.rootPath == nil || model.navigationLocked || model.offline).help(
          "Rescan (⌘R)")
      }
      Button {
        model.inspectorVisible.toggle()
      } label: {
        Label("Show Inspector", systemImage: "sidebar.right")
      }
      .disabled(model.rootPath == nil).help("Show or hide inspector (⌥⌘I)")
    }
  }
}
private struct WelcomeView: View {
  var model: DiskModel
  var body: some View {
    VStack(spacing: 16) {
      Image(systemName: "internaldrive").font(.system(size: 52, weight: .light)).foregroundStyle(
        .tint
      ).padding(.bottom, 8).accessibilityHidden(true)
      Text("See where your space goes.").font(.system(size: 27, weight: .semibold))
      Text(
        "Explore the folders on your Mac, understand their size,\nand find what’s taking up space."
      )
      .foregroundStyle(.secondary).multilineTextAlignment(.center).lineSpacing(4)
      VStack(spacing: 12) {
        Button("Scan Startup Disk") { model.start("/") }.buttonStyle(.borderedProminent)
          .controlSize(.large)
        Button("Choose Folder…") { model.chooseFolder() }.controlSize(.large)
      }.padding(.top, 12)
      Text("Disk usage appears first. Folder sizes update as scanning continues.")
        .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center).padding(
          .top, 12)
    }.padding(32).frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}
private struct FolderView: View {
  @Bindable var model: DiskModel
  var body: some View {
    GeometryReader { geometry in
      VStack(spacing: 0) {
        VStack(alignment: .leading, spacing: 20) {
          VolumeSummary(volume: model.snapshot?.volume)
          Divider()
          HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 6) {
              Text(model.folderName).font(.title2.weight(.semibold))
              Text(model.snapshot?.folder?.path ?? model.rootPath ?? "").font(.caption)
                .foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle).textSelection(
                  .enabled)
            }
            Spacer()
            if model.isScanning {
              HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(model.isStopping ? "Stopping…" : "Scanning")
              }.font(.caption).foregroundStyle(.secondary)
            } else if model.offline {
              statusBadge("Disk unavailable", color: .orange)
            } else if model.snapshot?.status == "stopped" {
              statusBadge("Stopped · partial results", color: .secondary)
            } else if (model.snapshot?.folder?.errors ?? 0) > 0 {
              statusBadge("Partial scan", color: .orange)
            }
          }
          HStack(spacing: 6) {
            Text(
              model.snapshot?.totalsStale == true
                ? "Snapshot file allocation" : "Scanned file allocation"
            ).foregroundStyle(.secondary)
            if let folder = model.snapshot?.folder {
              Text(
                folder.pending
                  ? "At least \(DiskFormat.bytes(folder.size)) found"
                  : DiskFormat.bytes(folder.size)
              ).fontWeight(.medium)
            } else {
              Text("Measuring…").foregroundStyle(.secondary)
            }
            Image(systemName: "info.circle").foregroundStyle(.secondary).help(
              "File allocation differs from disk usage: APFS shared blocks, snapshots and inaccessible files affect the measurements."
            )
          }.font(.callout)
        }.padding(24)
        if model.rows.isEmpty {
          emptyState.frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          fileTable(width: geometry.size.width)
        }
        if !model.rows.isEmpty {
          HStack {
            Text(
              model.isScanning
                ? "Percentages appear when the scan stops"
                : "Share of known allocation in this folder")
            Spacer()
            if model.isScanning { Text("Sort order held while scanning") }
          }.font(.caption2).foregroundStyle(.secondary).padding(.horizontal, 24).padding(
            .vertical, 8)
        }
        notices
        breadcrumbs
        statusBar
      }
      .background(Color(nsColor: .textBackgroundColor))
    }
  }
  private func statusBadge(_ title: String, color: Color) -> some View {
    Text(title).font(.caption).foregroundStyle(color).padding(.horizontal, 8).padding(.vertical, 4)
      .background(color.opacity(0.1), in: RoundedRectangle(cornerRadius: 5))
  }
  private func fileTable(width: CGFloat) -> some View {
    Table(model.rows, selection: $model.selection, sortOrder: $model.sortOrder) {
      TableColumn("Name", value: \.name) { entry in
        HStack(spacing: 8) {
          Image(systemName: entry.isDirectory ? "folder.fill" : "doc").foregroundStyle(
            entry.isDirectory ? Color.accentColor : Color.secondary
          ).accessibilityHidden(true)
          Text(entry.safeName).lineLimit(1).truncationMode(.middle)
        }.help(entry.path)
      }.width(min: 160, ideal: 260)
      TableColumn("Allocated Size", value: \.size) { entry in
        Text(DiskFormat.size(entry, scanning: model.isScanning)).monospacedDigit().frame(
          maxWidth: .infinity, alignment: .trailing
        )
        .foregroundStyle(entry.pending ? .secondary : .primary)
      }.width(min: 130, ideal: 145, max: 180)
      if model.shareVisible && width > 590 {
        TableColumn("Share") { entry in
          AllocationShare(
            size: entry.size, total: model.snapshot?.folder?.size ?? 0,
            hidden: model.isScanning || entry.skipReason != nil)
        }.width(min: 105, ideal: 120, max: 150)
      }
      if model.statusVisible && width > 480 {
        TableColumn("Status") { entry in
          HStack(spacing: 4) {
            if entry.errors > 0 {
              Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
                .accessibilityHidden(true)
            }
            Text(DiskFormat.status(entry, scanning: model.isScanning)).font(.caption)
              .foregroundStyle(.secondary)
          }
        }.width(min: 100, ideal: 115, max: 160)
      }
    }
    .tableStyle(.inset(alternatesRowBackgrounds: true))
    .onChange(of: model.sortOrder) { _, _ in model.sortRows() }
    .contextMenu(forSelectionType: String.self) { ids in
      if let id = ids.first, let entry = model.rows.first(where: { $0.id == id }) {
        if entry.isDirectory {
          Button("Open Folder") { model.open(entry) }.disabled(model.navigationLocked)
        }
        Button("Open in Finder") { model.reveal(entry) }.disabled(
          !entry.actionsAvailable || model.offline)
        Button("Copy Path") { model.copyPath(entry) }.disabled(!entry.actionsAvailable)
        Divider()
        Button("Move to Trash…", role: .destructive) { model.requestTrash(entry) }
          .disabled(
            model.isScanning || model.navigationLocked || model.offline
              || entry.trashBlockedReason != nil)
      }
    } primaryAction: { ids in
      if let id = ids.first, let entry = model.rows.first(where: { $0.id == id }) {
        model.open(entry)
      }
    }
    .disabled(model.isTrashing)
  }
  @ViewBuilder private var emptyState: some View {
    if model.offline {
      ContentUnavailableView(
        "Disk Unavailable", systemImage: "externaldrive.badge.xmark",
        description: Text("Choose another location or reconnect this disk."))
    } else if let error = model.startError ?? model.snapshot?.error {
      VStack(spacing: 14) {
        Image(systemName: "exclamationmark.triangle").font(.largeTitle).foregroundStyle(.orange)
        Text("This location couldn’t be read").font(.headline)
        Text(error).foregroundStyle(.secondary).textSelection(.enabled)
        Button("Choose Folder…") { model.chooseFolder() }
      }.padding(24)
    } else if let reason = model.snapshot?.folder?.skipReason {
      ContentUnavailableView(
        "Not Included", systemImage: "externaldrive",
        description: Text(
          reason == "other volume"
            ? "Select this disk as a new scan root to measure it separately."
            : "This directory was counted elsewhere in the scan."))
    } else if model.snapshot?.status == "stopped" && model.snapshot?.folder == nil {
      ContentUnavailableView(
        "Scan Stopped", systemImage: "stop.circle",
        description: Text(
          "Scanning stopped before folder contents were available. Rescan to start again."))
    } else if model.snapshot?.folder?.pending == true || model.snapshot == nil {
      VStack(spacing: 12) {
        if model.isScanning { ProgressView() }
        Text(model.isScanning ? "Loading folder contents…" : "This folder wasn’t fully scanned.")
          .foregroundStyle(.secondary)
        Text("You can go back while sizes are measured.").font(.caption).foregroundStyle(.secondary)
      }
    } else if model.snapshot?.folder == nil {
      ContentUnavailableView(
        "Folder No Longer Available", systemImage: "folder.badge.questionmark",
        description: Text("Go back or rescan to refresh this location."))
    } else if (model.snapshot?.folder?.errors ?? 0) > 0 {
      ContentUnavailableView(
        "Partial Results", systemImage: "folder.badge.questionmark",
        description: Text(
          "Some contents couldn’t be read. Open Details for the affected locations."))
    } else {
      ContentUnavailableView("This Folder Is Empty", systemImage: "folder")
    }
  }
  @ViewBuilder private var notices: some View {
    if let notice = model.notice {
      HStack(spacing: 10) {
        Image(systemName: "info.circle")
        Text(notice).font(.caption)
        Spacer()
        if model.snapshot?.totalsStale == true {
          Button("Rescan") { model.rescan() }.disabled(model.navigationLocked)
        }
        Button {
          model.notice = nil
        } label: {
          Image(systemName: "xmark")
        }.buttonStyle(.plain).accessibilityLabel("Dismiss notice")
      }.foregroundStyle(.secondary).padding(12).background(
        .quaternary, in: RoundedRectangle(cornerRadius: 8)
      ).padding(.horizontal, 24).padding(.bottom, 12)
    }
    if (model.snapshot?.folder?.errors ?? 0) > 0 {
      HStack(spacing: 10) {
        Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange)
        Text("Some items couldn’t be read. Sizes may be incomplete.").font(.caption)
        Spacer()
        Button("Details") { model.loadIssues() }
      }.padding(12).background(Color.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .padding(.horizontal, 24).padding(.bottom, 12)
    }
  }
  private var breadcrumbs: some View {
    HStack(spacing: 4) {
      ForEach(Array((model.snapshot?.breadcrumbs ?? []).enumerated()), id: \.element.id) {
        index, entry in
        if index > 0 {
          Image(systemName: "chevron.right").font(.system(size: 9)).foregroundStyle(.tertiary)
            .accessibilityHidden(true)
        }
        Button(index == 0 ? model.rootLabel : entry.safeName) { model.navigate(entry.id) }
          .buttonStyle(.plain).lineLimit(1).help(entry.path).disabled(model.navigationLocked)
      }
      Spacer(minLength: 0)
    }.font(.caption).foregroundStyle(.secondary).padding(.horizontal, 24).padding(.vertical, 10)
  }
  private var statusBar: some View {
    HStack(spacing: 6) {
      if model.isTrashing {
        ProgressView().controlSize(.mini)
        Text("Moving to Trash…")
      } else if model.isScanning {
        ProgressView().controlSize(.mini)
        Text(
          "\(model.snapshot?.files ?? 0) files · \(model.snapshot?.directories ?? 0) folders · \(elapsed)"
        )
      } else {
        Image(systemName: model.snapshot?.status == "stopped" ? "stop.circle" : "checkmark")
        Text(
          model.snapshot?.status == "stopped"
            ? "Stopped · partial results"
            : model.snapshot?.status == "failed" ? "Scan failed" : "Scan finished")
        Text("· \(model.rows.count) items")
      }
      Spacer()
      if model.windowWidth > 950 { Text("Other mounted volumes excluded") }
      Button {
        model.showHelp = true
      } label: {
        Label("Help", systemImage: "questionmark.circle")
      }.buttonStyle(.plain)
    }.font(.caption2).foregroundStyle(.secondary).padding(.horizontal, 14).padding(.vertical, 8)
      .background(.bar)
  }
  private var elapsed: String {
    let seconds = (model.snapshot?.elapsedMilliseconds ?? 0) / 1000
    return String(format: "%02d:%02d elapsed", seconds / 60, seconds % 60)
  }
}
private struct VolumeSummary: View {
  let volume: DiskVolume?
  @State private var showInfo = false
  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      HStack {
        Text("Disk space").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
        Spacer()
        Button {
          showInfo.toggle()
        } label: {
          Image(systemName: "info.circle")
        }.buttonStyle(.plain).foregroundStyle(.secondary).accessibilityLabel("About disk space")
      }
      if let volume {
        ViewThatFits(in: .horizontal) {
          HStack(alignment: .firstTextBaseline) {
            Text(DiskFormat.bytes(volume.used)).font(.system(size: 30, weight: .semibold))
              .monospacedDigit()
            Text("used of \(DiskFormat.bytes(volume.total))").foregroundStyle(.secondary)
            Spacer()
            Text("\(DiskFormat.bytes(volume.available)) available").font(.caption).foregroundStyle(
              .secondary)
          }
          VStack(alignment: .leading, spacing: 5) {
            Text("\(DiskFormat.bytes(volume.used)) used").font(.title2.weight(.semibold))
            Text(
              "\(DiskFormat.bytes(volume.total)) total · \(DiskFormat.bytes(volume.available)) available"
            ).font(.caption).foregroundStyle(.secondary)
          }
        }
        GeometryReader { geometry in
          ZStack(alignment: .leading) {
            Capsule().fill(.quaternary)
            Capsule().fill(Color.accentColor).frame(
              width: geometry.size.width
                * min(1, Double(volume.used) / Double(max(volume.total, 1))))
          }
        }.frame(height: 7).accessibilityHidden(true)
        HStack {
          Text(volume.mount == "/System/Volumes/Data" ? "Startup Data volume" : volume.mount)
          Spacer()
          Text("Reported by macOS")
        }.font(.caption2).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
      } else {
        Text("Disk space unavailable").font(.title3)
        Text("Folder sizes will still appear as scanning continues.").font(.caption)
          .foregroundStyle(.secondary)
      }
    }.popover(isPresented: $showInfo) {
      VStack(alignment: .leading, spacing: 10) {
        Text("Two different measurements").font(.headline)
        Text(
          "Disk space is reported by macOS for the volume. Folder allocation is measured by scanning files. Shared APFS blocks, snapshots and inaccessible files can make these values differ. They shouldn’t be subtracted to estimate missing storage."
        )
        if let volume { Text("Volume: \(volume.mount)").font(.caption).textSelection(.enabled) }
      }.padding(20).frame(width: 330)
    }
  }
}
private struct AllocationShare: View {
  let size: UInt64
  let total: UInt64
  let hidden: Bool
  var body: some View {
    HStack(spacing: 8) {
      GeometryReader { geometry in
        ZStack(alignment: .leading) {
          Capsule().fill(.quaternary)
          if !hidden && total > 0 {
            Capsule().fill(Color.accentColor).frame(
              width: geometry.size.width * min(1, Double(size) / Double(total)))
          }
        }
      }.frame(height: 4).accessibilityHidden(true)
      if !hidden && total > 0 {
        Text((Double(size) / Double(total)).formatted(.percent.precision(.fractionLength(1)))).font(
          .caption
        ).monospacedDigit().frame(width: 43, alignment: .trailing)
      }
    }.accessibilityLabel("Share of known folder allocation").accessibilityValue(
      hidden || total == 0
        ? "Unavailable"
        : (Double(size) / Double(total)).formatted(.percent.precision(.fractionLength(1))))
  }
}
private struct InspectorView: View {
  var model: DiskModel
  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 18) {
        Text("Selected Item").font(.caption.weight(.medium)).foregroundStyle(.secondary)
        if let entry = model.selected {
          VStack(spacing: 8) {
            Image(systemName: entry.isDirectory ? "folder.fill" : "doc").font(
              .system(size: 50, weight: .light)
            ).foregroundStyle(.tint).accessibilityHidden(true)
            Text(entry.safeName).font(.title3.weight(.semibold)).multilineTextAlignment(.center)
            Text(entry.isDirectory ? "Folder" : "File").font(.caption).foregroundStyle(.secondary)
          }.frame(maxWidth: .infinity).padding(.vertical, 12)
          Divider()
          detail("Allocated size") {
            Text(DiskFormat.size(entry, scanning: model.isScanning)).font(.title2.weight(.medium))
              .monospacedDigit()
            Text(DiskFormat.status(entry, scanning: model.isScanning)).font(.caption)
              .foregroundStyle(entry.errors > 0 ? Color.orange : Color.secondary)
          }
          Divider()
          detail("Location") {
            Text(entry.path).font(.caption).textSelection(.enabled).fixedSize(
              horizontal: false, vertical: true)
          }
          Divider()
          detail("Measurement") {
            Text(
              "File allocation can differ from disk usage because of shared APFS blocks, snapshots, and inaccessible files."
            ).font(.caption).foregroundStyle(.secondary)
          }
          VStack(spacing: 8) {
            Button("Open in Finder", systemImage: "macwindow") { model.reveal() }.disabled(
              !entry.actionsAvailable || model.offline)
            Button("Copy Path", systemImage: "doc.on.doc") { model.copyPath() }.disabled(
              !entry.actionsAvailable)
            Divider().padding(.vertical, 3)
            Button("Move to Trash…", systemImage: "trash", role: .destructive) {
              model.requestTrash()
            }.disabled(!model.canTrash)
          }.buttonStyle(.bordered).frame(maxWidth: .infinity)
          if let reason = entry.trashBlockedReason {
            Text(reason).font(.caption).foregroundStyle(.secondary).fixedSize(
              horizontal: false, vertical: true)
          } else if model.isScanning {
            Text("Move to Trash is available after scanning.").font(.caption).foregroundStyle(
              .secondary)
          }
        } else {
          ContentUnavailableView(
            "No Selection", systemImage: "cursorarrow",
            description: Text("Select a file or folder to see its details."))
        }
      }.padding(20)
    }.background(Color(nsColor: .windowBackgroundColor))
  }
  private func detail<Content: View>(_ label: String, @ViewBuilder content: () -> Content)
    -> some View
  {
    VStack(alignment: .leading, spacing: 6) {
      Text(label).font(.caption2).foregroundStyle(.secondary)
      content()
    }
  }
}
private struct TrashConfirmationView: View {
  var model: DiskModel
  let reply: TrashReply
  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      Label("Move ‘\(reply.name ?? "this item")’ to Trash?", systemImage: "trash").font(
        .title3.weight(.semibold)
      ).fixedSize(horizontal: false, vertical: true)
      ScrollView {
        VStack(alignment: .leading, spacing: 14) {
          Text(reply.path ?? "").font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
          Text(
            "Scanned allocation: \(DiskFormat.bytes(reply.size ?? 0))\(reply.incomplete == true ? " (partial or snapshot)" : "")"
          ).font(.caption).foregroundStyle(.secondary)
          Text(
            reply.isDirectory == true
              ? "This folder and everything inside it will move to Trash. Its original location will no longer contain it."
              : "This file will move to Trash. Its original location will no longer contain it.")
          Text(
            "You can restore it from Trash in Finder. Disk space may not be freed until Trash is emptied."
          ).foregroundStyle(.secondary)
        }.frame(maxWidth: .infinity, alignment: .leading)
      }.frame(maxHeight: 220)
      HStack {
        Text("Escape cancels").font(.caption).foregroundStyle(.secondary)
        Spacer()
        Button("Cancel") { model.trashConfirmation = nil }.keyboardShortcut(.cancelAction)
        Button("Move to Trash", role: .destructive) { model.confirmTrash() }
      }
    }.padding(24).frame(width: 520).fixedSize(horizontal: false, vertical: true)
  }
}
private struct MessageView: View {
  let message: MessageSheet
  let close: () -> Void
  var body: some View {
    VStack(alignment: .leading, spacing: 18) {
      Label(message.title, systemImage: "exclamationmark.triangle").font(.headline)
      ScrollView {
        Text(message.message).textSelection(.enabled).frame(
          maxWidth: .infinity, alignment: .leading)
      }.frame(maxHeight: 220)
      HStack {
        Spacer()
        Button("OK", action: close).keyboardShortcut(.defaultAction)
      }
    }.padding(24).frame(width: 460).fixedSize(horizontal: false, vertical: true)
  }
}
private struct IssuesView: View {
  @Bindable var model: DiskModel
  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text("Incomplete Results").font(.title2.weight(.semibold))
      Text(
        "These locations contain entries TDisk couldn’t read or skipped after errors. File permissions and macOS privacy settings can affect access."
      ).foregroundStyle(.secondary)
      List(model.issues?.locations ?? []) { location in
        VStack(alignment: .leading, spacing: 4) {
          Text(location.path).font(.callout).textSelection(.enabled)
          Text("\(location.count) skipped or unreadable entries").font(.caption).foregroundStyle(
            .secondary)
        }
      }
      if model.issues?.omitted == true {
        Text("Showing the first 500 affected locations.").font(.caption).foregroundStyle(.secondary)
      }
      Text(
        "Full Disk Access can help with privacy restrictions, but doesn’t override every file permission or system protection."
      ).font(.caption).foregroundStyle(.secondary)
      HStack {
        Button("Privacy Settings…") { model.openPrivacySettings() }
        Spacer()
        Button("Rescan") {
          model.showIssues = false
          model.rescan()
        }.disabled(model.navigationLocked)
        Button("Done") { model.showIssues = false }.keyboardShortcut(.cancelAction)
      }
    }.padding(24).frame(width: 620, height: 470)
  }
}
private struct HelpView: View {
  @Environment(\.dismiss) private var dismiss
  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text("Using TDisk").font(.title2.weight(.semibold))
      ScrollView {
        VStack(alignment: .leading, spacing: 14) {
          Text(
            "Choose a folder or scan the startup disk. System disk usage appears first; folder sizes grow as TDisk discovers files. Exact folder sizes require traversal."
          )
          Text(
            "Double-click a folder to open it. Use Back and Forward to revisit locations, or Command–Up Arrow to go to the parent folder within this scan."
          )
          Grid(alignment: .leading, horizontalSpacing: 28, verticalSpacing: 9) {
            shortcut("⌘O", "Choose Folder")
            shortcut("⌘R", "Rescan")
            shortcut("⌘.", "Stop Scan")
            shortcut("⌘[ / ⌘]", "Back / Forward")
            shortcut("⌘↑ / ⌘↓", "Parent / Open Folder")
            shortcut("⌥⌘I", "Show Inspector")
            shortcut("⇧⌘R", "Open in Finder")
            shortcut("⌥⌘C", "Copy Path")
            shortcut("⌘⌫", "Move to Trash…")
          }.padding(.vertical, 6)
          Text(
            "Allocated size is the space attributed to regular files, not the volume’s physical usage. APFS shared blocks, snapshots, and unreadable files can make those numbers differ."
          ).foregroundStyle(.secondary)
          Text(
            "‘≥’ means a lower bound. ‘Limited access’ means partial results. Other mounted volumes and repeated filesystem identities are excluded from this scan."
          ).foregroundStyle(.secondary)
          Text(
            "Move to Trash always asks for confirmation. Important directory containers are protected; ordinary files and subfolders inside Library and other locations are eligible where macOS permits. Restore items using Finder. TDisk never permanently deletes items or empties Trash."
          ).foregroundStyle(.secondary)
        }
      }
      HStack {
        Spacer()
        Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
      }
    }.padding(24).frame(width: 620, height: 570)
  }
  private func shortcut(_ key: String, _ action: String) -> some View {
    GridRow {
      Text(key).font(.system(.body, design: .monospaced)).foregroundStyle(.secondary)
      Text(action)
    }
  }
}
