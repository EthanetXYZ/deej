package deej

import (
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/omriharel/deej/pkg/deej/util"
)

// this file holds the platform-independent logic behind the settings window:
// a snapshot of everything it displays, and the changes it's allowed to make.
//
// the window calls into these functions on its UI thread, so they must return quickly: changes are
// applied in memory right away (so the volume follows the controls instantly), and the config file
// is written in the background once the user pauses

const (
	maxSettingsSliders = 64

	// how long to wait after the last change before writing the config file
	settingsSaveDelay = 400 * time.Millisecond

	// how often slow-to-read info (serial ports, autostart) is refreshed
	settingsSlowInfoInterval = 2 * time.Second
)

type settingsState struct {
	Status    ConnectionStatus `json:"status"`
	Ports     []string         `json:"ports"`
	COMPort   string           `json:"comPort"`
	Invert    bool             `json:"invert"`
	Noise     string           `json:"noise"`
	Autostart struct {
		Supported bool `json:"supported"`
		Enabled   bool `json:"enabled"`
	} `json:"autostart"`

	Presets  []settingsPreset `json:"presets"`
	Defaults settingsCurve    `json:"defaults"`
	Sliders  []settingsSlider `json:"sliders"`
	Version  string           `json:"version"`

	// apps deej.unmapped leaves alone, and the apps currently playing audio (to pick from)
	UnmappedExclude []string `json:"unmappedExclude"`
	AudioApps       []string `json:"audioApps"`
}

type settingsPreset struct {
	Key         string  `json:"key"`
	Name        string  `json:"name"`
	Value       float64 `json:"value"`
	Description string  `json:"description"`
}

type settingsCurve struct {
	Curve     float64 `json:"curve"`
	MaxVolume float64 `json:"maxVolume"` // 1 - 100
}

type settingsSlider struct {
	Index      int           `json:"index"`
	Targets    []string      `json:"targets"`
	UseDefault bool          `json:"useDefault"`
	Effective  settingsCurve `json:"effective"`
	Position   float32       `json:"position"` // 0 - 1, or -1 if unknown
	Volume     float32       `json:"volume"`   // 0 - 1, or -1 if unknown
}

// settingsBackend holds the state needed to keep the settings window snappy
type settingsBackend struct {
	lock sync.Mutex

	// edits waiting to be written to the config file
	pendingEdits []configEdit
	saveTimer    *time.Timer

	// cached info that's too slow to read on every refresh
	slowInfoAt       time.Time
	slowInfoBusy     bool
	ports            []string
	autostartEnabled bool
}

func (d *Deej) settingsBackend() *settingsBackend {
	d.settingsOnce.Do(func() {
		d.settings = &settingsBackend{}
		d.refreshSlowSettingsInfo()
	})

	return d.settings
}

// refreshSlowSettingsInfo re-reads the serial port list and autostart state
func (d *Deej) refreshSlowSettingsInfo() {
	ports := AvailablePorts()
	autostart := util.AutostartEnabled()

	b := d.settings
	b.lock.Lock()
	b.ports = ports
	b.autostartEnabled = autostart
	b.slowInfoAt = time.Now()
	b.slowInfoBusy = false
	b.lock.Unlock()
}

// slowSettingsInfo returns the cached port list and autostart state, refreshing them in the background when stale
func (d *Deej) slowSettingsInfo() ([]string, bool) {
	b := d.settingsBackend()

	b.lock.Lock()
	defer b.lock.Unlock()

	if !b.slowInfoBusy && time.Since(b.slowInfoAt) > settingsSlowInfoInterval {
		b.slowInfoBusy = true
		go d.refreshSlowSettingsInfo()
	}

	return append([]string(nil), b.ports...), b.autostartEnabled
}

// queueConfigEdits schedules edits to be written to the config file after a short pause.
// newer edits replace older ones for the same key (or anything nested under it)
func (d *Deej) queueConfigEdits(delay time.Duration, edits ...configEdit) {
	b := d.settingsBackend()

	b.lock.Lock()
	defer b.lock.Unlock()

	for _, edit := range edits {
		prefix := strings.Join(edit.Path, "\x00")

		kept := b.pendingEdits[:0]
		for _, pending := range b.pendingEdits {
			pendingKey := strings.Join(pending.Path, "\x00")
			if pendingKey != prefix && !strings.HasPrefix(pendingKey, prefix+"\x00") {
				kept = append(kept, pending)
			}
		}

		b.pendingEdits = append(kept, edit)
	}

	if b.saveTimer != nil {
		b.saveTimer.Stop()
	}

	b.saveTimer = time.AfterFunc(delay, d.flushConfigEdits)
}

