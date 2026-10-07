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

	// windows limits tray tooltips to 127 characters
	maxTooltipLength = 127
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
	tooltip := "deej"

	switch {
	case status.Connected:
		statusLine = fmt.Sprintf("Connected (%s)", status.Port)
		tooltip += " - " + status.Port
	case status.Error != "":
		statusLine = fmt.Sprintf("%s: %s - retrying...", status.Port, status.Error)
		tooltip += " - " + status.Port + " " + status.Error
	}

	t.status.SetTitle(statusLine)

	lines := []string{tooltip}
	for idx, position := range t.deej.serial.SliderPositions() {
		if position < 0 {
			continue
		}

		volume := t.deej.config.SliderSensitivity.get(idx).apply(position)
		targets, _ := t.deej.config.SliderMapping.get(idx)
		lines = append(lines, fmt.Sprintf("%s: %s", describeTargets(targets, 14), formatVolume(volume)))
	}

	tooltip = strings.Join(lines, "\n")
	if len(tooltip) > maxTooltipLength {
		tooltip = tooltip[:maxTooltipLength]
	}

	if tooltip != t.lastTooltip {
		systray.SetTooltip(tooltip)
		t.lastTooltip = tooltip
	}
}

func describeTargets(targets []string, maxLength int) string {
	if len(targets) == 0 {
		return "(unmapped)"
	}

	names := make([]string, len(targets))
	for idx, target := range targets {
		names[idx] = strings.TrimSuffix(target, ".exe")
	}

	result := strings.Join(names, ", ")
	if len(result) > maxLength {
		result = result[:maxLength-3] + "..."
	}

	return result
}

// formatVolume shows low volumes with a decimal so fine-grained curves are visible (e.g. "0.4%")
func formatVolume(volume float32) string {
	percent := float64(volume) * 100
	if percent > 0 && percent < 10 {
		return strconv.FormatFloat(percent, 'f', 1, 64) + "%"
	}

	return fmt.Sprintf("%.0f%%", percent)
}
