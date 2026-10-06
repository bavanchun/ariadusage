pragma ComponentBehavior: Bound
import QtQuick
import qs.Commons
import "controls"

Item {
  id: root

  property var settingsPage: null
  property var client: null

  implicitWidth: 360
  implicitHeight: sectionsCol.height + 20

  readonly property var sectionsList: (root.settingsPage && root.settingsPage.sections)
    ? root.settingsPage.sections
    : []

  Column {
    id: sectionsCol
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    spacing: 16

    Repeater {
      model: root.sectionsList

      delegate: Column {
        id: secCol
        required property var modelData
        required property int index

        readonly property string secTitle: String(secCol.modelData.title || secCol.modelData.id || "")
        readonly property var descList: secCol.modelData.descriptors ? secCol.modelData.descriptors : []

        width: sectionsCol.width
        spacing: 10

        Text {
          text: secCol.secTitle
          textFormat: Text.PlainText
          color: Color.accent
          font.pixelSize: 13
          font.bold: true
          visible: secCol.secTitle !== ""
        }

        Repeater {
          model: secCol.descList

          delegate: Item {
            id: descItem
            required property var modelData
            required property int index

            width: secCol.width
            height: controlLoader.implicitHeight > 0 ? controlLoader.implicitHeight : 36

            Loader {
              id: controlLoader
              anchors.fill: parent
              sourceComponent: {
                var t = (descItem.modelData && descItem.modelData.kind && descItem.modelData.kind.type)
                  ? String(descItem.modelData.kind.type)
                  : ""
                if (t === "toggle") return toggleComp
                if (t === "choice") return choiceComp
                if (t === "multiChoice") return multiChoiceComp
                if (t === "number") return numberComp
                if (t === "text") return textComp
                if (t === "secret") return secretComp
                if (t === "pathList") return pathListComp
                if (t === "color") return colorComp
                if (t === "actions") return actionsComp
                if (t === "tokenAccounts") return tokenAccountsComp
                return unsupportedComp
              }

              onLoaded: {
                if (controlLoader.item) {
                  controlLoader.item.descriptor = descItem.modelData
                  controlLoader.item.client = root.client
                }
              }
            }
          }
        }
      }
    }
  }

  Component {
    id: toggleComp
    ToggleControl {}
  }

  Component {
    id: choiceComp
    ChoiceControl {}
  }

  Component {
    id: multiChoiceComp
    MultiChoiceControl {}
  }

  Component {
    id: numberComp
    NumberControl {}
  }

  Component {
    id: textComp
    TextControl {}
  }

  Component {
    id: secretComp
    SecretControl {}
  }

  Component {
    id: pathListComp
    PathListControl {}
  }

  Component {
    id: colorComp
    ColorControl {}
  }

  Component {
    id: actionsComp
    ActionsControl {}
  }

  Component {
    id: tokenAccountsComp
    TokenAccountsControl {}
  }

  Component {
    id: unsupportedComp
    UnsupportedControl {}
  }
}
