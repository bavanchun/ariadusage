pragma ComponentBehavior: Bound
import QtQuick
import Quickshell

QtObject {
  id: root

  readonly property string socketPath: Quickshell.env("XDG_RUNTIME_DIR") !== ""
    ? (Quickshell.env("XDG_RUNTIME_DIR") + "/ariadusage/engine.sock")
    : "/tmp/ariadusage/engine.sock"
  readonly property string engineBinary: "/usr/bin/ariadusage"
}
