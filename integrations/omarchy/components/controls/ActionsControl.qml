pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons

Item {
  id: root

  property var descriptor: null
  property var client: null

  property var pendingAction: null

  implicitWidth: 340
  implicitHeight: labelCol.height + actionListCol.height + (confirmBox.visible ? confirmBox.height + 6 : 0) + 8

  readonly property string labelText: root.descriptor ? String(root.descriptor.label || root.descriptor.id || "") : ""
  readonly property string helpString: (root.descriptor && root.descriptor.help) ? String(root.descriptor.help) : ""
  readonly property var actionsList: (root.descriptor && root.descriptor.kind && root.descriptor.kind.actions)
    ? root.descriptor.kind.actions
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
    id: actionListCol
    anchors.left: parent.left
    anchors.top: labelCol.bottom
    anchors.topMargin: 4
    spacing: 6

    Repeater {
      model: root.actionsList

      delegate: Rectangle {
        id: actBtn
        required property var modelData
        required property int index

        readonly property string actId: String(actBtn.modelData.id || "")
        readonly property string actLabel: String(actBtn.modelData.label || actBtn.actId)

        width: Math.max(90, actText.implicitWidth + 20)
        height: 24
        radius: 4
        color: Color.background
        border.width: 1
        border.color: Color.muted

        Text {
          id: actText
          anchors.centerIn: parent
          text: actBtn.actLabel
          textFormat: Text.PlainText
          color: Color.foreground
          font.pixelSize: 11
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: {
            if (actBtn.modelData.confirmation) {
              root.pendingAction = actBtn.modelData
            } else {
              if (root.client) {
                root.client.runAction(actBtn.actId, true)
              }
            }
          }
        }
      }
    }
  }

  Rectangle {
    id: confirmBox
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: actionListCol.bottom
    anchors.topMargin: 6
    height: confirmCol.height + 16
    radius: 4
    color: Color.background
    border.width: 1
    border.color: Color.urgent
    visible: root.pendingAction !== null

    Column {
      id: confirmCol
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.top: parent.top
      anchors.margins: 8
      spacing: 6

      Text {
        text: root.pendingAction && root.pendingAction.confirmation ? String(root.pendingAction.confirmation.title || "Confirm Action") : ""
        textFormat: Text.PlainText
        color: Color.urgent
        font.pixelSize: 11
        font.bold: true
      }

      Text {
        text: root.pendingAction && root.pendingAction.confirmation ? String(root.pendingAction.confirmation.message || "") : ""
        textFormat: Text.PlainText
        color: Color.foreground
        font.pixelSize: 10
        wrapMode: Text.Wrap
        width: parent.width
      }

      Row {
        spacing: 8

        Rectangle {
          width: 70
          height: 20
          radius: 3
          color: Color.urgent

          Text {
            anchors.centerIn: parent
            text: root.pendingAction && root.pendingAction.confirmation ? String(root.pendingAction.confirmation.confirmLabel || "Confirm") : "Confirm"
            textFormat: Text.PlainText
            color: Color.background
            font.pixelSize: 10
            font.bold: true
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: {
              if (root.client && root.pendingAction) {
                root.client.runAction(root.pendingAction.id, true)
              }
              root.pendingAction = null
            }
          }
        }

        Rectangle {
          width: 60
          height: 20
          radius: 3
          color: Color.background
          border.width: 1
          border.color: Color.muted

          Text {
            anchors.centerIn: parent
            text: "Cancel"
            textFormat: Text.PlainText
            color: Color.foreground
            font.pixelSize: 10
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: {
              root.pendingAction = null
            }
          }
        }
      }
    }
  }
}
