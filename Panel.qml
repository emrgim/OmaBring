import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

Panel {
  id: root
  moduleName: "emrgim.omabring"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null

  function open() {
    root.controller.show()
  }

  function close() {
    root.controller.hide()
  }

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction)
    return false
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(240))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: content
        width: parent.width
        spacing: Style.space(8)

        Text {
          width: parent.width
          text: "OmaBring"
          color: root.barForeground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.subtitle
          font.bold: true
        }

        Text {
          width: parent.width
          text: "Grocery dashboard on this laptop."
          color: root.barForeground
          wrapMode: Text.WordWrap
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.body
        }

        WidgetButton {
          text: "Open dashboard"
          onPressed: function(buttonCode) {
            if (buttonCode === Qt.LeftButton) Qt.openUrlExternally("http://127.0.0.1:8877/")
          }
        }
      }
    }
  }
}
