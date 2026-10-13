import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import "plugin"

// Test harness: loads the widget in a thin bottom bar (on HWG_SCREEN, default HDMI-A-1) with a mock
// `bar` object (same surface as the facade the real bar injects). A scripted
// timeline opens the popup and drives the kill flow against two dummy
// `sleep` processes, so no real process is ever touched. Never installed.
ShellRoot {
  id: harness

  readonly property string screenName: Quickshell.env("HWG_SCREEN") || "HDMI-A-1"
  readonly property string scenario: Quickshell.env("HWG_SCENARIO") || "full"
  property int step: 0

  QtObject {
    id: mockBar
    property color foreground: "#a9b1d6"
    property color barForeground: "#a9b1d6"
    property color background: "#1a1b26"
    property color urgent: "#f7768e"
    property string fontFamily: "JetBrainsMono Nerd Font"
    property string position: "bottom"
    property bool vertical: Quickshell.env("HWG_VERTICAL") === "1"
    property int barSize: 26
    property bool foregroundAnimationEnabled: false
    property var activePopout: null
    property var clickTargets: []
    property var shell: null

    function run(cmd) { console.log("BAR.RUN " + cmd) }
    function shellQuote(v) { return "'" + String(v).replace(/'/g, "'\\''") + "'" }
    function showTooltip(target, text) { console.log("TOOLTIP " + text.replace(/\n/g, " | ")) }
    function hideTooltip(target) {}
    function registerClickTarget(t) { clickTargets = clickTargets.concat([t]) }
    function unregisterClickTarget(t) { clickTargets = clickTargets.filter(function(x) { return x !== t }) }
    function requestPopout(owner) { activePopout = owner }
    function releasePopout(owner) { if (activePopout === owner) activePopout = null }
    function switchPanelFrom(owner, direction) { return false }
    function targetBelongsToWindow(target, window) { return true }
  }

  Process { id: dummyA; command: ["sleep", "600"]; running: true }
  Process { id: dummyB; command: ["sleep", "601"]; running: true }

  function dummyGroups() {
    return [
      {
        id: "app:hwg-dummy", cls: "app", name: "hwg-dummy", title: "Dummy window for the kill test",
        n: 1, cpu: 55.5, gpu: 12.0, mem: 123456789, pids: [dummyA.processId], pidUids: [1000],
        critical: [], cmd: "sleep 600", ended: false
      },
      {
        id: "app:hwg-fake-hyprland", cls: "app", name: "Hyprland (fake)", title: "",
        n: 1, cpu: 44.0, gpu: null, mem: 99000000, pids: [dummyB.processId], pidUids: [1000],
        critical: ["session", "audio"], cmd: "sleep 601", ended: false
      }
    ]
  }

  PanelWindow {
    id: win
    screen: Quickshell.screens.find(function(s) { return s.name === harness.screenName }) || null
    anchors { bottom: true; left: true; right: true }
    implicitHeight: 26
    exclusionMode: ExclusionMode.Ignore
    color: "#1a1b26"
    WlrLayershell.layer: WlrLayer.Top
    WlrLayershell.namespace: "hwglance-harness"
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.None

    Item {
      anchors.fill: parent

      BarWidget {
        id: widget
        bar: mockBar
        x: 220
        anchors.verticalCenter: parent.verticalCenter
        extraGroups: harness.scenario === "nodummy" ? [] : harness.dummyGroups()
      }
    }
  }

  function groupById(id) {
    var list = harness.dummyGroups()
    for (var i = 0; i < list.length; i++) if (list[i].id === id) return list[i]
    return null
  }

  // Timeline, one entry per second.
  function act(n) {
    console.log("STEP " + n)
    var pop = widget.popup
    var killer = widget.killer
    switch (n) {
      case 1: console.log("DUMMY " + dummyA.processId + " " + dummyB.processId); widget.open(); break
      case 3: pop.openMenuAt(widget.popup.rows.length > 0 ? widget.popup.rows[0] : null, 120, 260); break
      case 4: pop.closeMenu(); killer.request(groupById("app:hwg-dummy"), "KILL"); break
      case 5: killer.cancel(); killer.request(groupById("app:hwg-fake-hyprland"), "TERM"); break
      case 6: killer.cancel(); killer.request(groupById("app:hwg-dummy"), "TERM"); break
      case 8: killer.request(groupById("app:hwg-fake-hyprland"), "KILL"); break
      case 9: killer.confirm(); break
      case 10: killer.run({ id: "app:hwg-perm", name: "perm-test", n: 1, pids: [1], pidUids: [1000], critical: [] }, "TERM"); break
      case 12: widget.close(); break
      case 13: Qt.quit(); break
    }
  }

  Timer {
    interval: 1000
    repeat: true
    running: harness.scenario !== "bar"
    onTriggered: { harness.step += 1; harness.act(harness.step) }
  }

  Timer {
    interval: Number(Quickshell.env("HWG_QUIT_MS") || 5000)
    running: harness.scenario === "bar"
    onTriggered: Qt.quit()
  }

  // Bar scenario: reports what the button shows so bar-test.sh can check it.
  Timer {
    interval: Number(Quickshell.env("HWG_REPORT_MS") || 2500)
    repeat: true
    running: harness.scenario === "bar"
    onTriggered: console.log("BARBUTTON mode=" + widget.barDisplay + " values=" + widget.showValues
      + " text=[" + widget.barButton.text + "] width=" + widget.width
      + " tooltip=[" + widget.barButton.tooltipText.replace(/\n/g, " | ") + "]")
  }
}
