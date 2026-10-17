import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Dictate into the TextArea using Handy's normal global shortcut.
// Handy enters text at the cursor; this widget intentionally does not
// scrape clipboard contents or access the microphone itself.
Panel {
    id: root
    moduleName: "io.github.godxgun.angela"
    manageIpc: false

    property var anchorItem: null
    property var hostWidget: null
    property string actionMessage: ""
    property bool busy: false
    property var actionArgs: []

    readonly property var data: hostWidget ? hostWidget.snapshot : ({})
    readonly property var rec: data.recommendation || ({})
    readonly property var active: data.active || null
    readonly property var upcoming: data.next_fixed || null
    readonly property var nearest: data.nearest_deadline || null
    readonly property var progress: data.today_minutes || ({})

    function open() { root.controller.show() }
    function close() { root.controller.hide() }
    function toggle() { if (root.opened) close(); else open() }
    function closeForPopoutSwitch() { close() }
    function switchPanel(direction) {
        if (root.bar && typeof root.bar.switchPanelFrom === "function")
            return root.bar.switchPanelFrom(root.hostWidget || root, direction)
        return false
    }
    function runNow(args) {
        if (root.busy) return
        root.busy = true
        root.actionMessage = "Working…"
        root.actionArgs = args
        actionProcess.command = ["/usr/bin/env", "now"].concat(args)
        actionProcess.running = true
    }

    Process {
        id: actionProcess
        onExited: function(code, status) {
            root.busy = false
            if (code === 0) {
                root.actionMessage = "Saved"
                if (root.actionArgs[0] === "log") reportArea.text = ""
                if (root.hostWidget) root.hostWidget.reload()
            } else {
                root.actionMessage = "Action failed; run the command in a terminal for details"
            }
        }
    }

    Process {
        id: handyProcess
        command: ["/usr/bin/env", "handy", "--toggle-transcription"]
        onExited: function(code, status) {
            if (code !== 0) root.actionMessage = "Handy CLI unavailable. Use your existing Handy hotkey."
        }
    }

    KeyboardPanel {
        id: panel
        anchorItem: root.anchorItem
        owner: root.hostWidget || root
        bar: root.bar
        open: root.opened
        focusTarget: keyCatcher
        contentWidth: panel.fittedContentWidth(Style.space(370))
        contentHeight: panel.fittedContentHeight(content.implicitHeight)

        PanelKeyCatcher {
            id: keyCatcher
            anchors.fill: parent
            onCloseRequested: root.close()
            onTabRequested: function(direction) { root.switchPanel(direction) }

            Column {
                id: content
                width: parent.width
                spacing: Style.space(10)

                Text {
                    width: parent.width
                    text: "NOW / NEXT ACTION"
                    color: root.barForeground
                    font.family: root.bar ? root.bar.fontFamily : Style.font.family
                    font.pixelSize: Style.font.subtitle
                    font.bold: true
                }
                Text {
                    width: parent.width
                    text: root.active ? "Tracking: " + root.active.project + " · " + root.active.minutes + " min"
                                      : "No active timer"
                    color: root.barForeground
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    text: root.rec.title || "No recommendation available"
                    color: root.barForeground
                    font.pixelSize: Style.font.subtitle
                    font.bold: true
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    text: root.rec.reason || "Start a timer or add a deadline in the CLI."
                    color: root.barForeground
                    opacity: 0.75
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    visible: !!root.nearest
                    text: root.nearest ? "Deadline: " + root.nearest.title + " · " + root.nearest.when : ""
                    color: root.barForeground
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    visible: !!root.upcoming
                    text: root.upcoming ? "Next fixed: " + root.upcoming.title + " · " + root.upcoming.when : ""
                    color: root.barForeground
                    wrapMode: Text.WordWrap
                }

                Text {
                    width: parent.width
                    text: "TODAY · DI " + Math.round(root.progress.DI || 0) + "/60m  ·  DJ "
                          + Math.round(root.progress.DJ || 0) + "/60m  ·  VI "
                          + Math.round(root.progress.VI || 0) + "/60m  ·  DS "
                          + Math.round(root.progress.DS || 0) + "/60m"
                    color: root.barForeground
                    opacity: 0.75
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    text: "Drawing " + Math.round(root.progress.Drawing || 0) + "/120m  ·  Docs "
                          + Math.round(root.progress.Docs || 0) + "m  ·  Games "
                          + Math.round(root.progress["Game development"] || 0) + "m"
                    color: root.barForeground
                    opacity: 0.75
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
                Row {
                    spacing: Style.space(7)
                    Button {
                        text: root.active ? "Stop" : "Start next"
                        enabled: !root.busy && (root.active || root.rec.project)
                        onClicked: root.active ? root.runNow(["stop"]) : root.runNow(["start", root.rec.project])
                    }
                    Button {
                        text: root.active && root.active.project === "Sleep" ? "Wake" : "Sleep"
                        enabled: !root.busy
                        onClicked: root.active && root.active.project === "Sleep"
                            ? root.runNow(["wake"]) : root.runNow(["sleep"])
                    }
                    Button {
                        text: "Refresh"
                        enabled: !root.busy
                        onClicked: { if (root.hostWidget) root.hostWidget.reload() }
                    }
                }

                Text {
                    width: parent.width
                    text: "ACTIVITY REPORT  ·  use Handy to dictate here"
                    color: root.barForeground
                    font.pixelSize: 12
                    font.bold: true
                }
                TextArea {
                    id: reportArea
                    width: parent.width
                    height: 105
                    wrapMode: TextEdit.Wrap
                    placeholderText: "I spent 90 minutes on Docs, then worked on DI…"
                    color: root.barForeground
                    selectByMouse: true
                    background: Rectangle {
                        radius: 6
                        color: "#24252b"
                        border.width: 1
                        border.color: "#5a5c64"
                    }
                }
                Row {
                    spacing: Style.space(8)
                    Button {
                        text: "Handy dictation"
                        enabled: !root.busy && !handyProcess.running
                        onClicked: {
                            reportArea.forceActiveFocus()
                            handyProcess.running = true
                        }
                    }
                    Button {
                        text: root.busy ? "Processing…" : "Save report (Ollama)"
                        enabled: !root.busy && reportArea.text.trim().length > 0
                        onClicked: root.runNow(["log", reportArea.text])
                    }
                    Button {
                        text: "Save raw"
                        enabled: !root.busy && reportArea.text.trim().length > 0
                        onClicked: root.runNow(["log", "--raw", reportArea.text])
                    }
                }
                Text {
                    width: parent.width
                    text: root.actionMessage
                    visible: root.actionMessage.length > 0
                    color: root.barForeground
                    opacity: 0.75
                    wrapMode: Text.WordWrap
                }
            }
        }
    }
}
