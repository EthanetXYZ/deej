package deej

import (
	"fmt"
	"strconv"
	"strings"
	"sync"
	"time"

	"fyne.io/systray"
	"go.uber.org/zap"

	"github.com/omriharel/deej/pkg/deej/icon"
	"github.com/omriharel/deej/pkg/deej/util"
)

const (
	trayRefreshInterval = 500 * time.Millisecond

	// the tray tooltip buffer holds 127 characters, but windows 11 only displays about the first 64
	maxTooltipLength = 63
)

type trayMenu struct {
	deej   *Deej
	logger *zap.SugaredLogger

	lock        sync.Mutex
	status      *systray.MenuItem
	lastTooltip string
}

func (d *Deej) initializeTray(onDone func()) {
	logger := d.logger.Named("tray")

	onReady := func() {
		logger.Debug("Tray instance ready")

		systray.SetTemplateIcon(icon.DeejLogo, icon.DeejLogo)
		systray.SetTitle("deej")
		systray.SetTooltip("deej")

		t := &trayMenu{deej: d, logger: logger}
		t.build()

		// left-clicking the tray icon opens the settings window, right-clicking shows the menu
		systray.SetOnTapped(d.openSettingsWindow)

		d.serial.SubscribeToStatusChanges(func(ConnectionStatus) {
			t.refresh()
		})

		go func() {
			for range time.Tick(trayRefreshInterval) {
				t.refresh()
			}
		}()

		// if deej was moved since autostart was enabled, point autostart at the new location
		if err := util.RefreshAutostart(); err != nil {
			logger.Warnw("Failed to refresh autostart entry", "error", err)
		}

		// actually start the main runtime
		onDone()
	}

	onExit := func() {
		logger.Debug("Tray exited")
	}

	// start the tray icon
	logger.Debug("Running in tray")
	systray.Run(onReady, onExit)
}

func (d *Deej) stopTray() {
	d.logger.Debug("Quitting tray")
	systray.Quit()
}

// onClick runs f every time the item is clicked
func onClick(item *systray.MenuItem, f func()) {
	go func() {
		for range item.ClickedCh {
			f()
		}
	}()
}

func (t *trayMenu) build() {
	d := t.deej

	t.status = systray.AddMenuItem("Connecting...", "Serial connection status")
	t.status.Disable()

	systray.AddSeparator()

	settings := systray.AddMenuItem("Open settings...", "Slider sensitivity, COM port and other options")
	settings.SetIcon(icon.EditConfig)
	onClick(settings, func() {
		t.logger.Info("Settings menu item clicked, opening settings window")
		d.openSettingsWindow()
	})

	editConfig := systray.AddMenuItem("Edit configuration", "Open config file with notepad")
	editConfig.SetIcon(icon.EditConfig)
	onClick(editConfig, func() {
		t.logger.Info("Edit config menu item clicked, opening config for editing")

		if err := d.runAction("editConfig"); err != nil {
			t.logger.Warnw("Failed to open config file for editing", "error", err)
		}
	})

	refreshSessions := systray.AddMenuItem("Re-scan audio sessions", "Manually refresh audio sessions if something's stuck")
	refreshSessions.SetIcon(icon.RefreshSessions)
	onClick(refreshSessions, func() {
		t.logger.Info("Refresh sessions menu item clicked, triggering session map refresh")
		d.runAction("rescan")
	})

	if d.version != "" {
		systray.AddSeparator()
		versionInfo := systray.AddMenuItem(d.version, "")
		versionInfo.Disable()
	}

	systray.AddSeparator()
	quit := systray.AddMenuItem("Quit", "Stop deej and quit")
	onClick(quit, func() {
		t.logger.Info("Quit menu item clicked, stopping")
		d.signalStop()
	})

	t.refresh()
}

// refresh updates the status line and the tooltip, which shows each slider's current volume
func (t *trayMenu) refresh() {
	t.lock.Lock()
	defer t.lock.Unlock()

	status := t.deej.serial.Status()

	statusLine := "Connecting..."
	header := "deej"

	switch {
	case status.Connected:
		statusLine = fmt.Sprintf("Connected (%s)", status.Port)
	case status.Error != "":
		statusLine = fmt.Sprintf("%s: %s - retrying...", status.Port, status.Error)
		header = fmt.Sprintf("deej: %s %s", status.Port, status.Error)
	}

	t.status.SetTitle(statusLine)

	var entries []tooltipEntry
	for idx, position := range t.deej.serial.SliderPositions() {
		targets, mapped := t.deej.config.SliderMapping.get(idx)
		if position < 0 || !mapped || len(targets) == 0 {
			continue
		}

		volume := t.deej.config.SliderSensitivity.get(idx).apply(position)
		entries = append(entries, tooltipEntry{targets: targets, volume: formatVolume(volume)})
	}

	tooltip := buildTooltip(header, entries)

	if tooltip != t.lastTooltip {
		systray.SetTooltip(tooltip)
		t.lastTooltip = tooltip
	}
}

// tooltipEntry is one slider's line in the tray tooltip
type tooltipEntry struct {
	targets []string
	volume  string
}

// names get shortened step by step until every slider fits in the tooltip
var tooltipNameLengths = []int{12, 8, 5}

// buildTooltip lists each slider's volume under the header, within maxTooltipLength. with many sliders, names
// are shortened until everything fits; if it still doesn't, it shows as many as fit and ends with "+N more".
// entries are never cut mid-way, so every number shown is complete
func buildTooltip(header string, entries []tooltipEntry) string {
	for _, nameLength := range tooltipNameLengths {
		if tooltip := joinTooltip(header, entries, nameLength); len(tooltip) <= maxTooltipLength {
			return tooltip
		}
	}

	shortest := tooltipNameLengths[len(tooltipNameLengths)-1]

	for shown := len(entries) - 1; shown >= 0; shown-- {
		tooltip := joinTooltip(header, entries[:shown], shortest) + fmt.Sprintf("\n+%d more", len(entries)-shown)
		if len(tooltip) <= maxTooltipLength {
			return tooltip
		}
	}

	return header
}

func joinTooltip(header string, entries []tooltipEntry, nameLength int) string {
	tooltip := header
	for _, entry := range entries {
		tooltip += "\n" + shortTargetName(entry.targets, nameLength) + " " + entry.volume
	}

	return tooltip
}

// shortTargetName gives a compact name for a slider's targets, e.g. "firefox+1" for firefox and brave
func shortTargetName(targets []string, maxNameLength int) string {
	name := strings.TrimSuffix(targets[0], ".exe")
	switch strings.ToLower(name) {
	case specialTargetTransformPrefix + specialTargetAllUnmapped:
		name = "unmapped"
	case specialTargetTransformPrefix + specialTargetCurrentWindow:
		name = "current app"
	}

	if len(name) > maxNameLength {
		name = name[:maxNameLength-1] + "~"
	}

	if len(targets) > 1 {
		name += fmt.Sprintf("+%d", len(targets)-1)
	}

	return name
}

// formatVolume shows low volumes with a decimal so fine-grained curves are visible (e.g. "0.4%")
func formatVolume(volume float32) string {
	percent := float64(volume) * 100
	if percent > 0 && percent < 10 {
		return strconv.FormatFloat(percent, 'f', 1, 64) + "%"
	}

	return fmt.Sprintf("%.0f%%", percent)
}
