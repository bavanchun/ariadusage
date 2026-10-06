pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + inputBorder.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property string currentVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.value !== undefined)
    ? String(root.descriptor.kind.value)
    : ""
  readonly property string placeholderVal: (root.descriptor && root.descriptor.kind && root.descriptor.kind.placeholder)
    ? String(root.descriptor.kind.placeholder)
    : ""

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

  Rectangle {
    id: inputBorder
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    height: 26
    radius: 4
    color: Color.background
    border.width: 1
    border.color: textInput.activeFocus ? Color.accent : Color.muted

    TextInput {
      id: textInput
      anchors.fill: parent
      anchors.margins: 4
      text: root.currentVal
      color: Color.foreground
      font.pixelSize: 11
      clip: true
      onEditingFinished: {
        if (root.client && root.descriptor && textInput.text !== root.currentVal) {
          root.client.setSetting(root.descriptor.id, textInput.text)
        }
      }

      Text {
        anchors.fill: parent
        text: root.placeholderVal
        textFormat: Text.PlainText
        color: Color.muted
        font.pixelSize: 11
        visible: textInput.text === "" && !textInput.activeFocus
      }
    }
  }
}
