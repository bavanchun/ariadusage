pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property string metricState: "unknown"
  property real usedPercent: 0.0
  property color foregroundColor: Color.foreground
  property bool compact: true

  implicitWidth: compact ? 36 : 56
  implicitHeight: compact ? 16 : 24

  readonly property color effectiveColor: {
    if (root.metricState === "error") return Color.urgent
    if (root.metricState === "stale") return Color.muted
    if (root.metricState === "unknown") return Color.muted
    if (root.metricState === "loading") return root.foregroundColor
    return root.foregroundColor
  }

  readonly property string displayText: {
    if (root.metricState === "value") {
      return Math.round(root.usedPercent) + "%"
    }
    if (root.metricState === "stale") {
      return "~" + Math.round(root.usedPercent) + "%"
    }
    if (root.metricState === "loading") {
      return "\u2026"
    }
    if (root.metricState === "error") {
      return "!"
    }
    return "?"
  }

  Text {
    id: label
    anchors.fill: parent
    text: root.displayText
    textFormat: Text.PlainText
    color: root.effectiveColor
    horizontalAlignment: Text.AlignHCenter
    verticalAlignment: Text.AlignVCenter
    font.pixelSize: root.compact ? 11 : 13
    font.bold: root.metricState === "error" || root.metricState === "value"
    opacity: root.metricState === "loading" ? 0.65 : 1.0
  }
}
