import AppKit
import Foundation

@main struct NativeSmoke {
  @MainActor static func main() async throws {
    _ = NSApplication.shared
    let fixture = FileManager.default.temporaryDirectory.appendingPathComponent(
      "tdisk-native-smoke-" + UUID().uuidString)
    let folder = fixture.appendingPathComponent("Library – 日本語")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: fixture) }
    let file = folder.appendingPathComponent("keep.txt")
    try Data(repeating: 65, count: 16_384).write(to: file)
    let backend = ScanBackend()
    let token = UUID()
    try await backend.start(path: fixture.path, generation: token)
    var snapshot: ScanSnapshot?
    for _ in 0..<500 {
      let response = try await backend.snapshot(generation: token, folder: "", revision: 0)
      snapshot = response
      if response.status != "scanning" { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    let root = snapshot!
    precondition(root.status == "complete" && root.folder?.size ?? 0 >= 16_384)
    precondition(root.volume != nil)
    let entry = root.children!.first!
    precondition(entry.name == "Library – 日本語")
    let unchanged = try await backend.snapshot(
      generation: token, folder: root.rootID, revision: root.listingRevision)
    precondition(unchanged.children == nil)
    let child = try await backend.snapshot(generation: token, folder: entry.id, revision: 0)
    precondition(child.children!.first!.name == "keep.txt" && child.breadcrumbs.count == 2)
    let protected = try await backend.prepareTrash(generation: token, id: root.rootID)
    precondition(!protected.ok && protected.error != nil)
    let prepared = try await backend.prepareTrash(generation: token, id: entry.id)
    precondition(prepared.ok && FileManager.default.fileExists(atPath: file.path))
    let replacement = UUID()
    try await backend.start(path: folder.path, generation: replacement)
    do {
      _ = try await backend.snapshot(generation: token, folder: "", revision: 0)
      preconditionFailure("Old scan generation must be rejected")
    } catch is CancellationError {}
    await backend.stop(generation: replacement)

    let model = DiskModel()
    model.start(fixture.path)
    for _ in 0..<500 {
      if model.snapshot?.status == "complete" { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    precondition(model.rows.count == 1 && !model.isScanning)
    let parentSelection = model.rows[0].id
    model.selection = parentSelection
    model.open()
    for _ in 0..<500 {
      if model.rows.first?.name == "keep.txt" { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    precondition(model.rows.first?.name == "keep.txt" && model.canGoBack && model.canGoUp)
    model.goBack()
    for _ in 0..<500 {
      if model.rows.first?.id == parentSelection { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    precondition(model.selection == parentSelection && model.canGoForward)
    model.requestTrash()
    for _ in 0..<500 {
      if model.trashConfirmation != nil { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    precondition(model.trashConfirmation?.ok == true && model.navigationLocked)
    model.trashConfirmation = nil
    precondition(FileManager.default.fileExists(atPath: file.path) && !model.navigationLocked)
    model.handleUnmount(fixture)
    precondition(model.offline)
    model.handleMount(fixture)
    precondition(!model.offline && model.notice != nil)
    model.start(fixture.appendingPathComponent("missing").path)
    for _ in 0..<500 {
      if model.snapshot?.status == "failed" { break }
      try await Task.sleep(for: .milliseconds(10))
    }
    precondition(model.snapshot?.status == "failed" && !model.isScanning)
    print(
      "Native smoke checks passed: bridge, allocation, Unicode paths, generations, navigation, selection restoration, Trash preparation/cancellation, failed scan."
    )
  }
}
