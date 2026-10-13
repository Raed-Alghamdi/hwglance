import QtQuick
import Quickshell.Io
import qs.Commons
import "Model.js" as Model

// Text colours for the popup, picked from the active Omarchy palette so that
// every label reaches 4.5:1 on the popup surface: the secondary label colour,
// warn (yellow) and critical (red) text and the device accents. Each is the
// theme's own colour when it already reads well, else its bright variant, else
// a blend toward the popup text colour (Model.labelColors does the picking).
// colors.toml is re-read when a theme switch changes the shell palette, since
// a theme switch can swap the file rather than edit it in place.
Item {
  id: root

  width: 0
  height: 0
  visible: false

  // The theme's colour table (key -> "#rrggbb"); empty until colors.toml has
  // been read, when the defaults below stand in for it.
  property var table: ({})

  // Popup surface and text from the shell, so a theme's shell.toml overrides
  // are honoured. The strongest fill a label sits on is the hover fill.
  readonly property color surface: Color.popups.background
  readonly property color ink: Color.popups.text
  readonly property real fillAlpha: Math.max(Style.normalFillAlpha, Style.hoverFillAlpha)

  // A QML colour as the plain object Model understands (no string parsing).
  function plain(c) {
    return { r: c.r, g: c.g, b: c.b }
  }

  readonly property var colors: {
    var merged = {
      yellow: "#e0af68", blue: "#7aa2f7", magenta: "#ad8ee6", cyan: "#449dab"
    }
    merged.red = plain(Color.urgent)
    for (var k in table) merged[k] = table[k]
    return merged
  }

  // Output of Model.labelColors. If it cannot be computed the plain palette is
  // used, so the popup still has colours.
  readonly property var picked: {
    try {
      return Model.labelColors(colors, plain(ink), plain(surface), fillAlpha)
    } catch (e) {
      return { ink: ink, muted: ink, warn: colors.yellow, crit: Color.urgent,
               blue: colors.blue, magenta: colors.magenta, cyan: colors.cyan }
    }
  }

  // Secondary labels, captions and unavailable values.
  readonly property color muted: picked.muted
  readonly property color warn: picked.warn
  readonly property color crit: picked.crit
  readonly property color blue: picked.blue
  readonly property color magenta: picked.magenta
  readonly property color cyan: picked.cyan

  // Text colour for a card's colour key ("blue", "magenta", "cyan"), or the
  // given fallback for devices without one.
  function device(colorKey, fallback) {
    if (colorKey === "blue") return blue
    if (colorKey === "magenta") return magenta
    if (colorKey === "cyan") return cyan
    return fallback
  }

  FileView {
    id: file
    path: Color.currentThemePath + "/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: root.table = Model.parseColorsToml(text())
    onFileChanged: reload()
  }

  Timer {
    id: themeSettle
    interval: 500
    onTriggered: file.reload()
  }

  Connections {
    target: Color
    function onForegroundChanged() { themeSettle.restart() }
    function onBackgroundChanged() { themeSettle.restart() }
  }
}
