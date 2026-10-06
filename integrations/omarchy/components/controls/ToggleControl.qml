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
  readonly property bool checked: (root.descriptor && root.descriptor.kind && root.descriptor.kind.value === true)

  Row {
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    spacing: 12

    Column {
      id: labelCol
      width: parent.width - switchTrack.width - parent.spacing
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

    Rectangle {
      id: switchTrack
      width: 38
      height: 20
      radius: 10
      color: root.checked ? Color.accent : Color.background
      border.width: 1
      border.color: root.checked ? Color.accent : Color.muted

      Rectangle {
        id: switchThumb
        width: 16
        height: 16
        radius: 8
        anchors.verticalCenter: parent.verticalCenter
        x: root.checked ? (switchTrack.width - width - 2) : 2
        color: root.checked ? Color.background : Color.foreground
      }

      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: {
          if (root.client && root.descriptor) {
            root.client.setSetting(root.descriptor.id, !root.checked)
          }
        }
      }
    }
  }
}
