import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// Live CPU and GPU usage for the bar button, read from the collector's
// summary.json. The daemon replaces that file (temp file plus rename) on every
// sample (once a second by default), which can drop an inotify watch, so it is
// reloaded on a timer (every 2 s) instead. Nothing runs while `active` is false.
Item {
  id: root

  property bool active: false

  width: 0
  height: 0
  visible: false

  // HWGLANCE_SUMMARY overrides the location (used by the test harness only).
  readonly property string path: Quickshell.env("HWGLANCE_SUMMARY") || (Quickshell.env("XDG_RUNTIME_DIR") + "/hwglance/summary.json")
  // cpu and gpu are null and fresh is false while the data is missing or stale.
  property var summary: Model.parseSummary("", 0)

  function apply(raw) {
    var next = Model.parseSummary(raw, Date.now())
    // Keep the same object while nothing changed so bindings do not re-run.
    if (next.fresh === summary.fresh && next.cpu === summary.cpu && next.gpu === summary.gpu) return
    summary = next
  }

  onActiveChanged: if (!active) summary = Model.parseSummary("", 0)

  FileView {
    id: file
    path: root.active ? root.path : ""
    printErrors: false
    onLoaded: root.apply(text())
    onLoadFailed: root.apply("")
  }

  Timer {
    interval: 2000
    repeat: true
    running: root.active
    triggeredOnStart: true
    onTriggered: file.reload()
  }
}
