import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// Talks to the hwglanced collector over its Unix socket (newline-delimited
// JSON). It is connected only while `active` (the popup is open): it
// subscribes with processes, asks for the recent history of the big values so
// the trend lines are filled at once, then appends each live value. While the
// daemon is not reachable it retries on a timer.
Item {
  id: root

  width: 0
  height: 0
  visible: false

  // Set by the consumer.
  property bool active: false
  property var wanted: ({})
  // Reading ids whose recent history is kept for the trend lines.
  property var historyIds: []
  property int historySeconds: 120

  // Results.
  readonly property bool connected: sock.connected && !stale
  property bool stale: false
  // True once a connection attempt has failed since the last connect.
  property bool gaveUp: false
  property int uid: -1
  property var readings: ({})
  property var sensors: []
  // The snapshot's limit_overrides: which readings have the user's limits, and whether "at or below".
  property var limits: ({})
  property var procGroups: null
  // id -> [[unix_ms, value], ...], oldest first, kept to historySeconds.
  property var series: ({})
  property double lastMessageMs: 0

  readonly property string runtimeDir: {
    var env = Quickshell.env("XDG_RUNTIME_DIR")
    if (env) return env
    return uid >= 0 ? "/run/user/" + uid : ""
  }
  // HWGLANCE_SOCKET overrides the path (used by the test harness only).
  readonly property string socketPath: Quickshell.env("HWGLANCE_SOCKET")
    || (runtimeDir ? runtimeDir + "/hwglance/daemon.sock" : "")

  signal snapshotReceived()

  function send(message) {
    if (!sock.connected) return
    sock.write(JSON.stringify(message) + "\n")
    sock.flush()
  }

  function requestHistory() {
    if (historyIds.length === 0) return
    send({ cmd: "history", ids: historyIds, seconds: historySeconds })
  }

  function tryConnect() {
    if (!active || socketPath === "" || sock.connected) return
    sock.connected = true
  }

  function clearData() {
    readings = ({})
    sensors = []
    procGroups = null
    series = ({})
  }

  function appendLive(unixMs) {
    var next = {}
    var cutoff = unixMs - (historySeconds + 10) * 1000
    for (var i = 0; i < historyIds.length; i++) {
      var id = historyIds[i]
      var old = series[id] || []
      var r = readings[id]
      var pts = old
      if (r && r.value !== null) {
        pts = old.filter(function(p) { return p[0] >= cutoff })
        pts.push([unixMs, r.value])
      }
      next[id] = pts
    }
    series = next
  }

  function mergeHistory(list) {
    var next = {}
    for (var id in series) next[id] = series[id]
    for (var i = 0; i < list.length; i++) {
      var s = list[i]
      var hist = Array.isArray(s.points) ? s.points : []
      var last = hist.length > 0 ? hist[hist.length - 1][0] : 0
      var live = (series[s.id] || []).filter(function(p) { return p[0] > last })
      next[s.id] = hist.concat(live)
    }
    series = next
  }

  function handleLine(line) {
    if (!line) return
    var msg
    try {
      msg = JSON.parse(line)
    } catch (e) {
      return
    }
    if (!msg) return
    lastMessageMs = Date.now()
    if (msg.type === "history") {
      mergeHistory(msg.series || [])
      return
    }
    // Other types (focus, error) are not used by the widget.
    if (msg.type !== "snapshot") return
    stale = false
    sensors = msg.sensors || []
    limits = msg.limit_overrides || {}
    readings = Model.extractReadings(sensors, wanted, limits)
    // Compacting here drops the large raw process payload right after this call.
    procGroups = msg.procs ? Model.compactGroups(msg.procs, msg.proto) : null
    appendLive(msg.unix_ms || Date.now())
    snapshotReceived()
  }

  onWantedChanged: if (sensors.length > 0) readings = Model.extractReadings(sensors, wanted, limits)

  onHistoryIdsChanged: requestHistory()

  onActiveChanged: {
    if (active) {
      clearData()
      gaveUp = false
      tryConnect()
    } else if (sock.connected) {
      sock.connected = false
    }
  }

  onSocketPathChanged: tryConnect()

  Process {
    command: ["id", "-u"]
    running: true
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var n = parseInt(text, 10)
        if (isFinite(n)) root.uid = n
      }
    }
  }

  Socket {
    id: sock
    path: root.socketPath

    parser: SplitParser {
      splitMarker: "\n"
      onRead: function(data) { root.handleLine(data) }
    }

    onConnectedChanged: {
      if (connected) {
        root.lastMessageMs = Date.now()
        root.stale = false
        root.gaveUp = false
        root.send({ cmd: "subscribe", procs: true })
        root.requestHistory()
      } else {
        root.procGroups = null
      }
    }

    onError: root.gaveUp = true
  }

  // Retry quietly while the daemon is down (service restart, not started yet).
  Timer {
    interval: 3000
    repeat: true
    running: root.active && !sock.connected
    onTriggered: root.tryConnect()
  }

  // A connected daemon that stops talking is treated as gone: drop the
  // connection and let the retry timer reopen it.
  Timer {
    interval: 2000
    repeat: true
    running: sock.connected
    onTriggered: {
      if (Date.now() - root.lastMessageMs > 6000) {
        root.stale = true
        sock.connected = false
      }
    }
  }
}
