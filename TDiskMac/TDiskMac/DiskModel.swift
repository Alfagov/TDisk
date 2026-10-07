import AppKit
import Observation
import SwiftUI

struct DiskLocation: Identifiable, Sendable, Hashable {
  let path: String
  let name: String
  let symbol: String
  var id: String { path }
  static func devices() -> [DiskLocation] {
    let root = URL(fileURLWithPath: "/")
    let name = (try? root.resourceValues(forKeys: [.volumeNameKey]).volumeName) ?? "Startup Disk"
    var locations = [DiskLocation(path: "/", name: name, symbol: "internaldrive")]
    for url in FileManager.default.mountedVolumeURLs(
      includingResourceValuesForKeys: [.volumeNameKey, .volumeIsInternalKey],
      options: [.skipHiddenVolumes]) ?? [] where url.path.hasPrefix("/Volumes/")
    {
      let values = try? url.resourceValues(forKeys: [.volumeNameKey, .volumeIsInternalKey])
      locations.append(
        DiskLocation(
          path: url.path, name: values?.volumeName ?? url.lastPathComponent,
          symbol: values?.volumeIsInternal == true ? "internaldrive" : "externaldrive"))
    }
    return locations
  }
}
struct NavigationPosition {
  var id: String
  var selection: String?
}
struct MessageSheet: Identifiable {
  let id = UUID()
  let title: String
  let message: String
}
@MainActor @Observable final class DiskModel {
  private let backend = ScanBackend()
  private var generation = UUID()
  private var polling: Task<Void, Never>?
  private var listingRevision: UInt64 = 0
  private var rowIndices: [String: Int] = [:]
  private var history: [NavigationPosition] = []
  private var historyIndex = 0
  var snapshot: ScanSnapshot?
  var rows: [DiskEntry] = []
  var selection: String?
  var folderID = ""
  var rootPath: String?
  var rootLabel = "TDisk"
  var locationSelection: String?
  var devices: [DiskLocation] = []
  var recents: [DiskLocation] = []
  var sortOrder = [
    KeyPathComparator(\DiskEntry.size, order: .reverse), KeyPathComparator(\DiskEntry.name),
  ]
  var columnVisibility: NavigationSplitViewVisibility = .all
  var inspectorVisible = UserDefaults.standard.object(forKey: "showInspector") as? Bool ?? true {
    didSet { UserDefaults.standard.set(inspectorVisible, forKey: "showInspector") }
  }
  var shareVisible = UserDefaults.standard.object(forKey: "showShare") as? Bool ?? true {
    didSet { UserDefaults.standard.set(shareVisible, forKey: "showShare") }
  }
  var statusVisible = UserDefaults.standard.object(forKey: "showStatus") as? Bool ?? true {
    didSet { UserDefaults.standard.set(statusVisible, forKey: "showStatus") }
  }
  var appearance = UserDefaults.standard.string(forKey: "appearance") ?? "system" {
    didSet { UserDefaults.standard.set(appearance, forKey: "appearance") }
  }
  var trashConfirmation: TrashReply?
  var messageSheet: MessageSheet?
  var issues: ScanIssues?
  var showIssues = false
  var showHelp = false
  var isTrashing = false
  var isPreparingTrash = false
  var isStopping = false
  var offline = false
  var notice: String?
  var startError: String?
  var windowWidth: CGFloat = 1120
  var isScanning: Bool {
    rootPath != nil && startError == nil && (snapshot == nil || snapshot?.status == "scanning")
  }
  var navigationLocked: Bool { isTrashing || isPreparingTrash || trashConfirmation != nil }
  var selected: DiskEntry? {
    selection.flatMap { rowIndices[$0].flatMap { rows.indices.contains($0) ? rows[$0] : nil } }
  }
  var canGoBack: Bool { historyIndex > 0 && !navigationLocked }
  var canGoForward: Bool { historyIndex + 1 < history.count && !navigationLocked }
  var canGoUp: Bool { (snapshot?.breadcrumbs.count ?? 0) > 1 && !navigationLocked }
  var canTrash: Bool {
    !isScanning && !navigationLocked && !offline && selected?.trashBlockedReason == nil
      && selected != nil
  }
  var folderName: String {
    folderID == snapshot?.rootID || folderID.isEmpty
      ? rootLabel : (snapshot?.folder?.safeName ?? "Folder")
  }
  var favorites: [DiskLocation] {
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    return [
      DiskLocation(path: home, name: "Home", symbol: "house"),
      DiskLocation(path: home + "/Documents", name: "Documents", symbol: "doc"),
      DiskLocation(path: home + "/Downloads", name: "Downloads", symbol: "arrow.down.circle"),
    ]
  }
  init() {
    let paths = UserDefaults.standard.stringArray(forKey: "recentLocations") ?? []
    recents = paths.map {
      DiskLocation(path: $0, name: URL(fileURLWithPath: $0).lastPathComponent, symbol: "folder")
    }
    refreshDevices()
  }
  func refreshDevices() {
    Task { devices = await Task.detached(priority: .utility) { DiskLocation.devices() }.value }
  }
  func chooseFolder() {
    guard !navigationLocked else { return }
    let panel = NSOpenPanel()
    panel.title = "Choose a Folder to Scan"
    panel.prompt = "Scan"
    panel.canChooseFiles = false
    panel.canChooseDirectories = true
    panel.allowsMultipleSelection = false
    if let path = snapshot?.folder?.path { panel.directoryURL = URL(fileURLWithPath: path) }
    if let window = NSApp.keyWindow {
      panel.beginSheetModal(for: window) { [weak self] result in
        guard result == .OK, let url = panel.url else { return }
        self?.start(url.path)
      }
    } else if panel.runModal() == .OK, let url = panel.url {
      start(url.path)
    }
  }
  func start(_ path: String) {
    guard !navigationLocked else { return }
    polling?.cancel()
    generation = UUID()
    let token = generation
    rootPath = path
    rootLabel =
      devices.first(where: { $0.path == path })?.name
      ?? (path == "/" ? "Startup Disk" : FileManager.default.displayName(atPath: path))
    locationSelection = path
    folderID = ""
    selection = nil
    snapshot = nil
    rows = []
    rowIndices = [:]
    listingRevision = 0
    history = []
    historyIndex = 0
    offline = false
    isStopping = false
    notice = nil
    startError = nil
    let favoritePaths = Set(favorites.map(\.path))
    if path != "/" && !favoritePaths.contains(path) && !devices.contains(where: { $0.path == path })
    {
      recents.removeAll { $0.path == path }
      recents.insert(DiskLocation(path: path, name: rootLabel, symbol: "folder"), at: 0)
      recents = Array(recents.prefix(8))
      UserDefaults.standard.set(recents.map(\.path), forKey: "recentLocations")
    }
    polling = Task {
      do {
        guard token == generation, !Task.isCancelled else { return }
        try await backend.start(path: path, generation: token)
        guard token == generation, !Task.isCancelled else { return }
        await poll(token)
      } catch is CancellationError {} catch {
        if token == generation {
          startError = error.localizedDescription
          showError(error.localizedDescription)
          await backend.stop(generation: token)
        }
      }
    }
  }
  private func poll(_ token: UUID) async {
    do {
      repeat {
        let requestedFolder = folderID
        let response = try await backend.snapshot(
          generation: token, folder: requestedFolder, revision: listingRevision)
        guard token == generation, !Task.isCancelled else { return }
        // Navigation can change while a background snapshot is being decoded.
        if requestedFolder == folderID { apply(response) }
        if !isScanning { break }
        try await Task.sleep(for: .milliseconds(200))
      } while !Task.isCancelled
    } catch is CancellationError {} catch {
      if token == generation {
        startError = error.localizedDescription
        showError(error.localizedDescription)
        await backend.stop(generation: token)
      }
    }
  }
  private func apply(_ response: ScanSnapshot) {
    let wasScanning = isScanning
    snapshot = response
    if folderID.isEmpty {
      folderID = response.rootID
      history = [NavigationPosition(id: folderID, selection: nil)]
      historyIndex = 0
    }
    if let children = response.children {
      rows = children
      if listingRevision == 0, history.indices.contains(historyIndex) {
        selection = history[historyIndex].selection
      }
      sortRows()
    } else {
      for entry in response.updates { if let index = rowIndices[entry.id] { rows[index] = entry } }
    }
    listingRevision = response.listingRevision
    if wasScanning && response.status != "scanning" {
      isStopping = false
      sortRows()
      if let window = NSApp.mainWindow {
        NSAccessibility.post(
          element: window, notification: .announcementRequested,
          userInfo: [
            .announcement: response.status == "stopped"
              ? "Scan stopped. Partial results available." : "Scan finished.",
            .priority: NSAccessibilityPriorityLevel.medium.rawValue,
          ])
      }
    }
    if let selection, rowIndices[selection] == nil { self.selection = nil }
  }
  func sortRows() {
    rows.sort(using: sortOrder)
    rowIndices = Dictionary(uniqueKeysWithValues: rows.enumerated().map { ($1.id, $0) })
  }
  func navigate(_ id: String, restoring: Bool = false) {
    guard !navigationLocked, id != folderID else { return }
    if history.indices.contains(historyIndex) { history[historyIndex].selection = selection }
    if !restoring {
      history = Array(history.prefix(historyIndex + 1))
      history.append(NavigationPosition(id: id, selection: nil))
      historyIndex = history.count - 1
    }
    folderID = id
    listingRevision = 0
    selection = nil
    rows = []
    rowIndices = [:]
    refreshCurrentFolder()
  }
  private func refreshCurrentFolder() {
    let token = generation
    let id = folderID
    Task {
      do {
        let response = try await backend.snapshot(generation: token, folder: id, revision: 0)
        guard token == generation, id == folderID else { return }
        apply(response)
        if response.status == "scanning", polling == nil { resumePolling() }
      } catch is CancellationError {} catch { showError(error.localizedDescription) }
    }
  }
  func open(_ entry: DiskEntry? = nil) {
    if let entry = entry ?? selected, entry.isDirectory { navigate(entry.id) }
  }
  func goBack() {
    guard canGoBack else { return }
    let target = historyIndex - 1
    navigate(history[target].id, restoring: true)
    historyIndex = target
  }
  func goForward() {
    guard canGoForward else { return }
    let target = historyIndex + 1
    navigate(history[target].id, restoring: true)
    historyIndex = target
  }
  func goUp() {
    if canGoUp, let crumb = snapshot?.breadcrumbs.dropLast().last { navigate(crumb.id) }
  }
  func goRoot() { if let root = snapshot?.rootID { navigate(root) } }
  func stop() {
    guard isScanning, !isStopping else { return }
    isStopping = true
    let token = generation
    Task { await backend.stop(generation: token) }
  }
  func rescan() { if let rootPath, !navigationLocked { start(rootPath) } }
  func reveal(_ entry: DiskEntry? = nil) {
    guard !offline, let entry = entry ?? selected, entry.actionsAvailable else { return }
    NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: entry.path)])
  }
  func copyPath(_ entry: DiskEntry? = nil) {
    guard let entry = entry ?? selected, entry.actionsAvailable else { return }
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(entry.path, forType: .string)
  }
  func requestTrash(_ entry: DiskEntry? = nil) {
    guard !navigationLocked, !isScanning, !offline, let entry = entry ?? selected else { return }
    let token = generation
    isPreparingTrash = true
    Task {
      defer { isPreparingTrash = false }
      do {
        let reply = try await backend.prepareTrash(generation: token, id: entry.id)
        guard token == generation else { return }
        if reply.ok {
          trashConfirmation = reply
        } else {
          messageSheet = MessageSheet(
            title: "Move to Trash Unavailable", message: reply.error ?? "This item is protected.")
        }
      } catch { showError(error.localizedDescription) }
    }
  }
  func confirmTrash() {
    guard trashConfirmation != nil, !isTrashing else { return }
    trashConfirmation = nil
    isTrashing = true
    polling?.cancel()
    polling = nil
    let token = generation
    Task {
      defer { isTrashing = false }
      do {
        let result = try await backend.moveToTrash(generation: token)
        guard token == generation else { return }
        if result.ok {
          selection = nil
          notice = "Moved to Trash. Restore it using Finder. Rescan to refresh folder sizes."
          listingRevision = 0
          refreshCurrentFolder()
        } else {
          showError(result.error ?? "macOS couldn't move this item to Trash.")
        }
      } catch { showError(error.localizedDescription) }
    }
  }
  func loadIssues() {
    let token = generation
    Task {
      do {
        issues = try await backend.issues(generation: token)
        if token == generation { showIssues = true }
      } catch { showError(error.localizedDescription) }
    }
  }
  func openPrivacySettings() {
    NSWorkspace.shared.open(
      URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!)
  }
  func removeRecent(_ path: String) {
    recents.removeAll { $0.path == path }
    UserDefaults.standard.set(recents.map(\.path), forKey: "recentLocations")
  }
  func handleMount(_ url: URL?) {
    refreshDevices()
    guard offline, let mount = url?.path, let rootPath,
      rootPath == mount || rootPath.hasPrefix(mount + "/")
    else { return }
    offline = false
    notice = "This disk is available again. Rescan to refresh its contents."
  }
  func handleUnmount(_ url: URL?) {
    refreshDevices()
    guard let mount = url?.path, let rootPath, rootPath == mount || rootPath.hasPrefix(mount + "/")
    else { return }
    offline = true
    notice = "This disk is no longer available. These are the last results from this session."
    stop()
  }
  private func resumePolling() {
    let token = generation
    polling?.cancel()
    polling = Task { await poll(token) }
  }
  func showError(_ message: String) {
    messageSheet = MessageSheet(title: "TDisk", message: message)
  }
}
@MainActor enum DiskFormat {
  static func bytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(clamping: bytes), countStyle: .decimal)
  }
  static func size(_ entry: DiskEntry, scanning: Bool) -> String {
    if entry.skipReason != nil { return "—" }
    if entry.pending {
      return entry.size > 0 ? "≥ " + bytes(entry.size) : (scanning ? "Measuring…" : "Not scanned")
    }
    return bytes(entry.size)
  }
  static func status(_ entry: DiskEntry, scanning: Bool) -> String {
    if let reason = entry.skipReason {
      return reason == "other volume" ? "Separate volume" : "Counted elsewhere"
    }
    if entry.pending { return scanning ? "Scanning" : "Not scanned" }
    return entry.errors > 0 ? "Limited access" : "Complete"
  }
}
