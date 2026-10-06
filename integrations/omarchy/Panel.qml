pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons
import qs.Ui
import "components"

Panel {
  id: root
  moduleName: "io.github.bavanchun.ariadusage"
  ipcTarget: "io.github.bavanchun.ariadusage"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  property var client: null

  readonly property PluginBarApi hostBar: root.bar as PluginBarApi
  readonly property var barIdentity: hostWidget || root
  readonly property color contentForeground: hostBar ? hostBar.foreground : Color.foreground
  readonly property color contentMuted: Color.muted
  readonly property color contentAccent: Color.accent
  readonly property color contentBackground: Color.background
  readonly property color contentSurface: Color.background
  readonly property string contentFontFamily: hostBar ? hostBar.fontFamily : Style.fontFamily

  property int currentTab: 0 // 0: Overview, 1: Settings
  property double nowMs: Date.now()

  Timer {
    id: ageTimer
    interval: 5000
    repeat: true
    running: root.opened
    onTriggered: {
      root.nowMs = Date.now()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem as Item
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    centerOnBar: true
    focusTarget: keyCatcher
    contentWidth: Style.space(380)
    contentHeight: Style.space(460)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent

      onCloseRequested: root.close()
      onActivateRequested: {
        if (root.client) root.client.refresh()
      }
      onTabRequested: function(direction) {
        root.currentTab = root.currentTab === 0 ? 1 : 0
      }
      onTextKey: function(t) {
        if (t === "r" || t === "R") {
          if (root.client) root.client.refresh()
        } else if (t === "1") {
          root.currentTab = 0
        } else if (t === "2") {
          root.currentTab = 1
        }
      }

      Column {
        id: panelLayout
        anchors.fill: parent
        anchors.margins: 14
        spacing: 12

        // Header: Brand Title, Tabs, Refresh button
        Row {
          id: headerRow
          width: panelLayout.width
          height: 32
          spacing: 8

          Text {
            id: titleLabel
            anchors.verticalCenter: parent.verticalCenter
            text: "AriadUsage"
            textFormat: Text.PlainText
            color: root.contentForeground
            font.family: root.contentFontFamily
            font.pixelSize: 15
            font.bold: true
          }

          Item {
            // Spacer
            width: Math.max(0, headerRow.width - titleLabel.implicitWidth - tabButtonsRow.implicitWidth - refreshBtn.implicitWidth - 24)
            height: 1
          }

          Row {
            id: tabButtonsRow
            anchors.verticalCenter: parent.verticalCenter
            spacing: 4

            Rectangle {
              id: tabOverviewBtn
              width: 76
              height: 26
              radius: 4
              color: root.currentTab === 0 ? root.contentAccent : root.contentBackground
              opacity: root.currentTab === 0 ? 1.0 : 0.6

              Text {
                anchors.centerIn: parent
                text: "Overview"
                textFormat: Text.PlainText
                color: root.currentTab === 0 ? Color.background : root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: 12
                font.bold: root.currentTab === 0
              }

              MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.currentTab = 0
              }
            }

            Rectangle {
              id: tabSettingsBtn
              width: 70
              height: 26
              radius: 4
              color: root.currentTab === 1 ? root.contentAccent : root.contentBackground
              opacity: root.currentTab === 1 ? 1.0 : 0.6

              Text {
                anchors.centerIn: parent
                text: "Settings"
                textFormat: Text.PlainText
                color: root.currentTab === 1 ? Color.background : root.contentForeground
                font.family: root.contentFontFamily
                font.pixelSize: 12
                font.bold: root.currentTab === 1
              }

              MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                  root.currentTab = 1
                  if (root.client) root.client.fetchSettings()
                }
              }
            }
          }

          Rectangle {
            id: refreshBtn
            anchors.verticalCenter: parent.verticalCenter
            width: 26
            height: 26
            radius: 4
            color: root.contentBackground
            border.width: 1
            border.color: root.contentMuted

            Text {
              anchors.centerIn: parent
              text: "\u21bb"
              textFormat: Text.PlainText
              color: root.contentForeground
              font.pixelSize: 14
              font.bold: true
            }

            MouseArea {
              anchors.fill: parent
              cursorShape: Qt.PointingHandCursor
              onClicked: {
                if (root.client) root.client.refresh()
              }
            }
          }
        }

        Rectangle {
          id: divider
          width: panelLayout.width
          height: 1
          color: root.contentMuted
          opacity: 0.3
        }

        // Tab Content
        Item {
          id: contentArea
          width: panelLayout.width
          height: panelLayout.height - headerRow.height - divider.height - footerRow.height - 36

          // Overview Tab
          Flickable {
            id: overviewFlickable
            anchors.fill: parent
            visible: root.currentTab === 0
            contentWidth: overviewCol.width
            contentHeight: overviewCol.height
            clip: true

            Column {
              id: overviewCol
              width: overviewFlickable.width
              spacing: 8

              Item {
                id: emptyStateNotice
                width: overviewCol.width
                height: 120
                visible: !root.client || !root.client.engineRunning || !root.client.snapshot || !root.client.snapshot.providers || root.client.snapshot.providers.length === 0

                Column {
                  anchors.centerIn: parent
                  spacing: 6

                  Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: (!root.client || !root.client.engineRunning) ? "Engine not running" : "No active providers"
                    textFormat: Text.PlainText
                    color: Color.urgent
                    font.family: root.contentFontFamily
                    font.pixelSize: 14
                    font.bold: true
                  }

                  Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: (!root.client || !root.client.engineRunning) ? "Awaiting connection to engine socket\u2026" : "Enable providers in settings or configuration."
                    textFormat: Text.PlainText
                    color: root.contentMuted
                    font.family: root.contentFontFamily
                    font.pixelSize: 11
                  }
                }
              }

              Repeater {
                id: providerRepeater
                model: (root.client && root.client.snapshot && root.client.snapshot.providers) ? root.client.snapshot.providers : []

                delegate: ProviderRow {
                  id: rowDelegate
                  required property var modelData
                  required property int index

                  width: overviewCol.width
                  provider: rowDelegate.modelData
                  nowMs: root.nowMs
                }
              }
            }
          }

          // Settings Tab
          Flickable {
            id: settingsFlickable
            anchors.fill: parent
            visible: root.currentTab === 1
            contentWidth: settingsFormItem.width
            contentHeight: settingsFormItem.height
            clip: true

            SettingsForm {
              id: settingsFormItem
              width: settingsFlickable.width
              settingsPage: root.client ? root.client.settingsPage : null
              client: root.client
            }
          }
        }

        // Footer: Status and version
        Row {
          id: footerRow
          width: panelLayout.width
          height: 20
          spacing: 8

          Rectangle {
            id: statusDot
            anchors.verticalCenter: parent.verticalCenter
            width: 8
            height: 8
            radius: 4
            color: (root.client && root.client.engineRunning) ? (root.client.stale ? Color.muted : Color.accent) : Color.urgent
          }

          Text {
            id: statusText
            anchors.verticalCenter: parent.verticalCenter
            text: {
              if (!root.client || !root.client.engineRunning) return "Engine not running"
              if (root.client.stale) return "Connected (stale)"
              return "Connected"
            }
            textFormat: Text.PlainText
            color: root.contentMuted
            font.family: root.contentFontFamily
            font.pixelSize: 11
          }

          Item {
            // Spacer
            width: Math.max(0, footerRow.width - statusDot.width - statusText.implicitWidth - versionText.implicitWidth - 24)
            height: 1
          }

          Text {
            id: versionText
            anchors.verticalCenter: parent.verticalCenter
            text: (root.client && root.client.snapshot && root.client.snapshot.engine) ? "v" + String(root.client.snapshot.engine.version) : ""
            textFormat: Text.PlainText
            color: root.contentMuted
            font.family: root.contentFontFamily
            font.pixelSize: 11
          }
        }
      }
    }
  }
}
