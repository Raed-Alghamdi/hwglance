pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons
import qs.Ui
import "Model.js" as Model

// One device card: name in the device colour, the big value, a trend line of
// it, and up to four secondary rows. Values take the warn and critical colours
// from their thresholds and also carry the window's letters (H and HH for high,
// L and LL for low), so the state does not rest on colour. Labels use the
// muted colour, never opacity. Hovering lists every value with a short
// description.
BorderSurface {
  id: root

  // { key, ids } from Model.buildCards.
  property var card: ({ key: "", ids: [] })
  property var readings: ({})
  property var series: ({})
  property color accent: "white"
  property color fg: "white"
  property color muted: "gray"
  property color warnColor: "yellow"
  property color critColor: "red"
  property string fam: "monospace"
  property bool online: false
  // Secondary-row slots to reserve so cards in one row share a height.
  property int slots: 3

  readonly property string bigId: card.ids.length > 0 ? card.ids[0] : ""
  readonly property var bigReading: readings[bigId]
  readonly property var throttle: readings[card.key + "/throttle/thermal"]
  readonly property bool throttled: online && throttle !== undefined && throttle.value !== null && throttle.value >= 0.5
  readonly property int rowHeight: Style.space(18)

  function levelColor(reading) {
    var level = Model.readingLevel(reading)
    if (level >= 2) return root.critColor
    if (level === 1) return root.warnColor
    return root.fg
  }

  function levelMark(reading) {
    return root.hasValue(reading) ? Model.readingMark(reading) : ""
  }

  function hasValue(reading) {
    return root.online && reading !== undefined && reading.value !== null
  }

  padding: Style.space(10)
  radius: Style.cornerRadius
  color: Style.normalFill
  borderSpec: Border.flat(Util.alpha(root.accent, 0.45), Math.max(1, Style.space(1)))
  implicitHeight: column.implicitHeight + contentTopInset + contentBottomInset

  Column {
    id: column
    x: root.contentLeftInset
    y: root.contentTopInset
    width: root.width - root.contentLeftInset - root.contentRightInset
    spacing: Style.space(4)

    // Title and throttling tag.
    Item {
      width: parent.width
      implicitHeight: Math.max(titleText.implicitHeight, tag.height)

      Text {
        id: titleText
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: Model.deviceTitle(root.card.key, root.bigReading)
        color: root.accent
        font.family: root.fam
        font.pixelSize: Style.font.subtitle
        font.bold: true
      }

      Rectangle {
        id: tag
        visible: root.throttled
        anchors.left: titleText.right
        anchors.leftMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        width: tagText.implicitWidth + Style.space(10)
        height: tagText.implicitHeight + Style.space(2)
        radius: Style.cornerRadius
        color: Util.alpha(root.critColor, 0.16)
        border.width: 1
        border.color: Util.alpha(root.critColor, 0.7)

        Text {
          id: tagText
          textFormat: Text.PlainText
          anchors.centerIn: parent
          text: "Throttling"
          color: root.critColor
          font.family: root.fam
          font.pixelSize: Style.font.caption
          font.bold: true
        }
      }
    }

    // Big value and its label.
    Item {
      width: parent.width
      implicitHeight: bigText.implicitHeight

      Text {
        id: bigText
        textFormat: Text.PlainText
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        text: root.hasValue(root.bigReading) ? Model.formatCompact(root.bigReading.value, root.bigReading.unit) : "--"
        color: root.hasValue(root.bigReading) ? root.levelColor(root.bigReading) : root.muted
        font.family: root.fam
        font.pixelSize: Style.font.display
        font.bold: true
      }

      Text {
        id: bigMark
        textFormat: Text.PlainText
        anchors.left: bigText.right
        anchors.leftMargin: Style.space(4)
        anchors.baseline: bigText.baseline
        text: root.levelMark(root.bigReading)
        color: root.levelColor(root.bigReading)
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
        font.bold: true
      }

      Text {
        textFormat: Text.PlainText
        anchors.right: parent.right
        anchors.baseline: bigText.baseline
        text: Model.shortLabel(root.bigId, root.bigReading)
        color: root.muted
        font.family: root.fam
        font.pixelSize: Style.font.bodySmall
      }
    }

    Sparkline {
      width: parent.width
      height: Style.space(30)
      color: root.accent
      points: root.series[root.bigId] || []
      minSpan: root.bigReading && root.bigReading.unit === "celsius" ? 5 : 10
    }

    Item {
      width: parent.width
      height: Style.space(4)
    }

    // Secondary rows.
    Item {
      width: parent.width
      height: root.slots * root.rowHeight

      Column {
        width: parent.width

        Repeater {
          model: Math.max(0, root.card.ids.length - 1)

          Item {
            id: row
            required property int index
            readonly property string rid: root.card.ids[index + 1]
            readonly property var reading: root.readings[rid]
            readonly property bool live: root.hasValue(reading)

            width: parent.width
            height: root.rowHeight

            Text {
              textFormat: Text.PlainText
              anchors.left: parent.left
              anchors.right: markText.left
              anchors.rightMargin: Style.space(6)
              anchors.verticalCenter: parent.verticalCenter
              text: Model.shortLabel(row.rid, row.reading)
              color: root.muted
              elide: Text.ElideRight
              font.family: root.fam
              font.pixelSize: Style.font.bodySmall
            }

            Text {
              id: valueText
              textFormat: Text.PlainText
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              text: row.live ? Model.formatFull(row.reading.value, row.reading.unit) : "--"
              color: row.live ? root.levelColor(row.reading) : root.muted
              font.family: root.fam
              font.pixelSize: Style.font.bodySmall
            }

            // H, HH, L or LL beside a value past a threshold.
            Text {
              id: markText
              textFormat: Text.PlainText
              anchors.right: valueText.left
              anchors.rightMargin: text !== "" ? Style.space(4) : 0
              anchors.verticalCenter: parent.verticalCenter
              text: root.levelMark(row.reading)
              color: root.levelColor(row.reading)
              font.family: root.fam
              font.pixelSize: Style.font.caption
              font.bold: true
            }
          }
        }
      }
    }
  }

  MouseArea {
    id: hover
    anchors.fill: parent
    hoverEnabled: true
    acceptedButtons: Qt.NoButton
  }

  PanelToolTip {
    visible: hover.containsMouse
    text: Model.cardTooltip(root.card, root.readings)
    fontFamily: root.fam
  }
}
