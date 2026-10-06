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
  readonly property bool isSet: (root.descriptor && root.descriptor.kind && (root.descriptor.kind.isSet === true || root.descriptor.kind.is_set === true))
  readonly property string sourceName: (root.descriptor && root.descriptor.kind && root.descriptor.kind.source)
    ? String(root.descriptor.kind.source)
    : ""

  Row {
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    spacing: 12

    Column {
      id: labelCol
      width: parent.width - actionGroup.width - parent.spacing
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
      id: actionGroup
      spacing: 8
      anchors.verticalCenter: parent.verticalCenter

      Rectangle {
        width: 50
        height: 22
        radius: 3
        color: root.isSet ? Color.accent : Color.background
        border.width: 1
        border.color: root.isSet ? Color.accent : Color.muted

        Text {
          anchors.centerIn: parent
          text: root.isSet ? "Set" : "Not set"
          textFormat: Text.PlainText
          color: root.isSet ? Color.background : Color.muted
          font.pixelSize: 10
          font.bold: root.isSet
        }
      }

      Rectangle {
        width: 106
        height: 22
        radius: 3
        color: Color.background
        border.width: 1
        border.color: Color.muted

        Text {
          anchors.centerIn: parent
          text: "Set in terminal\u2026"
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 10
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (root.client && root.descriptor) {
              root.client.launchSecretSet(root.descriptor.id)
            }
          }
        }
      }
    }
  }
}
