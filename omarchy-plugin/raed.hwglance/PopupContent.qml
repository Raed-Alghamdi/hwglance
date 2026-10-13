pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Contents of the HW Glance popup: header, key readings, top processes with
// End task / Force kill, and the inline confirmation. The owner passes in the
// collector client, the ui.json config, the kill controller and the palette.
// Text colours come from the palette (ink and muted, warn and critical picked
// to stay readable on the popup surface), never from opacity.
Item {
  id: content

  property var client: null
  property var config: Model.parseUi("")
  property var cards: []
  property var kill: null
  property var theme: null
  property var bar: null
  property bool active: false
  // Extra groups appended to the live ones. Used by the test harness to show
  // a dummy process; empty in the real shell.
  property var extraGroups: []

  signal openApp()
  signal openSettings()

  readonly property color fg: Color.popups.text
  readonly property color muted: theme ? theme.muted : Qt.darker(fg, 1.4)
  readonly property string fam: (bar && bar.fontFamily) ? bar.fontFamily : Style.font.family
  readonly property bool online: client ? client.connected : false

  // Process list state.
  property string sortKey: "cpu"
  property var rows: []
  property double lastSortMs: 0
  property var menuGroup: null
  readonly property bool menuOpen: menuGroup !== null
  // Row under the keyboard or pointer cursor, by group id so a re-sort cannot
  // move it onto another process. Arrows move it, Enter arms End on it.
  property string cursorId: ""
  property bool keyboardCursor: false
  readonly property bool holdOrder: listHover.hovered || keyboardCursor || menuOpen
    || (kill ? (kill.confirmationOpen || kill.armedId !== "") : false)

  readonly property int nameGap: Style.space(8)
  readonly property int cpuWidth: Style.space(52)
  readonly property int gpuWidth: Style.space(52)
  readonly property int memWidth: Style.space(60)
  readonly property int rowHeight: Style.space(28)
  // "End task" has to fit its button, whatever the font.
  readonly property int endPadding: Style.space(8)
  readonly property int endWidth: Math.max(Style.space(46), Math.ceil(endMetrics.advanceWidth) + endPadding * 2 + Style.space(4))
  // The button is a click target: at least 24 px tall.
  readonly property int endHeight: Math.max(24, Style.space(24))

  TextMetrics {
    id: endMetrics
    font.family: content.fam
    font.pixelSize: Style.font.caption
    text: "End task"
  }

  // The armed button reads "End <name>?" and is as wide as that needs.
  readonly property var armedGroup: (kill && kill.armedId !== "") ? findGroup(kill.armedId) : null
  readonly property int armedWidth: Math.max(endWidth, Math.ceil(armMetrics.advanceWidth) + endPadding * 2 + Style.space(4))

  TextMetrics {
    id: armMetrics
    font.family: content.fam
    font.pixelSize: Style.font.caption
    text: content.armedGroup ? Model.armLabel(content.armedGroup) : ""
  }

  // Secondary rows to reserve per card so all cards share one height.
  readonly property int cardSlots: {
    var most = 0
    for (var i = 0; i < cards.length; i++) most = Math.max(most, cards[i].ids.length - 1)
    return most
  }

  implicitHeight: column.implicitHeight

  function levelColor(level, base) {
    if (!theme) return base
    if (level >= 2) return theme.crit
    if (level === 1) return theme.warn
    return base
  }

  function liveGroups() {
    var live = (client && client.procGroups) ? client.procGroups : []
    return extraGroups.length > 0 ? live.concat(extraGroups) : live
  }

  // Every group now listed, or null when there is no process list (collector
  // away), so a kill cannot be told "ended" from "unknown".
  function procList() {
    if (!client || !client.procGroups) return null
    return liveGroups()
  }

  function findGroup(id) {
    var live = liveGroups()
    for (var i = 0; i < live.length; i++)
      if (live[i].id === id) return live[i]
    return null
  }

  // Rows follow the live numbers every tick, but their order only changes on
  // a re-sort: at most every 3 s, and never while the pointer is over the
  // list, a menu is open or a confirmation is pending.
  function refreshRows(force) {
    var live = liveGroups()
    if (live.length === 0) {
      if (rows.length > 0 && !client.connected) rows = []
      else if (rows.length > 0) rows = rows.map(function(r) { return Model.endedCopy(r) })
      return
    }
    var now = Date.now()
    // Also re-sort at once when the list is shorter than it could be, so the
    // first real process data replaces whatever arrived before it.
    var short = rows.length < Math.min(config.processCount, live.length)
    var resort = force || short || (!holdOrder && now - lastSortMs >= 3000)
    if (resort) {
      rows = Model.topGroups(live, sortKey, config.processCount)
      lastSortMs = now
      return
    }
    var byId = {}
    for (var i = 0; i < live.length; i++) byId[live[i].id] = live[i]
    rows = rows.map(function(r) { return byId[r.id] || Model.endedCopy(r) })
  }

  function openMenuAt(group, x, y) {
    menu.x = Math.max(0, Math.min(x, content.width - menu.width))
    menu.y = Math.max(0, Math.min(y, content.height - menu.height))
    menuGroup = group
  }

  function openMenu(group, item, mouse) {
    var p = item.mapToItem(content, mouse.x, mouse.y)
    openMenuAt(group, p.x, p.y)
  }

  function closeMenu() {
    menuGroup = null
  }

  // "End task" arms the row's End button, like clicking it; "Force kill"
  // always asks first.
  function menuAction(signal) {
    var g = menuGroup
    menuGroup = null
    if (!g || !kill) return
    var fresh = findGroup(g.id) || g
    cursorId = fresh.id
    if (signal === "TERM") kill.press(fresh)
    else kill.request(fresh, signal)
  }

  function rowIndexOf(id) {
    for (var i = 0; i < rows.length; i++)
      if (rows[i].id === id) return i
    return -1
  }

  // Keyboard cursor over the process rows. The first move lands on the first
  // row. Nothing moves while a confirmation is open: that is answered with the
  // mouse (or Esc).
  function moveCursor(dx, dy) {
    if (dy === 0 || rows.length === 0 || menuOpen || (kill && kill.confirmationOpen)) return
    var i = rowIndexOf(cursorId)
    if (i < 0) i = 0
    else i = Math.max(0, Math.min(rows.length - 1, i + (dy > 0 ? 1 : -1)))
    cursorId = rows[i].id
    keyboardCursor = true
  }

  // Enter, Space or x on the cursor row: the same as clicking its End button,
  // so the first press arms it and the second sends. A confirmation is never
  // answered from the keyboard, so a stray key cannot Force kill.
  function activateCursor() {
    if (!kill || cursorId === "" || menuOpen || kill.confirmationOpen) return
    var g = findGroup(cursorId)
    if (g) kill.press(g)
  }

  function hoverRow(group) {
    cursorId = group.id
    keyboardCursor = false
  }

  function reset() {
    rows = []
    lastSortMs = 0
    menuGroup = null
    cursorId = ""
    keyboardCursor = false
    if (kill) kill.reset()
  }

  // State is cleared when the popup opens, not when it closes, so the card
  // keeps its content (and size) while it fades out.
  onActiveChanged: {
    if (!active) return
    reset()
    sortKey = config.sort
  }
  onSortKeyChanged: if (active) refreshRows(true)
  Connections {
    target: content.client
    ignoreUnknownSignals: true
    function onSnapshotReceived() { if (content.active) content.refreshRows(false) }
  }

  onConfigChanged: if (active) refreshRows(true)

  Component.onCompleted: {
    if (kill) {
      kill.lookup = function(id) { return content.findGroup(id) }
      kill.liveGroups = function() { return content.procList() }
    }
  }

  Column {
    id: column
    width: parent.width
    spacing: Style.space(10)

    // ---------- Header ----------
    Item {
      width: parent.width
      implicitHeight: Math.max(titleText.implicitHeight, headerButtons.implicitHeight)

      Text {
        id: titleText
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: "HW Glance"
        color: content.fg
        font.family: content.fam
        font.pixelSize: Style.font.title
        font.bold: true
      }

      Row {
        id: headerButtons
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(6)

        Button {
          text: "Open"
          iconText: ""
          iconSize: Style.font.bodySmall
          fontSize: Style.font.bodySmall
          foreground: content.fg
          fontFamily: content.fam
          verticalPadding: Style.spacing.controlPaddingY
          bordered: true
          onClicked: content.openApp()
        }

        Button {
          iconText: ""
          iconSize: Style.font.bodySmall
          foreground: content.fg
          fontFamily: content.fam
          verticalPadding: Style.spacing.controlPaddingY
          horizontalPadding: Style.spacing.controlPaddingX
          bordered: true
          tooltipText: "HW Glance settings"
          onClicked: content.openSettings()
        }
      }
    }

    Text {
      visible: !content.online && !!content.client && content.client.gaveUp
      width: parent.width
      textFormat: Text.PlainText
      wrapMode: Text.WordWrap
      text: "The collector (hwglanced) is not running. Values show -- until it is back."
      color: content.levelColor(1, content.fg)
      font.family: content.fam
      font.pixelSize: Style.font.bodySmall
    }

    // ---------- Device cards ----------
    Grid {
      id: cardsGrid
      visible: content.cards.length > 0
      width: parent.width
      columns: Math.max(1, Math.min(3, content.cards.length))
      columnSpacing: Style.space(8)
      rowSpacing: Style.space(8)

      Repeater {
        model: content.cards.length

        DeviceCard {
          id: cardItem
          required property int index
          width: (cardsGrid.width - cardsGrid.columnSpacing * (cardsGrid.columns - 1)) / cardsGrid.columns
          card: content.cards[index]
          readings: content.client ? content.client.readings : ({})
          series: content.client ? content.client.series : ({})
          accent: content.theme ? content.theme.device(Model.deviceColorKey(card.key), content.fg) : content.fg
          fg: content.fg
          muted: content.muted
          warnColor: content.theme ? content.theme.warn : content.fg
          critColor: content.theme ? content.theme.crit : content.fg
          fam: content.fam
          online: content.online
          slots: content.cardSlots
        }
      }
    }

    PanelSeparator { foreground: content.fg }

    // ---------- Processes ----------
    Item {
      width: parent.width
      implicitHeight: Math.max(processHeader.implicitHeight, sortButtons.implicitHeight)

      PanelSectionHeader {
        id: processHeader
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: "TOP PROCESSES"
        foreground: content.fg
        color: content.muted
        fontFamily: content.fam
      }

      Row {
        id: sortButtons
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(4)

        Repeater {
          model: [
            { key: "cpu", label: "CPU" },
            { key: "gpu", label: "GPU" },
            { key: "mem", label: "Memory" }
          ]

          Button {
            required property var modelData
            text: modelData.label
            fontSize: Style.font.caption
            foreground: content.fg
            fontFamily: content.fam
            horizontalPadding: Style.space(8)
            verticalPadding: Style.space(3)
            bordered: true
            selected: content.sortKey === modelData.key
            onClicked: content.sortKey = modelData.key
          }
        }
      }
    }

    // Column captions.
    Item {
      width: parent.width
      implicitHeight: captionName.implicitHeight

      Text {
        id: captionName
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.leftMargin: Style.space(6)
        text: "Name"
        color: content.muted
        font.family: content.fam
        font.pixelSize: Style.font.caption
      }

      Text {
        textFormat: Text.PlainText
        x: parent.width - content.endWidth - content.nameGap - content.memWidth - content.gpuWidth - content.cpuWidth
        width: content.cpuWidth
        horizontalAlignment: Text.AlignRight
        text: "CPU"
        color: content.sortKey === "cpu" ? content.fg : content.muted
        font.family: content.fam
        font.pixelSize: Style.font.caption
      }

      Text {
        textFormat: Text.PlainText
        x: parent.width - content.endWidth - content.nameGap - content.memWidth - content.gpuWidth
        width: content.gpuWidth
        horizontalAlignment: Text.AlignRight
        text: "GPU"
        color: content.sortKey === "gpu" ? content.fg : content.muted
        font.family: content.fam
        font.pixelSize: Style.font.caption
      }

      Text {
        textFormat: Text.PlainText
        x: parent.width - content.endWidth - content.nameGap - content.memWidth
        width: content.memWidth
        horizontalAlignment: Text.AlignRight
        text: "Memory"
        color: content.sortKey === "mem" ? content.fg : content.muted
        font.family: content.fam
        font.pixelSize: Style.font.caption
      }
    }

    Item {
      id: list
      width: parent.width
      implicitHeight: content.rows.length > 0 ? listColumn.implicitHeight : content.rowHeight

      HoverHandler { id: listHover }

      Text {
        visible: content.rows.length === 0
        anchors.left: parent.left
        anchors.leftMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: content.online ? "Collecting process data..." : "No process data"
        color: content.muted
        font.family: content.fam
        font.pixelSize: Style.font.bodySmall
      }

      Column {
        id: listColumn
        width: parent.width

        Repeater {
          model: content.config.processCount

          Item {
            id: row
            required property int index
            readonly property var g: index < content.rows.length ? content.rows[index] : null
            readonly property bool ended: !!(g && g.ended)
            readonly property bool targeted: !!(g && content.kill && content.kill.pending && content.kill.pending.id === g.id)
            readonly property bool armedRow: !!(g && !ended && content.kill && content.kill.armedId === g.id)
            readonly property bool hasCursor: !!(g && content.cursorId === g.id)

            visible: g !== null
            width: listColumn.width
            height: g !== null ? content.rowHeight : 0

            // One highlight for pointer and keyboard (the shell's cursor model).
            CursorSurface {
              anchors.fill: parent
              hasCursor: row.hasCursor
              foreground: content.fg
            }

            // The row a confirmation or an armed End button is about.
            Rectangle {
              anchors.fill: parent
              radius: Style.cornerRadius
              color: "transparent"
              border.width: (row.targeted || row.armedRow) ? 1 : 0
              border.color: content.theme ? content.theme.crit : content.fg
            }

            MouseArea {
              id: rowMouse
              anchors.fill: parent
              hoverEnabled: true
              acceptedButtons: Qt.RightButton
              onContainsMouseChanged: if (containsMouse && row.g) content.hoverRow(row.g)
              onClicked: function(mouse) { if (row.g && !row.ended) content.openMenu(row.g, rowMouse, mouse) }
            }

            PanelToolTip {
              visible: rowMouse.containsMouse && row.g !== null && !content.menuOpen && !row.armedRow
              text: row.g ? Model.groupTooltip(row.g) : ""
              fontFamily: content.fam
            }

            Text {
              id: nameText
              textFormat: Text.PlainText
              anchors.left: parent.left
              anchors.leftMargin: Style.space(6)
              anchors.right: cpuText.left
              anchors.rightMargin: content.nameGap
              anchors.verticalCenter: parent.verticalCenter
              text: row.g ? (row.ended ? row.g.name + " ended" : Model.displayName(row.g)) : ""
              color: row.ended ? content.muted : content.fg
              elide: Text.ElideRight
              font.family: content.fam
              font.pixelSize: Style.font.bodySmall
            }

            Text {
              id: cpuText
              textFormat: Text.PlainText
              anchors.right: gpuText.left
              anchors.verticalCenter: parent.verticalCenter
              width: content.cpuWidth
              horizontalAlignment: Text.AlignRight
              text: row.g && !row.ended ? Model.percentText(row.g.cpu) : ""
              color: content.sortKey === "cpu" ? content.fg : content.muted
              font.family: content.fam
              font.pixelSize: Style.font.bodySmall
            }

            Text {
              id: gpuText
              textFormat: Text.PlainText
              anchors.right: memText.left
              anchors.verticalCenter: parent.verticalCenter
              width: content.gpuWidth
              horizontalAlignment: Text.AlignRight
              text: row.g && !row.ended ? Model.percentText(row.g.gpu) : ""
              color: content.sortKey === "gpu" ? content.fg : content.muted
              font.family: content.fam
              font.pixelSize: Style.font.bodySmall
            }

            Text {
              id: memText
              textFormat: Text.PlainText
              anchors.right: endSlot.left
              anchors.rightMargin: content.nameGap
              anchors.verticalCenter: parent.verticalCenter
              width: content.memWidth
              horizontalAlignment: Text.AlignRight
              text: row.g && !row.ended ? Model.compactBytes(row.g.mem) : ""
              color: content.sortKey === "mem" ? content.fg : content.muted
              font.family: content.fam
              font.pixelSize: Style.font.bodySmall
            }

            // Reserves the End column; an armed button grows leftward over the
            // numbers, which the row does not need while it asks.
            Item {
              id: endSlot
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              width: content.endWidth
              height: 1
            }

            // First click arms ("End <name>?" for 3 s), second click sends. A
            // group with a critical process asks in the box below instead.
            Button {
              id: endButton
              z: 2
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              width: row.armedRow ? Math.min(content.armedWidth, row.width * 0.7) : content.endWidth
              height: content.endHeight
              text: row.armedRow && row.g ? Model.armLabel(row.g) : "End task"
              fontSize: Style.font.caption
              foreground: row.armedRow && content.theme ? content.theme.crit : content.fg
              fontFamily: content.fam
              horizontalPadding: content.endPadding
              verticalPadding: Style.space(2)
              bordered: true
              active: row.armedRow
              visible: row.g !== null && !row.ended
              onHovered: function(isHovered) { if (isHovered && row.g) content.hoverRow(row.g) }
              onClicked: if (row.g && content.kill) content.kill.press(row.g)
            }
          }
        }
      }
    }

    // ---------- Confirmation ----------
    BorderSurface {
      id: confirmBox
      visible: !!(content.kill && content.kill.confirmationOpen)
      width: parent.width
      color: Util.alpha(content.theme ? content.theme.crit : content.fg, 0.1)
      borderSpec: Border.flat(content.theme ? content.theme.crit : content.fg, Math.max(1, Style.space(1)))
      padding: Style.space(10)
      radius: Style.cornerRadius
      implicitHeight: confirmColumn.implicitHeight + contentTopInset + contentBottomInset

      Column {
        id: confirmColumn
        x: confirmBox.contentLeftInset
        y: confirmBox.contentTopInset
        width: confirmBox.width - confirmBox.contentLeftInset - confirmBox.contentRightInset
        spacing: Style.space(8)

        Text {
          width: parent.width
          textFormat: Text.PlainText
          wrapMode: Text.WordWrap
          text: content.kill && content.kill.pending
            ? (content.kill.pending.signal === "KILL" ? "Force kill " : "End ")
              + "\"" + content.kill.pending.name + "\" ("
              + Model.pluralProcesses(content.kill.pending.count) + ")?"
            : ""
          color: content.fg
          font.family: content.fam
          font.pixelSize: Style.font.bodySmall
          font.bold: true
        }

        Text {
          visible: !!(content.kill && content.kill.pending && content.kill.pending.warning !== "")
          width: parent.width
          textFormat: Text.PlainText
          wrapMode: Text.WordWrap
          text: content.kill && content.kill.pending ? content.kill.pending.warning : ""
          color: content.theme ? content.theme.crit : content.fg
          font.family: content.fam
          font.pixelSize: Style.font.bodySmall
        }

        Text {
          visible: !!(content.kill && content.kill.pending && content.kill.pending.signal === "KILL")
          width: parent.width
          textFormat: Text.PlainText
          wrapMode: Text.WordWrap
          text: "It stops at once and cannot save its work."
          color: content.fg
          font.family: content.fam
          font.pixelSize: Style.font.bodySmall
        }

        Row {
          spacing: Style.space(8)

          Button {
            text: "Cancel"
            fontSize: Style.font.bodySmall
            foreground: content.fg
            fontFamily: content.fam
            bordered: true
            onClicked: if (content.kill) content.kill.cancel()
          }

          Button {
            text: content.kill && content.kill.pending && content.kill.pending.signal === "KILL" ? "Force kill" : "End task"
            fontSize: Style.font.bodySmall
            foreground: content.theme ? content.theme.crit : content.fg
            fontFamily: content.fam
            bordered: true
            onClicked: if (content.kill) content.kill.confirm()
          }
        }
      }
    }

    // ---------- Result ----------
    // What the last End task or Force kill came to; after an End task that left
    // processes running it offers Force kill.
    Item {
      visible: !!(content.kill && content.kill.status !== "")
      width: parent.width
      implicitHeight: Math.max(statusText.implicitHeight, forceButton.visible ? forceButton.implicitHeight : 0)

      Text {
        id: statusText
        anchors.left: parent.left
        anchors.right: forceButton.visible ? forceButton.left : parent.right
        anchors.rightMargin: forceButton.visible ? Style.space(8) : 0
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
        text: content.kill ? content.kill.status : ""
        color: content.kill && content.kill.failed ? content.levelColor(2, content.fg) : content.fg
        font.family: content.fam
        font.pixelSize: Style.font.bodySmall
      }

      Button {
        id: forceButton
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        visible: !!(content.kill && content.kill.canForce)
        text: "Force kill"
        fontSize: Style.font.caption
        foreground: content.theme ? content.theme.crit : content.fg
        fontFamily: content.fam
        horizontalPadding: content.endPadding
        verticalPadding: Style.space(2)
        height: content.endHeight
        bordered: true
        onClicked: {
          var g = content.kill ? content.findGroup(content.kill.forceId) : null
          if (g) content.kill.request(g, "KILL")
          else if (content.kill) content.kill.clearOutcome()
        }
      }
    }
  }

  // ---------- Right-click menu ----------
  MouseArea {
    anchors.fill: parent
    z: 20
    visible: content.menuOpen
    acceptedButtons: Qt.AllButtons
    onClicked: content.closeMenu()
  }

  BorderSurface {
    id: menu
    z: 21
    visible: content.menuOpen
    width: Style.space(150)
    height: menuColumn.implicitHeight + contentTopInset + contentBottomInset
    color: Color.popups.background
    borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(1)))
    padding: Style.space(4)
    radius: Style.cornerRadius

    Column {
      id: menuColumn
      x: menu.contentLeftInset
      y: menu.contentTopInset
      width: menu.width - menu.contentLeftInset - menu.contentRightInset
      spacing: Style.space(2)

      Button {
        width: parent.width
        leftAlign: true
        text: "End task"
        fontSize: Style.font.bodySmall
        foreground: content.fg
        fontFamily: content.fam
        onClicked: content.menuAction("TERM")
      }

      Button {
        width: parent.width
        leftAlign: true
        text: "Force kill"
        fontSize: Style.font.bodySmall
        foreground: content.theme ? content.theme.crit : content.fg
        fontFamily: content.fam
        onClicked: content.menuAction("KILL")
      }
    }
  }
}
