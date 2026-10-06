pragma ComponentBehavior: Bound
import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  EngineConfig {
    id: config
  }

  readonly property string socketPath: config.socketPath
  readonly property string engineBinary: config.engineBinary

  readonly property bool connected: socket.connected
  readonly property bool engineRunning: socket.connected && welcomeReceived
  property bool welcomeReceived: false

  property var snapshot: null
  property var settingsPage: null
  property bool stale: false
  property int staleAfterSeconds: 15

  property double lastSnapshotMs: 0

  readonly property var backoffDelays: [1, 2, 5, 10, 30]
  property int backoffIndex: 0

  Socket {
    id: socket
    path: root.socketPath
    connected: false

    onConnectionStateChanged: {
      if (socket.connected) {
        root.backoffIndex = 0
        reconnectTimer.interval = 1000
        root.sendHello()
      } else {
        root.welcomeReceived = false
      }
    }

    parser: SplitParser {
      splitMarker: "\n"
      onRead: function(line) {
        if (line.length > 1048576) {
          // Drop line over 1 MiB (defence in depth guard)
          return
        }
        root.handleFrame(line)
      }
    }
  }

  Timer {
    id: reconnectTimer
    interval: 1000
    repeat: true
    running: !socket.connected
    onTriggered: {
      var delaySec = root.backoffDelays[root.backoffIndex]
      reconnectTimer.interval = delaySec * 1000
      root.backoffIndex = Math.min(root.backoffIndex + 1, root.backoffDelays.length - 1)
      socket.connected = false
      Qt.callLater(function() {
        if (!socket.connected) {
          socket.connected = true
        }
      })
    }
  }

  Timer {
    id: staleCheckTimer
    interval: 1000
    repeat: true
    running: root.engineRunning
    onTriggered: {
      if (root.lastSnapshotMs > 0 && root.staleAfterSeconds > 0) {
        var elapsedSec = (Date.now() - root.lastSnapshotMs) / 1000.0
        root.stale = elapsedSec > root.staleAfterSeconds
      }
    }
  }

  function sendRaw(jsonStr) {
    if (socket.connected) {
      socket.write(jsonStr + "\n")
    }
  }

  function sendHello() {
    sendRaw(JSON.stringify({
      "type": "hello",
      "protocols": ["ariadusage-ipc/1"],
      "client": {
        "name": "ariadusage-omarchy",
        "version": "0.0.0"
      },
      "id": "req-hello"
    }))
  }

  function sendSubscribe() {
    sendRaw(JSON.stringify({
      "type": "subscribe",
      "topics": ["snapshot", "settings", "notices"],
      "id": "req-sub"
    }))
  }

  function fetchSettings() {
    sendRaw(JSON.stringify({
      "type": "getSettings",
      "scope": "app",
      "id": "req-settings"
    }))
  }

  function setSetting(settingId, value) {
    sendRaw(JSON.stringify({
      "type": "setSetting",
      "id": settingId,
      "value": value
    }))
  }

  function runAction(actionId, confirm) {
    sendRaw(JSON.stringify({
      "type": "runAction",
      "id": actionId,
      "confirm": confirm === undefined ? true : confirm
    }))
  }

  function refresh(providerId) {
    var req = {
      "type": "refresh",
      "id": "req-ref-" + Date.now()
    }
    if (providerId) req.provider = providerId
    sendRaw(JSON.stringify(req))
  }

  function launchSecretSet(settingId) {
    Quickshell.execDetached([
      "/usr/bin/uwsm-app",
      "--",
      "/usr/bin/xdg-terminal-exec",
      root.engineBinary,
      "secret",
      "set",
      "--id",
      String(settingId),
      "--socket",
      root.socketPath
    ])
  }

  function handleFrame(line) {
    var msg
    try {
      msg = JSON.parse(line)
    } catch (_e) {
      return
    }
    if (!msg || typeof msg !== "object") return

    var t = msg.type
    if (t === "welcome") {
      root.welcomeReceived = true
      root.backoffIndex = 0
      root.sendSubscribe()
      root.fetchSettings()
    } else if (t === "snapshot") {
      if (msg.snapshot) {
        root.snapshot = msg.snapshot
        root.lastSnapshotMs = Date.now()
        root.stale = false
      }
    } else if (t === "settingsChanged") {
      root.fetchSettings()
    } else if (t === "response") {
      if (msg.payload && msg.id && String(msg.id).indexOf("settings") !== -1) {
        root.settingsPage = msg.payload
      }
    }
  }

  Component.onCompleted: {
    socket.connected = true
  }
}
