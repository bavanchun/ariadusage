pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + pathsCol.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property var pathsList: (root.descriptor && root.descriptor.kind && root.descriptor.kind.paths)
    ? root.descriptor.kind.paths
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

  Column {
    id: pathsCol
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 4

    Repeater {
      model: root.pathsList

      delegate: Row {
        id: pathRow
        required property var modelData
        required property int index

        readonly property string pathStr: String(pathRow.modelData || "")

        spacing: 8
        height: 22
        width: pathsCol.width

        Rectangle {
          width: pathRow.width - 28
          height: 20
          radius: 3
          color: Color.background
          border.width: 1
          border.color: Color.muted

          Text {
            anchors.fill: parent
            anchors.margins: 4
            text: pathRow.pathStr
            textFormat: Text.PlainText
            color: Color.foreground
            font.pixelSize: 10
            elide: Text.ElideMiddle
          }
        }

        Rectangle {
          width: 20
          height: 20
          radius: 3
          color: Color.background
          border.width: 1
          border.color: Color.muted

          Text {
            anchors.centerIn: parent
            text: "\u00d7"
            textFormat: Text.PlainText
            color: Color.urgent
            font.pixelSize: 12
            font.bold: true
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: {
              if (!root.client || !root.descriptor) return
              var updated = []
              for (var i = 0; i < root.pathsList.length; i++) {
                if (i !== pathRow.index) {
                  updated.push(root.pathsList[i])
                }
              }
              root.client.setSetting(root.descriptor.id, updated)
            }
          }
        }
      }
    }
  }
}
