pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var provider: null
  property double nowMs: Date.now()

  implicitWidth: 360
  implicitHeight: root.errorMessage !== "" && root.metricState === "error" ? 64 : 48

  readonly property var primaryMetric: (root.provider && root.provider.windows && root.provider.windows.primary) ? root.provider.windows.primary : null
  readonly property string metricState: root.primaryMetric ? String(root.primaryMetric.state || "unknown") : "unknown"
  readonly property real usedPercent: (root.primaryMetric && root.primaryMetric.value && root.primaryMetric.value.usedPercent !== undefined)
    ? Number(root.primaryMetric.value.usedPercent)
    : 0.0

  readonly property string displayName: root.provider ? String(root.provider.displayName || root.provider.id || "") : ""
  readonly property string resetDescription: (root.primaryMetric && root.primaryMetric.value && root.primaryMetric.value.resetDescription)
    ? String(root.primaryMetric.value.resetDescription)
    : ""

  readonly property string errorMessage: (root.provider && root.provider.lastError && root.provider.lastError.message)
    ? String(root.provider.lastError.message)
    : ""

  readonly property string updatedAgeText: {
    if (!root.provider || !root.provider.updatedAt) return ""
    var d = Date.parse(root.provider.updatedAt)
    if (isNaN(d)) return ""
    var elapsedSec = Math.max(0, Math.floor((root.nowMs - d) / 1000.0))
    if (elapsedSec < 60) return "updated " + elapsedSec + "s ago"
    var elapsedMin = Math.floor(elapsedSec / 60)
    if (elapsedMin < 60) return "updated " + elapsedMin + "m ago"
    var elapsedHour = Math.floor(elapsedMin / 60)
    return "updated " + elapsedHour + "h ago"
  }

  Rectangle {
    id: bg
    anchors.fill: parent
    radius: 6
    color: Color.background
    opacity: 0.5
    border.width: 1
    border.color: Color.muted
  }

  Row {
    id: mainRow
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.margins: 10
    spacing: 12

    Column {
      id: infoCol
      width: mainRow.width - metricView.width - mainRow.spacing - 10
      spacing: 3

      Row {
        spacing: 8
        Text {
          text: root.displayName
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 13
          font.bold: true
        }

        Text {
          text: root.updatedAgeText
          textFormat: Text.PlainText
          color: Color.muted
          font.pixelSize: 11
          visible: root.updatedAgeText !== ""
        }
      }

      Text {
        text: root.resetDescription
        textFormat: Text.PlainText
        color: Color.muted
        font.pixelSize: 11
        visible: root.resetDescription !== "" && root.metricState !== "error"
      }

      Text {
        text: root.errorMessage
        textFormat: Text.PlainText
        color: Color.urgent
        font.pixelSize: 11
        visible: root.errorMessage !== "" && root.metricState === "error"
        elide: Text.ElideRight
        width: infoCol.width
      }
    }

    MetricView {
      id: metricView
      anchors.verticalCenter: parent.verticalCenter
      metricState: root.metricState
      usedPercent: root.usedPercent
      compact: false
      foregroundColor: Color.foreground
    }
  }
}
