package deej

import (
	_ "embed"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"unsafe"

	webview2 "github.com/jchv/go-webview2"
	"golang.org/x/sys/windows"

	"github.com/omriharel/deej/pkg/deej/icon"
)

//go:embed assets/settings.html
var settingsPageHTML string

var (
	user32               = windows.NewLazySystemDLL("user32.dll")
	procShowWindow       = user32.NewProc("ShowWindow")
	procSetForeground    = user32.NewProc("SetForegroundWindow")
	procIsIconic         = user32.NewProc("IsIconic")
	procGetDpiForSystem  = user32.NewProc("GetDpiForSystem")
	procLoadImageW       = user32.NewProc("LoadImageW")
	procSendMessageW     = user32.NewProc("SendMessageW")
	settingsWindowLock   sync.Mutex
	settingsWindowHandle uintptr
)

const (
	swRestore = 9

	imageIcon       = 1
	lrLoadFromFile  = 0x10
	lrDefaultSize   = 0x40
	wmSetIcon       = 0x80
	iconSmall       = 0
	iconBig         = 1
	settingsWidth   = 860
	settingsHeight  = 480
	defaultDPI      = 96
	settingsDataDir = "deej"
)

// openSettingsWindow shows the settings window, or brings it to the front if it's already open
func (d *Deej) openSettingsWindow() {
	settingsWindowLock.Lock()
	defer settingsWindowLock.Unlock()

	if settingsWindowHandle != 0 {
		if minimized, _, _ := procIsIconic.Call(settingsWindowHandle); minimized != 0 {
			procShowWindow.Call(settingsWindowHandle, swRestore)
		}

		procSetForeground.Call(settingsWindowHandle)
		return
	}

	// mark it as opening, so double clicks don't create two windows
	settingsWindowHandle = ^uintptr(0)

	go d.runSettingsWindow()
}

func (d *Deej) runSettingsWindow() {
	logger := d.logger.Named("settings")

	// the webview and its message loop must live on a single OS thread
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	defer func() {
		settingsWindowLock.Lock()
		settingsWindowHandle = 0
		settingsWindowLock.Unlock()
	}()

	scale := 1.0
	if dpi, _, _ := procGetDpiForSystem.Call(); dpi > 0 {
		scale = float64(dpi) / defaultDPI
	}

	dataPath := filepath.Join(os.Getenv("LOCALAPPDATA"), settingsDataDir, "webview")

	view := webview2.NewWithOptions(webview2.WebViewOptions{
		AutoFocus: true,
		DataPath:  dataPath,
		WindowOptions: webview2.WindowOptions{
			Title:  "deej settings",
			Width:  uint(settingsWidth * scale),
			Height: uint(settingsHeight * scale),
			Center: true,
		},
	})

	if view == nil {
		logger.Warn("Failed to create settings window, WebView2 runtime may be missing")
		d.notifier.Notify("Can't open settings",
			"deej's settings window needs the Microsoft Edge WebView2 Runtime. You can still edit config.yaml from the tray menu.")
		return
	}

	hwnd := uintptr(view.Window())

	settingsWindowLock.Lock()
	settingsWindowHandle = hwnd
	settingsWindowLock.Unlock()

	setWindowIcon(hwnd)

	var connectedOnce sync.Once

	bindings := map[string]interface{}{
		"deejState": func() settingsState {
			connectedOnce.Do(func() { logger.Debug("Settings page connected") })
			return d.settingsState()
		},
		"deejPositions": func() []float32 {
			return d.sliderPositions()
		},
		"deejSetSensitivity": func(slider string, field string, value float64, clear bool) error {
			return d.setSensitivity(slider, field, value, clear)
		},
		"deejOverrideSensitivity": func(slider int, curve float64, maxVolume float64) error {
			return d.overrideSensitivity(slider, curve, maxVolume)
		},
		"deejSetOption": func(name string, value string) error {
			return d.setOption(name, value)
		},
		"deejAction": func(name string) error {
			return d.runAction(name)
		},
	}

	for name, f := range bindings {
		if err := view.Bind(name, f); err != nil {
			logger.Warnw("Failed to bind settings function", "name", name, "error", err)
		}
	}

	view.SetHtml(settingsPageHTML)

	logger.Info("Settings window opened")
	view.Run()
	logger.Info("Settings window closed")
}

// setWindowIcon gives the settings window deej's icon (the notifier keeps a copy of it on disk)
func setWindowIcon(hwnd uintptr) {
	path := filepath.Join(os.TempDir(), "deej.ico")
	if _, err := os.Stat(path); err != nil {
		os.WriteFile(path, icon.DeejLogo, 0644)
	}

	iconPath, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return
	}

	for _, size := range []uintptr{iconSmall, iconBig} {
		hIcon, _, _ := procLoadImageW.Call(0, uintptr(unsafe.Pointer(iconPath)), imageIcon, 0, 0, lrLoadFromFile|lrDefaultSize)
		if hIcon != 0 {
			procSendMessageW.Call(hwnd, wmSetIcon, size, hIcon)
		}
	}
}