// flushConfigEdits writes any queued edits to the config file now
func (d *Deej) flushConfigEdits() {
	b := d.settingsBackend()

	b.lock.Lock()
	edits := b.pendingEdits
	b.pendingEdits = nil
	if b.saveTimer != nil {
		b.saveTimer.Stop()
		b.saveTimer = nil
	}
	b.lock.Unlock()

	if len(edits) == 0 {
		return
	}

	if err := d.config.Edit(edits...); err != nil {
		d.logger.Warnw("Failed to save settings", "error", err)
	}
}

func (d *Deej) settingsState() settingsState {
	config := d.config
	ports, autostartEnabled := d.slowSettingsInfo()

	state := settingsState{
		Status:  d.serial.Status(),
		Ports:   ports,
		COMPort: config.ConnectionInfo.COMPort,
		Invert:  config.InvertSliders,
		Noise:   config.NoiseReductionLevel,
		Version: d.version,
	}

	if state.Noise != "low" && state.Noise != "high" {
		state.Noise = "default"
	}

	state.Autostart.Supported = util.AutostartSupported()
	state.Autostart.Enabled = autostartEnabled

	state.UnmappedExclude = append([]string{}, config.UnmappedExclude()...)
	state.AudioApps = d.sessions.processKeys()

	for _, preset := range curvePresets {
		state.Presets = append(state.Presets, settingsPreset{preset.Key, preset.Name, preset.Value, preset.Description})
	}

	defaults := config.SliderSensitivity.getDefault()
	state.Defaults = settingsCurve{Curve: defaults.Curve, MaxVolume: math.Round(defaults.MaxVolume * 100)}

	// show every slider that's either mapped in the config or reported by the board
	positions := d.serial.SliderPositions()
	indexes := map[int]bool{}

	for idx := range positions {
		indexes[idx] = true
	}

	config.SliderMapping.iterate(func(idx int, _ []string) {
		indexes[idx] = true
	})

	for idx := range indexes {
		if idx < 0 || idx >= maxSettingsSliders {
			continue
		}

		targets, _ := config.SliderMapping.get(idx)
		override := config.SliderSensitivity.getOverride(idx)
		sensitivity := config.SliderSensitivity.get(idx)

		slider := settingsSlider{
			Index:      idx,
			Targets:    append([]string{}, targets...),
			UseDefault: override.Curve == nil && override.MaxVolume == nil,
			Effective:  settingsCurve{Curve: sensitivity.Curve, MaxVolume: math.Round(sensitivity.MaxVolume * 100)},
			Position:   -1,
			Volume:     -1,
		}

		if idx < len(positions) && positions[idx] >= 0 {
			slider.Position = positions[idx]
			slider.Volume = sensitivity.apply(positions[idx])
		}

		state.Sliders = append(state.Sliders, slider)
	}

	sort.Slice(state.Sliders, func(i, j int) bool { return state.Sliders[i].Index < state.Sliders[j].Index })

	return state
}

// sliderPositions is the cheap, high-frequency part of the state: just where each slider is (-1 if unknown)
func (d *Deej) sliderPositions() []float32 {
	positions := d.serial.SliderPositions()
	if positions == nil {
		positions = []float32{}
	}

	return positions
}

func parseSettingsSlider(slider string) (int, string, error) {
	if slider == sensitivityDefaultKey {
		return -1, sensitivityDefaultKey, nil
	}

	idx, err := strconv.Atoi(slider)
	if err != nil || idx < 0 || idx >= maxSettingsSliders {
		return 0, "", fmt.Errorf("invalid slider %q", slider)
	}

	return idx, strconv.Itoa(idx), nil
}

// applySensitivityNow updates sensitivity in memory and re-applies the affected volumes immediately
func (d *Deej) applySensitivityNow(sliderIdx int, field string, value *float64) {
	d.config.SliderSensitivity.setField(sliderIdx, field, value)

	if sliderIdx < 0 {
		d.serial.ResendAllSliders()
	} else {
		d.serial.ResendSlider(sliderIdx)
	}
}

