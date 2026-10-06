pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: helpText.visible ? 44 : 32

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property real numValue: (root.descriptor && root.descriptor.kind && root.descriptor.kind.value !== undefined)
    ? Number(root.descriptor.kind.value)
    : 0.0
  readonly property real minVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.min !== undefined)
    ? Number(root.descriptor.kind.min)
    : 0.0
  readonly property real maxVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.max !== undefined)
    ? Number(root.descriptor.kind.max)
    : 100000.0
  readonly property real stepVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.step !== undefined)
    ? Number(root.descriptor.kind.step)
    : 1.0
  readonly property string unitText: (root.descriptor && root.descriptor.kind && root.descriptor.kind.unit)
    ? String(root.descriptor.kind.unit)
    : ""

  Row {
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    spacing: 12

    Column {
      id: labelCol
      width: parent.width - stepperRow.width - parent.spacing
      spacing: 2

      Text {
        text: root.labelText
        textFormat: Text.PlainText
        color: Color.foreground
        font.pixelSize: 12
        elide: Text.ElideRight
        width: parent.width
      }

      Text {
        id: helpText
        text: root.helpString
        textFormat: Text.PlainText
        color: Color.muted
        font.pixelSize: 10
        visible: root.helpString !== ""
        elide: Text.ElideRight
        width: parent.width
      }
    }

    Row {
      id: stepperRow
      spacing: 6
      anchors.verticalCenter: parent.verticalCenter

      Rectangle {
        id: decBtn
        width: 24
        height: 24
        radius: 4
        color: Color.background
        border.width: 1
        border.color: Color.muted

        Text {
          anchors.centerIn: parent
          text: "-"
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 14
          font.bold: true
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (!root.client || !root.descriptor) return
            var next = Math.max(root.minVal, root.numValue - root.stepVal)
            root.client.setSetting(root.descriptor.id, next)
          }
        }
      }

      Rectangle {
        id: valBox
        width: 56
        height: 24
        radius: 4
        color: Color.background
        border.width: 1
        border.color: Color.muted

        Text {
          anchors.centerIn: parent
          text: root.numValue + (root.unitText !== "" ? (" " + root.unitText) : "")
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 11
        }
      }

      Rectangle {
        id: incBtn
        width: 24
        height: 24
        radius: 4
        color: Color.background
        border.width: 1
        border.color: Color.muted

        Text {
          anchors.centerIn: parent
          text: "+"
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 14
          font.bold: true
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (!root.client || !root.descriptor) return
            var next = Math.min(root.maxVal, root.numValue + root.stepVal)
            root.client.setSetting(root.descriptor.id, next)
          }
        }
      }
    }
  }
}
