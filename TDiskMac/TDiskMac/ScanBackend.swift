import Foundation

struct DiskEntry: Identifiable, Decodable, Sendable, Equatable {
  let id: String
  let name: String
  let path: String
  var size: UInt64
  var isDirectory: Bool
  var pending: Bool
  var errors: UInt64
  var skipReason: String?
  var trashBlockedReason: String?
  let actionsAvailable: Bool
  var safeName: String {
    name.unicodeScalars.map { CharacterSet.controlCharacters.contains($0) ? "�" : String($0) }
      .joined()
  }
}
struct DiskVolume: Decodable, Sendable {
  let mount: String
  let used: UInt64
  let available: UInt64
  let total: UInt64
}
struct ScanSnapshot: Decodable, Sendable {
  let rootID: String
  let rootPath: String
  let status: String
  let error: String?
  let volume: DiskVolume?
  let folder: DiskEntry?
  let breadcrumbs: [DiskEntry]
  let listingRevision: UInt64
  let children: [DiskEntry]?
  let updates: [DiskEntry]
  let files: UInt64
  let directories: UInt64
  let elapsedMilliseconds: UInt64
  let totalsStale: Bool
  enum CodingKeys: String, CodingKey {
    case rootID = "rootId"
    case rootPath, status, error, volume, folder, breadcrumbs, listingRevision, children, updates,
      files, directories, elapsedMilliseconds, totalsStale
  }
}
struct TrashReply: Decodable, Sendable, Identifiable {
  let ok: Bool
  let error: String?
  let id: String?
  let name: String?
  let path: String?
  let size: UInt64?
  let incomplete: Bool?
  let isDirectory: Bool?
}
struct ScanIssues: Decodable, Sendable {
  struct Location: Decodable, Sendable, Identifiable {
    let path: String
    let count: UInt64
    var id: String { path }
  }
  let locations: [Location]
  let omitted: Bool
}
private final class ScanHandle: @unchecked Sendable {
  // The backend actor is the only caller; Rust owns the handle and returned strings.
  let pointer: UnsafeMutableRawPointer
  init(path: String) throws {
    guard let pointer = path.withCString({ tdisk_scan_create($0) }) else {
      throw BackendError.message("The scanner couldn't start. Please try again.")
    }
    self.pointer = pointer
  }
  deinit {
    tdisk_scan_cancel(pointer)
    tdisk_scan_free(pointer)
  }
}
enum BackendError: LocalizedError {
  case message(String)
  var errorDescription: String? {
    if case .message(let value) = self { return value }
    return nil
  }
}
actor ScanBackend {
  private var handle: ScanHandle?
  private var generation: UUID?
  private func decode<T: Decodable>(_ pointer: UnsafeMutablePointer<CChar>?) throws -> T {
    guard let pointer else {
      throw BackendError.message("The scanner returned an invalid response. Please rescan.")
    }
    defer { tdisk_string_free(pointer) }
    return try JSONDecoder().decode(T.self, from: Data(bytes: pointer, count: strlen(pointer)))
  }
  func start(path: String, generation: UUID) throws {
    handle = nil
    self.generation = generation
    handle = try ScanHandle(path: path)
  }
  private func current(_ generation: UUID) throws -> ScanHandle {
    guard self.generation == generation, let handle else { throw CancellationError() }
    return handle
  }
  func snapshot(generation: UUID, folder: String, revision: UInt64) throws -> ScanSnapshot {
    let handle = try current(generation)
    return try decode(folder.withCString { tdisk_scan_snapshot(handle.pointer, $0, revision) })
  }
  func stop(generation: UUID) {
    guard let handle = try? current(generation) else { return }
    tdisk_scan_cancel(handle.pointer)
  }
  func prepareTrash(generation: UUID, id: String) throws -> TrashReply {
    let handle = try current(generation)
    return try decode(id.withCString { tdisk_trash_prepare(handle.pointer, $0) })
  }
  func moveToTrash(generation: UUID) throws -> TrashReply {
    let handle = try current(generation)
    return try decode(tdisk_trash_execute(handle.pointer))
  }
  func issues(generation: UUID) throws -> ScanIssues {
    let handle = try current(generation)
    return try decode(tdisk_scan_issues(handle.pointer))
  }
}