// setSensitivity changes one sensitivity field. slider is "default" or a slider index,
// field is "curve", "maxVolume" or "all" (only valid with clear, to go back to the default)
func (d *Deej) setSensitivity(slider string, field string, value float64, clear bool) error {
	sliderIdx, section, err := parseSettingsSlider(slider)
	if err != nil {
		return err
	}

	path := []string{configKeySliderSensitivity, section}

	switch {
	case clear && field == "all":
		d.applySensitivityNow(sliderIdx, sensitivityKeyCurve, nil)
		d.applySensitivityNow(sliderIdx, sensitivityKeyMaxVolume, nil)
		d.queueConfigEdits(0, configEdit{Path: path, Value: nil})

	case field == "curve":
		if clear {
			d.applySensitivityNow(sliderIdx, sensitivityKeyCurve, nil)
			d.queueConfigEdits(0, configEdit{Path: append(path, sensitivityKeyCurve), Value: nil})
			return nil
		}

		if math.IsNaN(value) || value < minCurve || value > maxCurve {
			return fmt.Errorf("curve must be between %.1f and %.1f", minCurve, maxCurve)
		}

		curve := math.Round(value*100) / 100
		d.applySensitivityNow(sliderIdx, sensitivityKeyCurve, &curve)
		d.queueConfigEdits(settingsSaveDelay, configEdit{Path: append(path, sensitivityKeyCurve), Value: curve})

	case field == "maxVolume":
		if clear {
			d.applySensitivityNow(sliderIdx, sensitivityKeyMaxVolume, nil)
			d.queueConfigEdits(0, configEdit{Path: append(path, sensitivityKeyMaxVolume), Value: nil})
			return nil
		}

		if math.IsNaN(value) || value < 1 || value > 100 {
			return fmt.Errorf("max volume must be between 1 and 100")
		}

		maxVolume := math.Round(value)
		scalar := maxVolume / 100
		d.applySensitivityNow(sliderIdx, sensitivityKeyMaxVolume, &scalar)
		d.queueConfigEdits(settingsSaveDelay, configEdit{Path: append(path, sensitivityKeyMaxVolume), Value: int(maxVolume)})

	default:
		return fmt.Errorf("invalid sensitivity field %q", field)
	}

	return nil
}

// overrideSensitivity makes a slider stop following the default, starting from the given values
func (d *Deej) overrideSensitivity(slider int, curve float64, maxVolume float64) error {
	if slider < 0 || slider >= maxSettingsSliders {
		return fmt.Errorf("invalid slider %d", slider)
	}

	if math.IsNaN(curve) || curve < minCurve || curve > maxCurve || math.IsNaN(maxVolume) || maxVolume < 1 || maxVolume > 100 {
		return fmt.Errorf("invalid sensitivity values")
	}

	curve = math.Round(curve*100) / 100
	maxVolume = math.Round(maxVolume)
	scalar := maxVolume / 100

	d.applySensitivityNow(slider, sensitivityKeyCurve, &curve)
	d.applySensitivityNow(slider, sensitivityKeyMaxVolume, &scalar)

	section := strconv.Itoa(slider)
	d.queueConfigEdits(settingsSaveDelay,
		configEdit{Path: []string{configKeySliderSensitivity, section, sensitivityKeyCurve}, Value: curve},
		configEdit{Path: []string{configKeySliderSensitivity, section, sensitivityKeyMaxVolume}, Value: int(maxVolume)},
	)

	return nil
}

// setOption changes one of the general settings
func (d *Deej) setOption(name string, value string) error {
	switch name {
	case "invert":
		d.queueConfigEdits(0, configEdit{Path: []string{configKeyInvertSliders}, Value: value == "true"})

	case "noise":
		if value != "low" && value != "default" && value != "high" {
			return fmt.Errorf("invalid noise reduction level %q", value)
		}

		d.queueConfigEdits(0, configEdit{Path: []string{configKeyNoiseReductionLevel}, Value: value})

	case "comPort":
		if value == "" || len(value) > 64 {
			return fmt.Errorf("invalid port %q", value)
		}

		d.queueConfigEdits(0, configEdit{Path: []string{configKeyCOMPort}, Value: value})

	case "autostart":
		if !util.AutostartSupported() {
			return fmt.Errorf("starting automatically isn't supported on this platform")
		}

		if err := util.SetAutostart(value == "true"); err != nil {
			return err
		}

		go d.refreshSlowSettingsInfo()

	default:
		return fmt.Errorf("unknown option %q", name)
	}

	return nil
}

// setUnmappedExclude replaces the list of apps that deej.unmapped leaves alone
func (d *Deej) setUnmappedExclude(names []string) error {
	names = normalizeProcessNames(names)

	if len(names) > 100 {
		return fmt.Errorf("too many apps")
	}

	for _, name := range names {
		if len(name) > 260 || strings.ContainsAny(name, "\r\n") {
			return fmt.Errorf("invalid app name %q", name)
		}
	}

	// apply right away, then save in the background
	d.config.SetUnmappedExclude(names)

	// an empty list is written as [] rather than removing the key, which would also drop its comment
	d.queueConfigEdits(0, configEdit{Path: []string{configKeyUnmappedExclude}, Value: names})

	return nil
}

// runAction performs one of the one-off actions offered by the settings window
func (d *Deej) runAction(name string) error {
	switch name {
	case "editConfig":

		// make sure the file is up to date before opening it
		d.flushConfigEdits()

		editor := "notepad.exe"
		if util.Linux() {
			editor = "gedit"
		}

		return util.OpenExternal(d.logger, editor, userConfigFilepath)

	case "rescan":

		// performance: users can't click this fast enough for a forced refresh to matter.
		// this can take a moment, so don't hold up the caller (which may be the settings window)
		go func() {
			d.sessions.refreshSessions(true)
			d.serial.ResendAllSliders()
		}()

		return nil
	}

	return fmt.Errorf("unknown action %q", name)
}
