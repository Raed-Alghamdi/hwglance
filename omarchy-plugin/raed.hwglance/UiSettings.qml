import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// Read-only view of ~/.config/hwglance/ui.json, which the HW Glance app owns.
// Reloads when the file changes. While the file is missing (the app has not
// written one yet) it is polled slowly so a later first write is picked up
// even though there was nothing to watch.
Item {
  id: root

  width: 0
  height: 0
  visible: false

  // HWGLANCE_UI overrides the location (used by the test harness only).
  readonly property string path: Quickshell.env("HWGLANCE_UI") || (Quickshell.env("HOME") + "/.config/hwglance/ui.json")
  property var config: Model.parseUi("")
  property bool missing: true
  property var lastText: null

  function apply(raw, isMissing) {
    missing = isMissing
    // Skip identical content so a poll or a touch does not re-trigger bindings.
    if (raw === lastText) return
    lastText = raw
    config = Model.parseUi(raw)
  }

  FileView {
    id: file
    path: root.path
    watchChanges: true
    printErrors: false
    onLoaded: root.apply(text(), false)
    onLoadFailed: root.apply("", true)
    // text() is stale inside the change signal itself, so reload and read in onLoaded.
    onFileChanged: reload()
  }

  Timer {
    interval: 10000
    repeat: true
    running: root.missing
    onTriggered: file.reload()
  }
}
