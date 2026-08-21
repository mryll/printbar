import QtQuick
import qs.Commons
import qs.Ui

// printbar bar widget: a printer glyph, muted while idle, with the queued-job
// count while printing and an urgent tint when the printer needs attention.
// Left click opens the detail panel (Panel.qml), right click opens the
// printer's web panel (EWS), middle click refreshes.
BarWidget {
  id: root
  moduleName: "mryll.printbar"

  readonly property var panel: panelLoader.item
  readonly property string severity: panel ? panel.severity : "ok"
  readonly property string status: panel ? panel.status : ""
  readonly property int jobCount: panel ? panel.jobCount : 0

  readonly property bool urgentState: severity === "critical" || severity === "offline" || severity === "error"
  readonly property bool printing: status === "printing" || jobCount > 0
  readonly property bool idleQuiet: severity === "ok" && !printing

  // colorMode (manifest setting): full | none | bar-only | panel-only. This is
  // the bar face, so it keeps its colors under "full" and "bar-only"; the panel
  // reads the same setting for itself.
  readonly property string colorMode: String(setting("colorMode", "full"))
  readonly property bool barColored: colorMode === "full" || colorMode === "bar-only"

  // barForeground is the bar-face token (transparency-aware), unlike the
  // panel content which uses the popup surface tokens.
  readonly property color barForeground: root.bar ? root.bar.barForeground : Color.foreground
  // Monochrome: foreground and dimmed foreground only — no urgent token. The
  // glyph still brightens for anything that wants attention, and the panel,
  // the tooltip line and the job count carry the actual severity.
  readonly property color stateColor: !barColored
    ? (urgentState || printing ? barForeground : Qt.darker(barForeground, 1.4))
    : urgentState ? (root.bar ? root.bar.urgent : Color.urgent)
    : printing ? barForeground
    : Qt.darker(barForeground, 1.4) // muted while idle

  // The shell's shared tooltip renders rich text; escape-then-wrap so external
  // strings (printer model, front-panel text) can't inject markup.
  function safeTooltip(s) {
    return "<span>" + String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;")
        .replace(/>/g, "&gt;").replace(/\n/g, "<br/>") + "</span>"
  }

  function injectPanel() {
    var target = panelLoader.item
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
  }

  function refresh() {
    if (panelLoader.item && panelLoader.item.refresh) panelLoader.item.refresh()
  }

  function togglePanel() {
    if (panelLoader.item && panelLoader.item.toggle) panelLoader.item.toggle()
  }

  // Shape contract for shell.summon/hide/toggle routing (Bar.findPanelWidget
  // requires open/close/opened on the bar-widget root) — same forwarding the
  // weather plugin uses.
  readonly property bool opened: panelLoader.item ? panelLoader.item.opened === true : false

  function open() {
    if (panelLoader.item && panelLoader.item.openFromHotkey) panelLoader.item.openFromHotkey()
  }

  function close() {
    if (panelLoader.item && panelLoader.item.close) panelLoader.item.close()
  }

  readonly property bool popoutSwitchClosing: panelLoader.item ? panelLoader.item.popoutSwitchClosing === true : false

  function closeForPopoutSwitch() {
    if (panelLoader.item) panelLoader.item.closeForPopoutSwitch()
  }

  // hideWhenIdle conceals the widget while everything is fine and nothing is
  // printing; an open panel always keeps it visible.
  visible: !setting("hideWhenIdle", false) || !idleQuiet || opened
  // How wide the bar's open-panel underline should be. Without this hint the bar
  // falls back to 55% of the SLOT, which reads as a dot under a narrow widget
  // but as a bar that visibly stops short under a wide one. The painted content
  // is the honest extent, so the mark tracks what the widget draws instead of a
  // fraction of the box it happens to sit in. (Same hint the first-party clock
  // gives; it passes its label width.)
  readonly property real openPanelIndicatorWidth: button.labelWidth

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: injectPanel()
  onSettingsChanged: injectPanel()

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
    // nf-md-printer; the job count joins it while printing (horizontal bars only).
    text: root.printing && root.jobCount > 0 && !root.vertical
      ? "󰐪 " + root.jobCount
      : "󰐪"
    fontSize: Style.bar.iconFont
    foreground: root.stateColor
    // Terse; the panel is the detail view.
    tooltipText: root.panel ? root.safeTooltip(root.panel.tooltipLine) : "Printer"

    onPressed: function(b) {
      if (b === Qt.RightButton) {
        if (root.panel) root.panel.openWebPanel()
      } else if (b === Qt.MiddleButton) {
        root.broadcast("refresh")
      } else {
        root.togglePanel()
      }
    }
  }
}
