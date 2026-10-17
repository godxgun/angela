import QtQuick
import Quickshell
import Quickshell.Io
import qs.Ui

// An Omarchy Quattro bar widget. It reads JSON from the installed now CLI.
// No HTTP servers, browser windows, or second Quickshell process are started.
BarWidget {
    id: root
    moduleName: "io.github.godxgun.angela"

    property var snapshot: ({})
    property string errorMessage: ""
    readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false
    readonly property bool popoutSwitchClosing: panelLoader.item
        ? panelLoader.item.popoutSwitchClosing === true : false

    function open() { if (panelLoader.item) panelLoader.item.open() }
    function close() { if (panelLoader.item) panelLoader.item.close() }
    function toggle() { if (panelLoader.item) panelLoader.item.toggle() }
    function closeForPopoutSwitch() {
        if (panelLoader.item) panelLoader.item.closeForPopoutSwitch()
    }
    function injectPanel() {
        if (!panelLoader.item) return
        panelLoader.item.bar = root.bar
        panelLoader.item.anchorItem = button
        panelLoader.item.hostWidget = root
    }
    function reload() {
        if (!snapshotProcess.running) snapshotProcess.running = true
    }

    implicitWidth: button.implicitWidth
    implicitHeight: button.implicitHeight
    onBarChanged: injectPanel()
    Component.onCompleted: reload()

    Process {
        id: snapshotProcess
        command: ["/usr/bin/env", "now", "snapshot"]
        stdout: StdioCollector {
            onStreamFinished: {
                try {
                    var data = JSON.parse(this.text)
                    root.snapshot = data
                    root.errorMessage = ""
                } catch (e) {
                    root.errorMessage = "Cannot read now snapshot"
                }
            }
        }
        onExited: function(code, status) {
            if (code !== 0) root.errorMessage = "now CLI not available"
        }
    }

    Timer {
        interval: 60000
        running: true
        repeat: true
        onTriggered: root.reload()
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

    WidgetButton {
        id: button
        anchors.fill: parent
        bar: root.bar
        text: root.snapshot.active && root.snapshot.active.project
            ? root.snapshot.active.project + " · " + root.snapshot.active.minutes + "m"
            : (root.snapshot.recommendation ? root.snapshot.recommendation.short : "Now")
        tooltipText: root.errorMessage || (root.snapshot.recommendation
            ? root.snapshot.recommendation.reason : "Open the local assistant")
        onPressed: function(buttonCode) {
            if (buttonCode === Qt.LeftButton) root.toggle()
        }
    }
}
