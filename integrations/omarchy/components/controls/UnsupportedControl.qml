pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: 28

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""

  Rectangle {
    anchors.fill: parent
    radius: 4
    color: Color.background
    border.width: 1
    border.color: Color.muted
    opacity: 0.6

    Text {
      anchors.centerIn: parent
      text: "unsupported setting " + root.labelText
      textFormat: Text.PlainText
      color: Color.muted
      font.pixelSize: 11
    }
  }
}
