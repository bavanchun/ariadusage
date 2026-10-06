pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + entriesCol.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property var entriesList: (root.descriptor && root.descriptor.kind && root.descriptor.kind.entries)
    ? root.descriptor.kind.entries
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
    id: entriesCol
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 4

    Repeater {
      model: root.entriesList

      delegate: Row {
        id: entryRow
        required property var modelData
        required property int index

        readonly property string entryId: String(entryRow.modelData.id || "")
        readonly property string entryLabel: String(entryRow.modelData.label || entryRow.entryId)
        readonly property bool isLocked: entryRow.modelData.locked === true
        readonly property bool isEnabled: entryRow.modelData.enabled === true

        spacing: 8
        height: 20

        Rectangle {
          id: checkSquare
          width: 14
          height: 14
          anchors.verticalCenter: parent.verticalCenter
          radius: 3
          color: entryRow.isEnabled ? Color.accent : Color.background
          border.width: 1
          border.color: entryRow.isEnabled ? Color.accent : Color.muted
          opacity: entryRow.isLocked ? 0.5 : 1.0

          Text {
            anchors.centerIn: parent
            text: "\u2713"
            textFormat: Text.PlainText
            color: Color.background
            font.pixelSize: 10
            visible: entryRow.isEnabled
          }

          MouseArea {
            anchors.fill: parent
            enabled: !entryRow.isLocked
            cursorShape: entryRow.isLocked ? Qt.ArrowCursor : Qt.PointingHandCursor
            onClicked: {
              if (!root.client || !root.descriptor) return
              var updated = []
              for (var i = 0; i < root.entriesList.length; i++) {
                var it = root.entriesList[i]
                if (it.id === entryRow.entryId) {
                  updated.push({
                    "id": it.id,
                    "label": it.label,
                    "locked": it.locked,
                    "enabled": !it.enabled
                  })
                } else {
                  updated.push(it)
                }
              }
              root.client.setSetting(root.descriptor.id, updated)
            }
          }
        }

        Text {
          anchors.verticalCenter: parent.verticalCenter
          text: entryRow.entryLabel + (entryRow.isLocked ? " (locked)" : "")
          textFormat: Text.PlainText
          color: entryRow.isLocked ? Color.muted : Color.foreground
          font.pixelSize: 11
        }
      }
    }
  }
}
