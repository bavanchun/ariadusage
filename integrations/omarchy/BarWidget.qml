pragma ComponentBehavior: Bound
import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Ui
import "components"

BarWidget {
  id: root
  moduleName: "io.github.bavanchun.ariadusage"

  readonly property PluginBarApi hostBar: root.bar as PluginBarApi
  readonly property string configuredProvider: String(setting("provider", ""))

  readonly property var activeProvider: {
    if (!engineClient.snapshot || !engineClient.snapshot.providers || engineClient.snapshot.providers.length === 0) {
      return null
    }
    var list = engineClient.snapshot.providers
    if (root.configuredProvider !== "") {
      for (var i = 0; i < list.length; i++) {
        if (list[i] && String(list[i].id) === root.configuredProvider) {
          return list[i]
        }
      }
    }
    return list[0]
  }

  readonly property string activeProviderId: root.activeProvider ? String(root.activeProvider.id || "") : ""
  readonly property var primaryMetric: (root.activeProvider && root.activeProvider.windows && root.activeProvider.windows.primary)
    ? root.activeProvider.windows.primary
    : null

  readonly property string metricState: {
    if (!engineClient.engineRunning) return "unknown"
    if (engineClient.stale) return "stale"
    if (root.primaryMetric) return String(root.primaryMetric.state || "unknown")
    return "unknown"
  }

  readonly property real usedPercent: (root.primaryMetric && root.primaryMetric.value && root.primaryMetric.value.usedPercent !== undefined)
    ? Number(root.primaryMetric.value.usedPercent)
    : 0.0

  readonly property string iconSource: {
    if (root.activeProviderId === "claude") return Qt.resolvedUrl("assets/providers/claude-mono.svg")
    if (root.activeProviderId === "codex") return Qt.resolvedUrl("assets/providers/codex-mono.svg")
    if (root.activeProviderId === "antigravity") return Qt.resolvedUrl("assets/providers/antigravity-mono.svg")
    return Qt.resolvedUrl("assets/ariadusage-mark-mono.svg")
  }

  readonly property Panel panelItem: panelLoader.item as Panel
  readonly property bool opened: panelItem ? panelItem.opened === true : false
  readonly property bool popoutSwitchClosing: panelItem ? panelItem.popoutSwitchClosing === true : false

  function open() {
    if (panelItem) panelItem.open()
  }

  function close() {
    if (panelItem) panelItem.close()
  }

  function togglePanel() {
    if (panelItem) panelItem.toggle()
  }

  function closeForPopoutSwitch() {
    if (panelItem) panelItem.closeForPopoutSwitch()
  }

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
    if ("client" in target) target.client = engineClient
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()

  EngineClient {
    id: engineClient
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    horizontalMargin: 8
    verticalPadding: 4

    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton || buttonCode === Qt.MiddleButton) {
        engineClient.refresh()
      } else {
        root.togglePanel()
      }
    }

    Row {
      anchors.centerIn: parent
      spacing: 6

      Image {
        id: iconImage
        anchors.verticalCenter: parent.verticalCenter
        source: root.iconSource
        sourceSize.width: Style.space(16)
        sourceSize.height: Style.space(16)
        fillMode: Image.PreserveAspectFit
      }

      MetricView {
        id: metricDisplay
        anchors.verticalCenter: parent.verticalCenter
        metricState: root.metricState
        usedPercent: root.usedPercent
        compact: true
        foregroundColor: button.foreground
      }
    }
  }

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  IpcHandler {
    target: "io.github.bavanchun.ariadusage"

    function refresh(): void { engineClient.refresh() }
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.togglePanel() }
  }
}
