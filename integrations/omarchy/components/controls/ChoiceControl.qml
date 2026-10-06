pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + optionsRow.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property string selectedId: (root.descriptor && root.descriptor.kind && root.descriptor.kind.selected)
    ? String(root.descriptor.kind.selected)
    : ""
  readonly property var optionsList: (root.descriptor && root.descriptor.kind && root.descriptor.kind.options)
    ? root.descriptor.kind.options
    : []

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
    id: optionsRow
    anchors.left: parent.left
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 6

    Repeater {
      model: root.optionsList

      delegate: Rectangle {
        id: optItem
        required property var modelData
        required property int index

        readonly property string optId: String(optItem.modelData.id || "")
        readonly property string optLabel: String(optItem.modelData.label || optItem.optId)
        readonly property bool isSelected: optItem.optId === root.selectedId

        width: Math.max(64, optText.implicitWidth + 16)
        height: 24
        radius: 4
        color: optItem.isSelected ? Color.accent : Color.background
        border.width: 1
        border.color: optItem.isSelected ? Color.accent : Color.muted

        Text {
          id: optText
          anchors.centerIn: parent
          text: optItem.optLabel
          textFormat: Text.PlainText
          color: optItem.isSelected ? Color.background : Color.foreground
          font.pixelSize: 11
          font.bold: optItem.isSelected
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (root.client && root.descriptor) {
              root.client.setSetting(root.descriptor.id, optItem.optId)
            }
          }
        }
      }
    }
  }
}
