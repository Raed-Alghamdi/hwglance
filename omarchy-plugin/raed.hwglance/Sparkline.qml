import QtQuick
import QtQuick.Shapes
import qs.Commons

// Small trend line with a faint fill below it. `points` is [[unix_ms, value]]
// oldest first; the x axis covers the `windowMs` ending at the newest point,
// the y axis fits the values with a minimum span so noise does not look like
// a swing.
Item {
  id: root

  property var points: []
  property color color: "white"
  property real windowMs: 120000
  property real minSpan: 5
  readonly property real inset: 2

  // [line points, fill polygon]
  readonly property var geometry: {
    var pts = root.points
    var w = root.width
    var h = root.height
    if (!pts || pts.length < 2 || w <= 0 || h <= 0) return [[], []]
    var tEnd = pts[pts.length - 1][0]
    var t0 = tEnd - root.windowMs
    var lo = Infinity
    var hi = -Infinity
    var inWindow = []
    for (var i = 0; i < pts.length; i++) {
      if (pts[i][0] < t0) continue
      inWindow.push(pts[i])
      lo = Math.min(lo, pts[i][1])
      hi = Math.max(hi, pts[i][1])
    }
    if (inWindow.length < 2) return [[], []]
    var nonNegative = lo >= 0
    if (hi - lo < root.minSpan) {
      var mid = (hi + lo) / 2
      lo = mid - root.minSpan / 2
      hi = mid + root.minSpan / 2
      if (nonNegative && lo < 0) {
        hi -= lo
        lo = 0
      }
    }
    var span = hi - lo
    var line = []
    var usable = h - root.inset * 2
    for (var j = 0; j < inWindow.length; j++) {
      var x = (inWindow[j][0] - t0) / root.windowMs * w
      var y = root.inset + usable - (inWindow[j][1] - lo) / span * usable
      line.push(Qt.point(x, y))
    }
    var fill = line.slice()
    fill.push(Qt.point(line[line.length - 1].x, h))
    fill.push(Qt.point(line[0].x, h))
    return [line, fill]
  }

  Shape {
    anchors.fill: parent
    antialiasing: true

    ShapePath {
      strokeColor: "transparent"
      strokeWidth: 0
      fillColor: Util.alpha(root.color, 0.16)
      PathPolyline { path: root.geometry[1] }
    }

    ShapePath {
      strokeColor: root.color
      strokeWidth: 1.5
      fillColor: "transparent"
      joinStyle: ShapePath.RoundJoin
      PathPolyline { path: root.geometry[0] }
    }
  }
}
