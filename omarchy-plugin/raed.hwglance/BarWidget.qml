pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons
import qs.Ui
import "Model.js" as Model

// HW Glance bar widget: a static icon, or live CPU and/or GPU usage when the
// app's Settings ask for it (tray.bar_display). Clicking it opens a popup with
// three device cards (CPU, GPU, Memory) and the top processes. The collector
// socket is connected only while the popup is open; the bar values come from
// the summary file the collector rewrites, read only in the value modes.
// Settings come from the HW Glance app's ~/.config/hwglance/ui.json (read only).
Panel {
  id: root

  moduleName: "raed.hwglance"
  ipcTarget: "raed.hwglance"

  // Test hook: groups appended to the process list (empty in the real shell).
  property var extraGroups: []
  // Handles for the test harness.
  property alias popup: content
  property alias killer: kill
  property alias collector: client

  readonly property var config: ui.config
  readonly property var cards: Model.buildCards(config.popupValues)
  // The window binary: hwglance from PATH (a package install), else the per-user install.
  readonly property string windowBinary: "\"$(command -v hwglance || echo \"$HOME/.local/bin/hwglance\")\""
  readonly property string openCommand: "omarchy-launch-or-focus hwglance " + windowBinary
  readonly property string settingsCommand: windowBinary + " --settings"

  function openApp() {
    if (root.bar) root.bar.run(openCommand)
  }

  function openSettings() {
    if (root.bar) root.bar.run(settingsCommand)
  }

  function handleEscape() {
    if (content.menuOpen) content.closeMenu()
    else if (kill.confirmationOpen || kill.armedId !== "") kill.cancel()
    else root.close()
  }

  readonly property string barDisplay: config.barDisplay
  // Vertical bars keep the icon: a text label does not fit their width.
  readonly property bool barVertical: root.bar ? root.bar.vertical : false
  readonly property var barParts: Model.barParts(barDisplay, feed.summary)
  readonly property bool showValues: !barVertical && barParts.length > 0
  readonly property Item barButton: showValues ? valueButton : iconButton

  implicitWidth: barButton.implicitWidth
  implicitHeight: barButton.implicitHeight

  onOpenedChanged: client.active = opened

  UiSettings { id: ui }

  ThemeColors { id: theme }

  SummaryFeed {
    id: feed
    active: root.barDisplay !== "icon" && !root.barVertical
  }

  HwClient {
    id: client
    wanted: Model.wantedSet(root.config)
    historyIds: Model.bigIds(root.cards)
  }

  KillController {
    id: kill
    uid: client.uid
  }

  function handlePress(b) {
    if (b === Qt.RightButton) root.openApp()
    else if (b === Qt.LeftButton) root.toggle()
  }

  // Sizes the value button once for the widest text of the mode.
  TextMetrics {
    id: sample
    font.family: valueButton.fontFamily
    font.pixelSize: valueButton.fontSize
    text: Model.barSample(root.barDisplay)
  }

  BarIconButton {
    id: iconButton
    anchors.fill: parent
    visible: !root.showValues
    interactive: visible
    bar: root.bar
    text: ""
    tooltipText: Model.barTooltip(root.barDisplay, feed.summary)
    onPressed: function(b) { root.handlePress(b) }
  }

  // Same label look as the clock: body font, bar foreground, 8.75 px margins.
  // The width is fixed so changing values do not move the neighbouring widgets.
  WidgetButton {
    id: valueButton
    anchors.fill: parent
    visible: root.showValues
    interactive: visible
    bar: root.bar
    text: Model.barText(root.barParts)
    horizontalMargin: 8.75
    fixedWidth: Math.ceil(sample.advanceWidth + scaledHorizontalMargin * 2)
    tooltipText: Model.barTooltip(root.barDisplay, feed.summary)
    onPressed: function(b) { root.handlePress(b) }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.barButton
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    centerOnBar: false
    contentWidth: panel.fittedContentWidth(Style.space(580))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onMoveRequested: function(dx, dy) { content.moveCursor(dx, dy) }
      onActivateRequested: content.activateCursor()
      onDeleteRequested: content.activateCursor()
      onCloseRequested: root.handleEscape()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      PopupContent {
        id: content
        width: parent.width
        client: client
        config: root.config
        cards: root.cards
        kill: kill
        theme: theme
        bar: root.bar
        active: root.opened
        extraGroups: root.extraGroups
        onOpenApp: { root.openApp(); root.close() }
        onOpenSettings: { root.openSettings(); root.close() }
      }
    }
  }
}
