pragma ComponentBehavior: Bound
import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Ui

// printbar detail panel. Owns the data: it polls `printbar <name> --json`
// (structured mode, raw data) and renders status, the printer's own
// front-panel words, supplies with animated level bars, trays and job stats.
//
// Where the colors come from, deliberately split two ways:
//
//   * The INKS come off the wire, from the core's published `palette` — they
//     are printbar's own data, and a second copy here is how two frontends
//     drift apart. See `corePalette`. The panel does NOT paint the severity
//     ramp into the meter fill: a red-to-green wash made a nearly empty
//     cartridge and a full one hard to tell apart, so the fill carries the
//     colorant's own hue and the severity moves to the track outline. The
//     Waybar tooltip still colors its level text by severity; both read the
//     same `state` the core decided.
//   * The panel's CHROME — urgent, accent, muted, the popup surface — uses the
//     shell's live `Color` tokens, NOT the palette's severity colors. This
//     panel is a shell surface: it has to match the window it opens over and
//     re-tint the instant the user switches theme, which a value sampled at
//     poll time cannot do. The published severity colors describe the Waybar
//     surface, where there is no live token to ask.
//
// The `colorMode` setting can take all of it away — see `panelColored`.
Panel {
  id: root
  moduleName: "mryll.printbar"
  ipcTarget: "mryll.printbar"
  manageIpc: false

  property var anchorItem: null
  property bool openedFromHotkey: false

  // The bar tracks the widget mounted in its slot — BarWidget.qml — not this
  // nested panel, so everything the bar identifies a panel by must be that
  // widget (popout coordinator, switchPanelFrom). Same as the weather plugin.
  property var hostWidget: null
  readonly property var barIdentity: hostWidget || root

  // ---- data ----------------------------------------------------------------

  // Last successfully parsed `printbar --json` report. Kept on failure so
  // stale data stays visible (never flash empty).
  property var report: null
  property string errorText: ""
  property string updatedText: ""

  readonly property string printerName: String(setting("printerName", "office"))
  readonly property string configPath: String(setting("configPath", ""))
  readonly property int refreshSeconds: Math.max(5, parseInt(setting("refreshIntervalSec", 30), 10) || 30)

  readonly property string severity: report && report.state ? String(report.state) : "ok"
  readonly property string status: report && report.status ? String(report.status) : ""
  readonly property int jobCount: report && report.jobs !== null && report.jobs !== undefined ? Number(report.jobs) : 0
  readonly property string displayText: report && report.display ? String(report.display) : ""
  readonly property var supplies: report && report.supplies ? report.supplies : []
  readonly property var trays: report && report.trays ? report.trays : []
  readonly property var reasons: report && report.reasons ? report.reasons : []
  readonly property string reportedModel: report && report.model ? String(report.model) : ""
  readonly property string reportedName: report && report.name ? String(report.name) : ""
  readonly property var impressions: report && report.impressions !== null && report.impressions !== undefined ? report.impressions : null

  readonly property string title: reportedModel || reportedName || printerName
  readonly property string statusLabel: status === "" ? "Unknown"
    : status.charAt(0).toUpperCase() + status.slice(1)
  readonly property string tooltipLine: title + " · " + statusLabel
    + (displayText !== "" ? " — " + displayText : "")

  // ---- theme colors ---------------------------------------------------------

  // Panel content renders on the popup card, so it uses the popup surface
  // tokens (the bar-face tokens live in BarWidget.qml).
  readonly property color fg: Color.popups.text
  readonly property string family: root.bar ? root.bar.fontFamily : Style.font.family

  // colorMode (manifest setting): full | none | bar-only | panel-only. The
  // panel keeps its colors under "full" and "panel-only"; the bar face reads
  // the same setting for itself in BarWidget.qml.
  //
  // Monochrome collapses every hue onto the foreground and its dimmed
  // relatives: no accent, no urgent, no ramp, no ink. Severity is not lost —
  // it still reads through the status label, the condition pills, the bold
  // percentage and the outlined meter track — and the structured JSON keeps
  // its `state` field for anything scripting on top of the CLI.
  // An unrecognized value normalizes to "full": a hand-edited shell.json must
  // not be able to silently take the color off both surfaces.
  readonly property string colorMode: {
    var v = String(setting("colorMode", "full"))
    return ["full", "none", "bar-only", "panel-only"].indexOf(v) >= 0 ? v : "full"
  }
  readonly property bool barColored:   colorMode === "full" || colorMode === "bar-only"
  readonly property bool panelColored: colorMode === "full" || colorMode === "panel-only"

  readonly property color urgent: panelColored ? Color.urgent : root.fg
  readonly property color accent: panelColored ? Color.accent : root.fg
  readonly property color mutedFg: panelColored ? Color.muted : Qt.darker(root.fg, 1.55)

  function mixColor(a, b, t) {
    return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, 1)
  }

  // Severity → theme color. warn interpolates between foreground and urgent so
  // it tracks every Omarchy theme instead of hardcoding an orange.
  // Monochrome makes urgent === fg, so both branches collapse to the
  // foreground on their own — nothing here needs a mode test.
  function severityColor(sev) {
    if (sev === "critical" || sev === "error" || sev === "offline") return root.urgent
    if (sev === "warn") return mixColor(root.fg, root.urgent, 0.55)
    if (sev === "unknown") return root.mutedFg
    return root.fg
  }

  // What the printer is DOING, kept separate from what CONDITIONS exist. The
  // two used to share one color, taken from the aggregate condition severity,
  // so an unrelated warn painted the word IDLE and the printer glyph in the
  // urgent tone — reporting a fault where there was none. (Any unrecognized
  // printer-state-reason maps to warn in the core, so "wifi-not-configured"
  // was enough to turn the whole hero red.) The conditions keep their own
  // severity-tinted pills further down; this color says only whether the
  // printer is working, resting, or unreachable.
  readonly property color statusColor: {
    if (status === "offline" || status === "stopped") return root.urgent
    if (status === "printing" || jobCount > 0) return root.accent
    // Idle, or a status the printer would not name: the normal resting state
    // is quiet, not an alarm.
    return root.mutedFg
  }

  // Hero brand mark: the widget's IDENTITY, unconditionally. It never doubles as
  // a severity indicator — a fault is already carried by the meters, the status
  // pills, the status label and the error card, and letting the identity mark
  // move too means the panel has no fixed point a reader can recognise. Pure
  // theme accent (foreground when the panel is monochrome).
  readonly property color brandColor: panelColored ? root.accent : root.fg

  function alphaColor(c, a) {
    return Qt.rgba(c.r, c.g, c.b, a)
  }

  // ---- the published palette ---------------------------------------------------

  // printbar resolves its whole ramp — the severity colors (from the active
  // Omarchy theme, pywal, or its built-in palette), the physical colorant
  // colors, and the threshold percentages they sit at — and publishes it as
  // `palette`. Keeping a second copy here is exactly how two frontends drift
  // apart, so there is none: the inks come off the wire.
  //
  // The thresholds themselves are never re-derived either. The core already
  // resolved each supply's `state` against `palette.stops`, and this panel
  // renders that field; the stops are published for any other consumer that
  // needs the positions rather than the verdict.
  readonly property var corePalette: report && report.palette ? report.palette : null
  readonly property var inkPalette: corePalette && corePalette.ink ? corePalette.ink : ({})

  function hexOrEmpty(v) {
    var s = v === undefined || v === null ? "" : String(v).trim()
    return /^#[0-9a-fA-F]{6}$/.test(s) ? s : ""
  }

  // Physical colorant swatch: the ink the cartridge actually holds, as
  // published by the core. Not theme styling — cyan toner is cyan under every
  // theme — with a hairline outline so black and pale swatches read on any
  // background. A colorant the core could not name falls back to the raw
  // string the printer reported, then to a muted neutral.
  //
  // Monochrome drops all of it, and the swatch renders as an empty outlined
  // square (see the `swatch` Rectangle). The ink hue is identity, but it is
  // never the ONLY carrier of that identity: every row is labelled with the
  // colorant's name. Someone who asked for no colors did not ask for "no
  // colors except four saturated squares", and hue is exactly the channel that
  // fails the people most likely to turn colors off.
  function swatchColor(s) {
    var c = s && s.color ? String(s.color) : ""
    var published = c === "" ? "" : root.hexOrEmpty(root.inkPalette[c])
    if (published !== "") return published
    var raw = root.hexOrEmpty(s && s.color_raw ? s.color_raw : "")
    if (raw !== "") return raw
    return root.mutedFg
  }

  // Ink tint for the level-bar fill: the published colorant's HUE with the
  // lightness taken from the theme foreground (the same theme-anchored trick
  // meteobar uses for its cold/heat colors), so cyan/magenta/yellow read on
  // light and dark themes alike. A colorant with no usable hue — black ink, or
  // one the printer would not name — stays neutral.
  // Monochrome: every fill is plain foreground, and length alone speaks.
  function inkFillColor(s) {
    if (!root.panelColored) return root.fg
    var l = Math.max(0.34, Math.min(0.70, fg.hslLightness))
    var base = Qt.color(root.swatchColor(s))
    if (base.hslSaturation > 0.15)
      return Qt.hsla(base.hslHue, Math.min(0.7, base.hslSaturation), l, 1)
    // Black ink is the page's own darkest tone: the foreground stands in for
    // it. Anything else without a hue is genuinely unknown, and reads dimmer.
    var c = s && s.color ? String(s.color) : ""
    return c === "black" ? root.fg : Qt.darker(root.fg, 1.2)
  }

  // ---- supply meter fill -------------------------------------------------------

  // The meter track: an alpha wash over the popup card. Monochrome keeps the
  // same subdued wash, taken from the foreground instead of the accent.
  readonly property color trackFill: panelColored
    ? Style.selectedFillFor(root.fg, Color.accent, root.urgent)
    : root.alphaColor(root.fg, Style.selectedFillAlpha)

  // The fill is the supply's ink at FULL strength at every level — length alone
  // carries the quantity. An earlier version painted a spatial gradient pinned
  // to the scale (track tone at empty → ink at full, like the sibling widgets'
  // meters). It looked lovely at healthy levels and failed where the
  // information matters most: a 5% stub came out 1.3:1 against the track it
  // sits on, so 0/5/10/15% were indistinguishable at a glance, and the ink
  // identity washed out exactly when you most want to know WHICH cartridge is
  // dying. A constant fill color keeps length the only variable (the cleanest
  // quantitative read), keeps a near-empty stub unmistakably colored, and keeps
  // the readouts at their true ink hue. The siblings' ramps encode severity
  // along the scale, which is meaningful for usage/battery; here severity
  // already has its own carriers (the track's outline, the percentage, the
  // condition pills), so a ramp would have been decoration only.

  // ---- readout legibility ------------------------------------------------------

  // WCAG relative luminance / contrast ratio, used to keep ink-colored text
  // readable on the popup card.
  function relLuminance(c) {
    function chan(v) { return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4) }
    return 0.2126 * chan(c.r) + 0.7152 * chan(c.g) + 0.0722 * chan(c.b)
  }

  function contrastRatio(a, b) {
    var la = relLuminance(a)
    var lb = relLuminance(b)
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
  }

  // Safety net for the percentage readouts: full-strength ink clears 4.5:1 on
  // its own for every colorant this theme produces, but a dark custom
  // `color_raw` on a dark theme could not. Blend toward the foreground, keeping
  // hue, only as far as it takes to clear the ratio — normally a no-op.
  function legibleOnCard(c) {
    var bg = Color.popups.background
    var out = c
    for (var i = 0; i < 12 && contrastRatio(out, bg) < 4.5; i++)
      out = mixColor(out, root.fg, 0.2)
    return out
  }

  // Short, alignable supply label: colorant name, else kind, else raw name.
  function supplyLabel(s) {
    var c = s && s.color ? String(s.color) : ""
    if (c === "black") return "Black"
    if (c === "cyan") return "Cyan"
    if (c === "magenta") return "Magenta"
    if (c === "yellow") return "Yellow"
    if (c === "tri-color") return "Tri-color"
    if (c === "photo") return "Photo"
    var k = s && s.kind ? String(s.kind) : ""
    if (k === "drum") return "Drum"
    if (k === "waste") return "Waste"
    return s && s.name ? String(s.name) : "?"
  }

  // Sentinel levels rendered as short value text (matches the waybar mode).
  function levelText(entry) {
    if (entry.level_pct !== null && entry.level_pct !== undefined) return entry.level_pct + "%"
    if (entry.level === "no-restriction") return "∞"
    if (entry.level === "some-remaining") return "ok"
    return "?"
  }

  // ---- polling ---------------------------------------------------------------

  function collectCommand() {
    var cmd = ["printbar", printerName, "--json"]
    // PRINTBAR_CONFIG is the CLI's config override; env(1) keeps this an argv
    // array (no shell string).
    if (configPath !== "") cmd = ["env", "PRINTBAR_CONFIG=" + configPath].concat(cmd)
    return cmd
  }

  // Poll state machine: collector + exit tracked separately so a failed start
  // (binary missing) still finalizes; a refresh requested mid-poll is queued as
  // a last-command-wins snapshot so setting changes are never dropped.
  property bool collectorDone: true
  property bool processDone: true

  // A fetch is in flight. BOTH halves matter: the exit code and the collected
  // stdout arrive in either order, which is exactly why maybeFinalize() waits
  // for the pair. The refresh button gates on this, not on collectorDone alone
  // — otherwise it re-enables in the gap between the two signals and a click
  // there queues a second run through pendingCmd, which is the one thing its
  // disabled state promises cannot happen.
  readonly property bool fetchBusy: !collectorDone || !processDone
  property string capturedText: ""
  property int exitCode: 0
  property var pendingCmd: null

  function refresh() {
    startRun(collectCommand())
  }

  function startRun(cmd) {
    if (statusProc.running) {
      pendingCmd = cmd
      return
    }
    collectorDone = false
    processDone = false
    capturedText = ""
    statusProc.command = cmd
    statusProc.running = true
  }

  function maybeFinalize() {
    if (!collectorDone || !processDone) return
    exitFallback.stop()
    finalizeRun()
  }

  function finalizeRun() {
    var text = capturedText.trim()
    if (text === "")
      setError("printbar produced no output — not installed or not on PATH?")
    else
      handle(text)
    if (pendingCmd) {
      var c = pendingCmd
      pendingCmd = null
      Qt.callLater(function() { root.startRun(c) })
    }
  }

  function setError(msg) {
    root.errorText = String(msg)
  }

  // Keeps last-known-good on ANY failure — including a well-formed structured
  // error, which is the one that used to slip through: it passes the schema
  // check but carries empty supplies, empty trays and a null palette, so
  // assigning it blanked a panel that had perfectly good data a second ago.
  // The report is replaced only by a document that actually carries a reading.
  function handle(out) {
    var d
    try {
      d = JSON.parse(out)
    } catch (e) {
      setError("printbar returned unparseable output")
      return
    }
    if (!d || typeof d !== "object" || Array.isArray(d) || d.schema_version !== 1) {
      setError("printbar returned an unexpected document (schema_version mismatch?)")
      return
    }
    if (d.error && d.error.message) {
      // A structured error means the poll could not run at all. Show why, over
      // whatever we last managed to read.
      setError(d.error.message)
      return
    }
    root.report = d
    root.updatedText = Qt.formatTime(new Date(), "HH:mm")
    if (root.exitCode !== 0) setError("printbar exited with code " + root.exitCode)
    else root.errorText = ""
  }

  function openWebPanel() {
    var url = report && report.ews_url ? String(report.ews_url) : ""
    if (/^https?:\/\//i.test(url)) Qt.openUrlExternally(url)
  }

  Process {
    id: statusProc
    // The linter flags onExited's QProcess::ExitStatus parameter type as
    // unresolvable; the shipped weather plugin's identical handler trips the
    // same warning — Quickshell type-info gap, not a real issue.
    onExited: function(code) {
      root.exitCode = code
      root.processDone = true
      exitFallback.restart() // failed-start case: the collector may never fire
      root.maybeFinalize()
    }
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.capturedText = text
        root.collectorDone = true
        root.maybeFinalize()
      }
    }
  }

  Timer {
    id: exitFallback
    interval: 300
    repeat: false
    onTriggered: {
      root.collectorDone = true // give up on the collector
      root.maybeFinalize()
    }
  }

  Timer {
    interval: root.refreshSeconds * 1000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.refresh()
  }

  // Settings changes (printer name, config path) take effect on the next poll;
  // kick one immediately so the panel doesn't show the old printer meanwhile.
  onPrinterNameChanged: Qt.callLater(refresh)
  onConfigPathChanged: Qt.callLater(refresh)

  // ---- open/close (weather plugin pattern) -----------------------------------

  // Panel-open sweep: the supply fills scale by this 0 → 1 fraction, so every
  // meter fills up to its current level as the panel appears. Fired only from
  // the open paths — data refreshes while the panel sits open keep the fills'
  // own 160ms Behaviors and never restart the sweep. Initialized at 1: the
  // fills must never sit collapsed at construction.
  property real openProgress: 1

  // Gates the fills' width Behavior during the sweep. Set BEFORE the
  // animation jumps openProgress to 0 (so the Behavior can't intercept the
  // collapse and smear it over 160ms) and cleared in onFinished — not
  // onStopped, which fires spuriously when restart() interrupts a running
  // sweep on rapid re-opens.
  property bool openSweeping: false

  NumberAnimation {
    id: openSweep
    target: root
    property: "openProgress"
    from: 0
    to: 1
    duration: 200
    easing.type: Easing.OutCubic
    onFinished: root.openSweeping = false
  }

  function startOpenSweep() {
    root.openSweeping = true
    openSweep.restart()
  }

  function open() {
    openedFromHotkey = false
    setCenterHoverRevealSuppressed(false)
    root.controller.show()
    startOpenSweep()
    root.refresh()
  }

  function openFromHotkey() {
    openedFromHotkey = true
    root.controller.show()
    startOpenSweep()
    root.refresh()
    Qt.callLater(function() {
      if (root.opened) setCenterHoverRevealSuppressed(true)
    })
  }

  function close() {
    setCenterHoverRevealSuppressed(false)
    root.controller.hide()
  }

  function toggle() {
    if (root.opened) root.close()
    else root.openFromHotkey()
  }

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.barIdentity, direction)
    return false
  }

  function setCenterHoverRevealSuppressed(value) {
    if (root.bar && "centerHoverRevealSuppressed" in root.bar)
      root.bar.centerHoverRevealSuppressed = value
  }

  // The shell's base handler covers open/close/show/hide/toggle; this one adds
  // `refresh` so a keybind or a script can force a fetch without opening the
  // panel. Overriding means restating the five, so `manageIpc: false` above
  // turns the base one off and this is the only handler on the target.
  IpcHandler {
    target: root.ipcTarget

    function open(): void { root.openFromHotkey() }
    function close(): void { root.close() }
    function show(): void { root.openFromHotkey() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
    function refresh(): void { root.refresh() }
  }

  // ---- panel UI ----------------------------------------------------------------

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(360))
    contentHeight: panel.fittedContentHeight(contentColumn.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) { if (t === "r") root.refresh() }

      Flickable {
        id: panelScroll
        anchors.fill: parent
        contentWidth: width
        contentHeight: contentColumn.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height

        Column {
          id: contentColumn
          width: panelScroll.width
          spacing: Style.space(12)

          // ---- Hero: the shared PanelHero, like every sibling panel. This was
          //      hand-rolled — icon, title, uppercase meta and a trailing chip,
          //      which is exactly the component's shape — and its 40px mark made
          //      printbar the one hero out of proportion with the family. The
          //      glyph now sits on the shared display size.
          PanelHero {
            width: parent.width
            title: root.title
            meta: root.title !== root.printerName ? root.printerName : ""
            foreground: root.fg
            fontFamily: root.family

            iconComponent: Component {
              Text {
                text: "󰐪" // nf-md-printer
                textFormat: Text.PlainText
                color: root.brandColor
                font.family: root.family
                font.pixelSize: Style.font.display
                Behavior on color { ColorAnimation { duration: 160 } }
              }
            }

            // Status chip: dot + caps label on a faint pill tinted by what the
            // printer is DOING, not by what conditions exist (those keep their
            // own severity pills further down).
            trailingControl: Component {
              Rectangle {
                implicitWidth: chipContent.implicitWidth + Style.space(20)
                implicitHeight: chipContent.implicitHeight + Style.space(10)
                radius: Math.min(height / 2, Math.max(Style.cornerRadius, 3))
                color: root.alphaColor(root.statusColor, 0.10)
                border.width: 1
                border.color: root.alphaColor(root.statusColor, 0.35)

                Behavior on color { ColorAnimation { duration: 160 } }
                Behavior on border.color { ColorAnimation { duration: 160 } }

                Row {
                  id: chipContent
                  anchors.centerIn: parent
                  spacing: Style.space(6)

                  Rectangle {
                    width: Style.space(6)
                    height: Style.space(6)
                    radius: width / 2
                    anchors.verticalCenter: parent.verticalCenter
                    color: root.statusColor
                    Behavior on color { ColorAnimation { duration: 160 } }
                  }

                  Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.statusLabel.toUpperCase()
                      + (root.jobCount > 0 ? " · " + root.jobCount : "")
                    textFormat: Text.PlainText
                    color: root.fg
                    font.family: root.family
                    font.pixelSize: Style.font.caption
                    font.bold: true
                    font.letterSpacing: 1
                  }
                }
              }
            }
          }

          // ---- The machine speaks: the literal text on the printer's front
          //      panel ("Ready", "Paper jam in tray 2", ...). printbar's
          //      signature datum, so it keeps a card of its own — but it is
          //      REPORTED STATE, not a citation. The blockquote styling it
          //      used to wear (a tinted rule, a quote glyph, italics) framed a
          //      status line as a quotation, and that rule carried the
          //      CONDITION severity, which painted "Modo de reposo activado."
          //      in the urgent tone while the printer was simply idle. A dim
          //      label and plain text now — the same shape the waybar tooltip
          //      gives it.
          Rectangle {
            visible: root.displayText !== ""
            width: parent.width
            implicitHeight: panelText.implicitHeight + Style.space(18)
            radius: Style.cornerRadius
            color: root.alphaColor(root.fg, 0.05)

            Text {
              id: panelLabel
              anchors.left: parent.left
              anchors.leftMargin: Style.space(12)
              anchors.top: panelText.top
              text: "PANEL"
              textFormat: Text.PlainText
              color: root.mutedFg
              font.family: root.family
              font.pixelSize: Style.font.caption
              font.bold: true
              font.letterSpacing: 1.2
            }

            Text {
              id: panelText
              anchors.left: panelLabel.right
              anchors.leftMargin: Style.space(12)
              anchors.right: parent.right
              anchors.rightMargin: Style.space(12)
              anchors.verticalCenter: parent.verticalCenter
              text: root.displayText
              textFormat: Text.PlainText
              color: root.fg
              font.family: root.family
              font.pixelSize: Style.font.body
              wrapMode: Text.Wrap
            }
          }

          // ---- Active conditions as severity-tinted pills (jam, cover open,
          //      supply low, ...). Flow wraps when the printer is very unhappy.
          Flow {
            visible: root.reasons.length > 0
            width: parent.width
            spacing: Style.space(6)

            Repeater {
              model: root.reasons

              Rectangle {
                id: reasonPill
                required property var modelData
                readonly property color sev: root.severityColor(String(reasonPill.modelData.state))

                implicitWidth: pillRow.implicitWidth + Style.space(16)
                implicitHeight: pillRow.implicitHeight + Style.space(8)
                radius: Math.min(height / 2, Math.max(Style.cornerRadius, 3))
                color: root.alphaColor(reasonPill.sev, 0.12)
                border.width: 1
                border.color: root.alphaColor(reasonPill.sev, 0.40)

                Row {
                  id: pillRow
                  anchors.centerIn: parent
                  spacing: Style.space(5)

                  Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: "" // nf-fa-warning
                    color: reasonPill.sev
                    font.family: root.family
                    font.pixelSize: Style.font.caption
                  }
                  Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: String(reasonPill.modelData.label)
                    textFormat: Text.PlainText
                    color: reasonPill.sev
                    font.family: root.family
                    font.pixelSize: Style.font.bodySmall
                  }
                }
              }
            }
          }

          PanelSeparator {
            visible: root.supplies.length > 0
            foreground: root.fg
          }

          PanelSectionHeader {
            visible: root.supplies.length > 0
            text: "SUPPLIES"
            foreground: root.fg
            fontFamily: root.family
          }

          // ---- Per-supply row: true-colorant swatch, name, a substantial level
          //      bar painted with the scale's ramp (urgent at empty → the
          //      supply's own ink tint at full), and the value in the ramp's
          //      color at that level.
          Column {
            visible: root.supplies.length > 0
            width: parent.width
            spacing: Style.space(8)

            Repeater {
              model: root.supplies

              Item {
                id: supplyRow
                required property var modelData
                readonly property bool hasPct: supplyRow.modelData.level_pct !== null && supplyRow.modelData.level_pct !== undefined
                readonly property real pct: supplyRow.hasPct ? Number(supplyRow.modelData.level_pct) : 0
                readonly property string sevState: String(supplyRow.modelData.state)
                readonly property bool needsAttention: sevState === "warn" || sevState === "critical"

                width: parent.width
                height: Style.space(22)

                Rectangle {
                  id: swatch
                  width: Style.space(11)
                  height: Style.space(11)
                  radius: Math.max(2, Math.min(4, Style.cornerRadius))
                  anchors.left: parent.left
                  anchors.leftMargin: Style.space(2)
                  anchors.verticalCenter: parent.verticalCenter
                  // Monochrome keeps the square — it is the row's structure and
                  // its alignment anchor — but leaves it empty and outlined.
                  color: root.panelColored ? root.swatchColor(supplyRow.modelData) : "transparent"
                  border.width: 1
                  border.color: root.alphaColor(root.fg, root.panelColored ? 0.35 : 0.55)
                }

                Text {
                  id: supplyName
                  anchors.left: swatch.right
                  anchors.leftMargin: Style.space(9)
                  anchors.verticalCenter: parent.verticalCenter
                  width: Style.space(76)
                  elide: Text.ElideRight
                  text: root.supplyLabel(supplyRow.modelData)
                  textFormat: Text.PlainText
                  color: root.fg
                  font.family: root.family
                  font.pixelSize: Style.font.body
                }

                Text {
                  id: supplyValue
                  anchors.right: parent.right
                  anchors.rightMargin: Style.space(2)
                  anchors.verticalCenter: parent.verticalCenter
                  width: Style.space(40)
                  horizontalAlignment: Text.AlignRight
                  // The figure counts up with its own bar: it reads the fill's
                  // animated width, the same quantity the fill's geometry uses,
                  // so they are synchronised structurally and not by two
                  // animations that happen to share a duration. Rounded the way
                  // levelText() prints it — level_pct is a whole number — so
                  // the last frame lands exactly on the real value.
                  // Sentinels (∞, ok, ?) have no meter, so they stay static.
                  text: supplyRow.hasPct
                    ? Math.round(fill.shownPct) + "%"
                    : root.levelText(supplyRow.modelData)
                  textFormat: Text.PlainText
                  // The supply's own ink color, matching its bar, through the
                  // contrast floor as a safety net. See legibleOnCard().
                  // Monochrome: plain foreground, like the fill it labels.
                  color: supplyRow.hasPct
                    ? root.legibleOnCard(root.inkFillColor(supplyRow.modelData))
                    : root.mutedFg
                  font.family: root.family
                  font.pixelSize: Style.font.body
                  font.bold: supplyRow.needsAttention
                }

                Rectangle {
                  id: track
                  anchors.left: supplyName.right
                  anchors.leftMargin: Style.space(10)
                  anchors.right: supplyValue.left
                  anchors.rightMargin: Style.space(10)
                  anchors.verticalCenter: parent.verticalCenter
                  height: Style.space(8)
                  radius: height / 2
                  color: root.trackFill
                  // Severity's home on the meter: a low/critical supply rings
                  // its whole track in urgent, so a nearly-empty bar (whose
                  // fill is a short stub, or nothing at all at 0%) still
                  // announces itself. One carrier only — the fill keeps saying "how much
                  // of which ink", never "how bad".
                  border.width: supplyRow.needsAttention ? Math.max(1, Style.spacing.hairline) : 0
                  border.color: root.alphaColor(root.severityColor(supplyRow.sevState),
                                                supplyRow.sevState === "critical" ? 0.85 : 0.45)
                  Behavior on border.color { ColorAnimation { duration: 160 } }

                  Rectangle {
                    id: fill
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    radius: parent.radius
                    // Width at the current level; the open sweep scales it by
                    // openProgress so the meter fills 0 → level in 200ms.
                    readonly property real fullWidth: supplyRow.hasPct
                      ? Math.max(supplyRow.pct > 0 ? track.height : 0, Math.round(track.width * supplyRow.pct / 100))
                      : 0
                    width: Math.round(fullWidth * root.openProgress)

                    // What this fill is painting right now, read straight off
                    // its own ANIMATED width. The figure beside the bar binds
                    // to this, so the two cannot drift: one animated quantity
                    // drives both, through the open sweep and through the 160ms
                    // refresh transition alike.
                    //
                    // Scaled against fullWidth rather than against the track,
                    // because fullWidth carries a minimum stub so a 1% supply
                    // still shows something: measuring the track directly would
                    // read that stub back as several percent and leave the
                    // resting figure sitting above the real level.
                    readonly property real shownPct: fullWidth > 0
                      ? supplyRow.pct * Math.min(1, width / fullWidth)
                      : 0

                    // This supply's ink at full strength, every level. See the
                    // note by trackFill for why this is flat and not a ramp.
                    color: root.inkFillColor(supplyRow.modelData)

                    // Yield to the sweep while it drives the width — otherwise
                    // this Behavior would re-smooth every sweep frame. Data
                    // refreshes while open still glide over 160ms. Gated on the
                    // openSweeping flag (raised before the jump to 0), not on
                    // openSweep.running, so the collapse can't race the gate.
                    Behavior on width {
                      enabled: !root.openSweeping
                      NumberAnimation { duration: 160; easing.type: Easing.OutCubic }
                    }
                    Behavior on color { ColorAnimation { duration: 160 } }
                  }
                }
              }
            }
          }

          PanelSeparator {
            visible: statsFlow.visible
            foreground: root.fg
          }

          // ---- Paper and counters as a compact stat row: one column per
          //      tray, then the job queue and the lifetime page counter.
          Flow {
            id: statsFlow
            visible: root.trays.length > 0 || root.jobCount > 0 || root.impressions !== null
            width: parent.width
            spacing: Style.space(24)

            Repeater {
              model: root.trays

              Column {
                id: trayStat
                required property var modelData
                spacing: Style.space(3)

                Text {
                  text: String(trayStat.modelData.name).toUpperCase()
                  textFormat: Text.PlainText
                  color: Qt.darker(root.fg, 1.55)
                  font.family: root.family
                  font.pixelSize: Style.font.caption
                  font.bold: true
                  font.letterSpacing: 1
                }
                Text {
                  text: trayStat.modelData.empty ? "EMPTY" : root.levelText(trayStat.modelData)
                  textFormat: Text.PlainText
                  color: trayStat.modelData.empty ? root.urgent : root.fg
                  font.family: root.family
                  font.pixelSize: Style.font.title
                  font.bold: trayStat.modelData.empty
                }
              }
            }

            Column {
              visible: root.jobCount > 0
              spacing: Style.space(3)

              Text {
                text: "JOBS"
                color: Qt.darker(root.fg, 1.55)
                font.family: root.family
                font.pixelSize: Style.font.caption
                font.bold: true
                font.letterSpacing: 1
              }
              Text {
                text: String(root.jobCount)
                color: root.accent
                font.family: root.family
                font.pixelSize: Style.font.title
                font.bold: true
              }
            }

            Column {
              visible: root.impressions !== null
              spacing: Style.space(3)

              Text {
                text: "PAGES"
                color: Qt.darker(root.fg, 1.55)
                font.family: root.family
                font.pixelSize: Style.font.caption
                font.bold: true
                font.letterSpacing: 1
              }
              Text {
                text: root.impressions !== null ? Number(root.impressions).toLocaleString(Qt.locale(), "f", 0) : ""
                color: root.fg
                font.family: root.family
                font.pixelSize: Style.font.title
              }
            }
          }

          // ---- First-poll placeholder.
          Text {
            visible: root.report === null && root.errorText === ""
            width: parent.width
            topPadding: Style.space(10)
            text: "Polling printer…"
            color: Qt.darker(root.fg, 1.55)
            font.family: root.family
            font.pixelSize: Style.font.body
            font.italic: true
            horizontalAlignment: Text.AlignHCenter
          }

          // ---- Error banner: last-known-good data stays rendered above it.
          Rectangle {
            visible: root.errorText !== ""
            width: parent.width
            implicitHeight: errorLabel.implicitHeight + Style.space(16)
            radius: Style.cornerRadius
            color: root.alphaColor(root.urgent, 0.10)
            border.width: 1
            border.color: root.alphaColor(root.urgent, 0.35)

            Text {
              id: errorLabel
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              anchors.leftMargin: Style.space(12)
              anchors.rightMargin: Style.space(12)
              text: root.errorText
              textFormat: Text.PlainText
              color: Qt.darker(root.fg, 1.4)
              font.family: root.family
              font.pixelSize: Style.font.caption
              wrapMode: Text.Wrap
            }
          }

          // ---- Freshness footer: when the data is from, plus an inline
          //      refresh. The button re-runs the CLI right now — the same
          //      forced refresh the bar's middle-click does — so a stale panel
          //      can be corrected without closing it, and it is disabled while
          //      a fetch is already in flight so clicks cannot queue up. The
          //      rule and the row are always shown: the button has to stay
          //      reachable exactly when there is no timestamp to print yet.
          PanelSeparator {
            foreground: root.fg
          }

          Item {
            width: parent.width
            implicitHeight: Math.max(footerLabel.implicitHeight, refreshButton.implicitHeight)

            Text {
              id: footerLabel
              anchors.left: parent.left
              anchors.right: refreshButton.left
              anchors.rightMargin: Style.spacing.sm
              anchors.verticalCenter: parent.verticalCenter
              // printbar polls the printer on every run, so there is no cached
              // state to go stale and no suffix to tint — the timestamp is the
              // whole footer.
              text: root.updatedText !== "" ? "󰅐  Updated " + root.updatedText : ""
              textFormat: Text.PlainText
              color: Qt.darker(root.fg, 1.55)
              font.family: root.family
              font.pixelSize: Style.font.caption
              elide: Text.ElideRight
            }

            PanelActionButton {
              id: refreshButton
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              // nf-md-refresh (U+F0450). Written literally: a JS "\\u" escape takes
              // exactly FOUR hex digits, so "\\uf0450" is U+F045 followed by a "0".
              iconText: "󰑐"
              tooltipText: "Refresh now"
              foreground: Qt.darker(root.fg, 1.55)
              hoverColor: root.fg
              fontFamily: root.family
              fontSize: Style.font.caption
              size: Style.space(20)
              enabled: !root.fetchBusy
              onClicked: root.refresh()
            }
          }
        }
      }
    }
  }
}
