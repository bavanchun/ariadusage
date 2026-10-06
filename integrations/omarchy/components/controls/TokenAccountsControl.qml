pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  implicitWidth: 340
  implicitHeight: labelCol.height + accountsCol.height + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property var accountsList: (root.descriptor && root.descriptor.kind && root.descriptor.kind.accounts)
    ? root.descriptor.kind.accounts
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
    id: accountsCol
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 4

    Repeater {
      model: root.accountsList

      delegate: Rectangle {
        id: accRow
        required property var modelData
        required property int index

        readonly property string accLabel: String(accRow.modelData.label || accRow.modelData.id || "")
        readonly property bool isActive: accRow.modelData.active === true
        readonly property bool isTokenSet: accRow.modelData.token_is_set === true

        width: accountsCol.width
        height: 24
        radius: 4
        color: Color.background
        border.width: 1
        border.color: accRow.isActive ? Color.accent : Color.muted

        Row {
          anchors.fill: parent
          anchors.margins: 6
          spacing: 8

          Text {
            text: accRow.accLabel
            textFormat: Text.PlainText
            color: Color.foreground
            font.pixelSize: 11
            font.bold: accRow.isActive
            elide: Text.ElideRight
            width: parent.width - statusRow.width - parent.spacing
          }

          Row {
            id: statusRow
            spacing: 6
            anchors.verticalCenter: parent.verticalCenter

            Rectangle {
              width: 44
              height: 14
              radius: 2
              color: accRow.isActive ? Color.accent : Color.muted

              Text {
                anchors.centerIn: parent
                text: accRow.isActive ? "ACTIVE" : "IDLE"
                textFormat: Text.PlainText
                color: Color.background
                font.pixelSize: 9
                font.bold: true
              }
            }

            Text {
              anchors.verticalCenter: parent.verticalCenter
              text: accRow.isTokenSet ? "Token set" : "No token"
              textFormat: Text.PlainText
              color: accRow.isTokenSet ? Color.accent : Color.muted
              font.pixelSize: 10
            }
          }
        }
      }
    }
  }
}
