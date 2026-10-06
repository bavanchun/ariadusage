pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + previewRow.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property string colorVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.value)
    ? String(root.descriptor.kind.value)
    : "#cacccc"

  Column {
    id: labelCol
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
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
    id: previewRow
    anchors.left: parent.left
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 8

    Rectangle {
      id: colorSwatch
      width: 24
      height: 24
      radius: 4
      color: root.colorVal
      border.width: 1
      border.color: Color.muted
    }

    Rectangle {
      id: hexBox
      width: 80
      height: 24
      radius: 4
      color: Color.background
      border.width: 1
      border.color: hexInput.activeFocus ? Color.accent : Color.muted

      TextInput {
        id: hexInput
        anchors.fill: parent
        anchors.margins: 4
        text: root.colorVal
        color: Color.foreground
        font.pixelSize: 11
        onEditingFinished: {
          if (root.client && root.descriptor && hexInput.text !== root.colorVal) {
            root.client.setSetting(root.descriptor.id, hexInput.text)
          }
        }
      }
    }
  }
}
