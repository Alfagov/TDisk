import SwiftUI

@main struct TDiskMacApp: App {
  @State private var model = DiskModel()
  var body: some Scene {
    Window("TDisk", id: "main") {
      ContentView(model: model)
    }
    .defaultSize(width: 1120, height: 700)
    .windowResizability(.contentMinSize)
    .commands {
      CommandGroup(replacing: .newItem) {
        Button("Choose Folder…") { model.chooseFolder() }.keyboardShortcut("o").disabled(
          model.navigationLocked)
        Button("Scan Startup Disk") { model.start("/") }.disabled(model.navigationLocked)
        Divider()
        Button("Rescan") { model.rescan() }.keyboardShortcut("r").disabled(
          model.rootPath == nil || model.navigationLocked || model.offline)
        Button("Stop Scan") { model.stop() }.keyboardShortcut(".").disabled(
          !model.isScanning || model.isStopping)
        Divider()
        Button("Move to Trash…") { model.requestTrash() }.keyboardShortcut(
          .delete, modifiers: .command
        ).disabled(
          model.selected == nil || model.isScanning || model.navigationLocked || model.offline)
      }
      CommandGroup(after: .pasteboard) {
        Button("Copy Path") { model.copyPath() }.keyboardShortcut(
          "c", modifiers: [.option, .command]
        ).disabled(model.selected?.actionsAvailable != true)
      }
      CommandGroup(after: .sidebar) {
        Button("Show/Hide Sidebar") {
          model.columnVisibility = model.columnVisibility == .detailOnly ? .all : .detailOnly
        }.keyboardShortcut("s", modifiers: [.control, .command])
        Toggle("Show Inspector", isOn: $model.inspectorVisible).keyboardShortcut(
          "i", modifiers: [.option, .command])
        Divider()
        Toggle("Show Share Column", isOn: $model.shareVisible)
        Toggle("Show Status Column", isOn: $model.statusVisible)
        Menu("Appearance") {
          Picker("Appearance", selection: $model.appearance) {
            Text("System").tag("system")
            Text("Light").tag("light")
            Text("Dark").tag("dark")
          }
        }
      }
      CommandMenu("Go") {
        Button("Back") { model.goBack() }.keyboardShortcut("[").disabled(!model.canGoBack)
        Button("Forward") { model.goForward() }.keyboardShortcut("]").disabled(!model.canGoForward)
        Button("Parent Folder") { model.goUp() }.keyboardShortcut(.upArrow, modifiers: .command)
          .disabled(!model.canGoUp)
        Button("Open Folder") { model.open() }.keyboardShortcut(.downArrow, modifiers: .command)
          .disabled(model.selected?.isDirectory != true || model.navigationLocked)
        Button("Scan Root") { model.goRoot() }.disabled(
          model.rootPath == nil || model.navigationLocked)
        Divider()
        Button("Open in Finder") { model.reveal() }.keyboardShortcut(
          "r", modifiers: [.shift, .command]
        ).disabled(model.selected?.actionsAvailable != true || model.offline)
      }
      CommandGroup(replacing: .help) {
        Button("TDisk Help") { model.showHelp = true }.keyboardShortcut("?", modifiers: .command)
      }
      CommandGroup(replacing: .appTermination) {
        Button("Quit TDisk") { NSApplication.shared.terminate(nil) }.keyboardShortcut("q").disabled(
          model.isTrashing)
      }
    }
  }
}
