import QtQuick
import Quickshell.Io
import "Model.js" as Model

// Ends process groups. Owns the armed End button, the pending confirmation,
// the commands that run (signal TERM for End task, KILL for Force kill; plain
// `kill` for the user's own pids, `pkexec kill` for the rest) and the one-line
// result, which follows the processes afterwards: "<name> ended", or
// "<name> is still running" with Force kill offered. Commands are argv
// vectors, never a shell string, and run one after another.
Item {
  id: root

  width: 0
  height: 0
  visible: false

  property int uid: -1

  // The row whose End button was clicked once: { id, name }. A second click
  // within armMs sends; nothing else does. "" when no button is armed.
  property var armed: null
  readonly property string armedId: armed ? armed.id : ""
  readonly property int armMs: 3000
  // How long to wait for the processes to disappear, and how long an outcome
  // stays on screen. The window uses 3 s and 10 s; the process list here only
  // refreshes once a second, so the wait is a little longer.
  readonly property int watchMs: 4000
  readonly property int outcomeMs: 10000

  // { id, name, count, signal, warning } while waiting for the user to confirm.
  property var pending: null
  readonly property bool confirmationOpen: pending !== null
  readonly property bool running: queue.length > 0 || proc.running
  property string status: ""
  property bool failed: false

  property var queue: []
  property var current: null
  property var failures: []

  // The processes of the last End task or Force kill, watched until they are
  // gone or the wait runs out: { name, keys, signal, until }.
  property var watch: null
  // Set when End task left processes running: the group's id, so the popup can
  // offer Force kill. "" otherwise.
  property string forceId: ""
  readonly property bool canForce: forceId !== ""

  // Asks the owner for the freshest copy of a group (pids change between
  // snapshots); returns null when the group is gone.
  property var lookup: function(id) { return null }
  // Asks the owner for every process group now listed, or null when there is
  // no process list to look at.
  property var liveGroups: function() { return null }

  function describe(g) {
    return Model.displayName(g)
  }

  // The End button of a row, and "End task" in its menu. The first click arms
  // the button (it then reads "End <name>?" for armMs); a second click sends.
  // A group with critical processes skips the arming and asks in the
  // confirmation box instead, with a plain warning: never one click.
  function press(group) {
    if (!group || group.ended || running) return
    if (Model.criticalText(group) !== "") {
      request(group, "TERM")
      return
    }
    if (armed && armed.id === group.id) {
      var fresh = lookup(group.id) || group
      disarm()
      request(fresh, "TERM")
      return
    }
    clearOutcome()
    armed = { id: group.id, name: group.name }
    armTimer.restart()
  }

  function disarm() {
    armed = null
    armTimer.stop()
  }

  // Entry point for Force kill and for the confirmation. `signal` is "TERM" or
  // "KILL"; KILL always asks, TERM asks only for critical groups.
  function request(group, signal) {
    if (!group || running) return
    disarm()
    clearOutcome()
    var warning = Model.criticalText(group)
    if (signal === "KILL" || warning !== "") {
      pending = {
        id: group.id,
        name: group.name,
        count: group.n,
        signal: signal,
        warning: warning
      }
      return
    }
    run(group, signal)
  }

  function clearOutcome() {
    clearTimer.stop()
    watch = null
    forceId = ""
    status = ""
    failed = false
  }

  function cancel() {
    pending = null
    disarm()
  }

  function confirm() {
    if (!pending) return
    var p = pending
    pending = null
    var group = lookup(p.id)
    if (!group) {
      finish(false, p.name + " is no longer running")
      return
    }
    run(group, p.signal)
  }

  function run(group, signal) {
    if (uid < 0) {
      finish(true, "Cannot tell which user you are yet, try again")
      return
    }
    var plan = Model.killPlan(group, uid)
    if (plan.blocked !== "") {
      finish(true, plan.blocked)
      return
    }
    current = { id: group.id, name: group.name, count: group.n, signal: signal, keys: Model.pidKeys(group) }
    failures = []
    queue = Model.killCommands(plan, signal)
    status = (signal === "KILL" ? "Force killing " : "Ending ") + describe(group) + "..."
    failed = false
    next()
  }

  function next() {
    if (queue.length === 0) {
      conclude()
      return
    }
    var rest = queue.slice()
    proc.command = rest.shift()
    queue = rest
    proc.running = true
  }

  function onExit(code) {
    // Read stderr on the next turn so the collector has seen the pipe close.
    Qt.callLater(function() { handleExit(code, String(err.text || "").trim()) })
  }

  function handleExit(code, text) {
    if (code !== 0) {
      var firstLine = text.split("\n")[0]
      var gone = text !== "" && text.split("\n").every(function(line) { return /No such process/i.test(line) })
      if (!gone) {
        var reason = firstLine
        if (code === 126) reason = "Authentication cancelled"
        else if (code === 127) reason = "Authentication failed"
        else if (reason === "") reason = "exit code " + code
        failures = failures.concat([reason])
      }
    }
    next()
  }

  function conclude() {
    var c = current
    current = null
    if (failures.length > 0) {
      var reason = failures[0].replace(/^kill: /, "")
      finish(true, "Could not end " + c.name + ": " + reason)
      return
    }
    // The signal went out; now see whether the processes really go.
    status = "Sent " + (c.signal === "KILL" ? "Force kill" : "End task") + " to " + Model.pluralProcesses(c.count)
    failed = false
    watch = { id: c.id, name: c.name, keys: c.keys, signal: c.signal, until: Date.now() + watchMs }
    watchTimer.restart()
    clearTimer.restart()
    checkWatch()
  }

  // Compares the processes of the last kill with the collector's list: gone is
  // "<name> ended"; still there when the wait is over is "<name> is still
  // running", with Force kill offered after an End task.
  function checkWatch() {
    if (!watch) {
      watchTimer.stop()
      return
    }
    var alive = Model.anyAlive(liveGroups(), watch.keys)
    var w = watch
    if (alive === false) {
      watch = null
      watchTimer.stop()
      finish(false, w.name + " ended")
    } else if (Date.now() >= w.until) {
      watch = null
      watchTimer.stop()
      // With no process list to look at there is nothing to add to "Sent ...".
      if (alive === true) {
        if (w.signal === "TERM") forceId = w.id
        finish(true, w.name + " is still running")
      }
    }
  }

  function finish(isFailure, text) {
    failed = isFailure
    status = text
    clearTimer.restart()
  }

  function reset() {
    pending = null
    queue = []
    disarm()
    clearOutcome()
  }

  Process {
    id: proc
    stderr: StdioCollector {
      id: err
      waitForEnd: true
    }
    onExited: function(exitCode) { root.onExit(exitCode) }
  }

  Timer {
    id: clearTimer
    interval: root.outcomeMs
    onTriggered: root.clearOutcome()
  }

  Timer {
    id: armTimer
    interval: root.armMs
    onTriggered: root.armed = null
  }

  Timer {
    id: watchTimer
    interval: 500
    repeat: true
    onTriggered: root.checkWatch()
  }
}
