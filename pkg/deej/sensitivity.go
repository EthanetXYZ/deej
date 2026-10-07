package deej

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"sync"
)

// sliderSensitivity controls how a slider's physical position is translated into a volume level.
//
// the output volume is calculated as: maxVolume * position ^ curve
//
// a curve of 1.0 is linear (deej's original behavior). higher curves make the lower part of the
// slider's travel much finer, which helps a lot because a linear 10% is still pretty loud to human ears
type sliderSensitivity struct {
	Curve     float64
	MaxVolume float64 // 0.0 - 1.0
}

// curvePreset is a named response curve that's selectable from the tray menu
type curvePreset struct {
	Key         string
	Name        string
	Value       float64
	Description string
}

const (
	sensitivityDefaultKey = "default"

	sensitivityKeyCurve     = "curve"
	sensitivityKeyMaxVolume = "max_volume"

	minCurve = 0.2
	maxCurve = 6.0
)

var curvePresets = []curvePreset{
	{"linear", "Linear (original deej)", 1.0, "Volume follows the slider exactly - 10% slider = 10% volume"},
	{"gentle", "Gentle", 1.5, "Slightly finer control at low volumes - 10% slider = ~3% volume"},
	{"balanced", "Balanced", 2.0, "Recommended - 10% slider = 1% volume, 50% slider = 25% volume"},
	{"fine", "Fine", 2.5, "Very fine control at low volumes - 50% slider = ~18% volume"},
	{"very_fine", "Very fine", 3.0, "Maximum precision at low volumes - 50% slider = ~13% volume"},
}

var defaultSensitivity = sliderSensitivity{Curve: 1.0, MaxVolume: 1.0}

// apply maps a slider position (0.0 - 1.0) to a volume level (0.0 - 1.0)
func (s sliderSensitivity) apply(position float32) float32 {
	p := math.Max(0, math.Min(1, float64(position)))

	v := s.MaxVolume * math.Pow(p, s.Curve)

	return float32(math.Max(0, math.Min(1, v)))
}

func (s sliderSensitivity) String() string {
	return fmt.Sprintf("<curve: %.2f, max: %.0f%%>", s.Curve, s.MaxVolume*100)
}

// sensitivityOverride holds the user-provided values for a single slider (or for the default),
// where nil means "not set, fall back to the default"
type sensitivityOverride struct {
	Curve     *float64
	MaxVolume *float64
}

// sensitivityMap holds the global default along with per-slider overrides
type sensitivityMap struct {
	lock      sync.RWMutex
	defaults  sensitivityOverride
	overrides map[int]sensitivityOverride
}

func newSensitivityMap() *sensitivityMap {
	return &sensitivityMap{overrides: map[int]sensitivityOverride{}}
}

// sensitivityMapFromConfig parses the "slider_sensitivity" config section, which looks like:
//
//	slider_sensitivity:
//	  default:
//	    curve: 2
//	    max_volume: 100
//	  0:
//	    curve: balanced
//	    max_volume: 80
func sensitivityMapFromConfig(raw map[string]interface{}) (*sensitivityMap, []error) {
	result := newSensitivityMap()
	var errs []error

	for key, value := range raw {
		fields, ok := value.(map[string]interface{})
		if !ok {
			errs = append(errs, fmt.Errorf("slider_sensitivity.%s: expected a section with curve/max_volume", key))
			continue
		}

		override, err := parseSensitivityOverride(fields)
		if err != nil {
			errs = append(errs, fmt.Errorf("slider_sensitivity.%s: %w", key, err))
		}

		if strings.EqualFold(key, sensitivityDefaultKey) {
			result.defaults = override
			continue
		}

		sliderIdx, err := strconv.Atoi(key)
		if err != nil || sliderIdx < 0 {
			errs = append(errs, fmt.Errorf("slider_sensitivity: %q isn't a slider index or \"default\"", key))
			continue
		}

		result.overrides[sliderIdx] = override
	}

	return result, errs
}

func parseSensitivityOverride(fields map[string]interface{}) (sensitivityOverride, error) {
	var override sensitivityOverride
	var firstErr error

	if raw, ok := fields[sensitivityKeyCurve]; ok && raw != nil {
		if curve, err := parseCurve(raw); err != nil {
			firstErr = err
		} else {
			override.Curve = &curve
		}
	}

	if raw, ok := fields[sensitivityKeyMaxVolume]; ok && raw != nil {
		if maxVolume, err := parseNumber(raw); err != nil || maxVolume < 1 || maxVolume > 100 {
			if firstErr == nil {
				firstErr = fmt.Errorf("max_volume must be a number between 1 and 100, got %v", raw)
			}
		} else {
			maxVolume /= 100
			override.MaxVolume = &maxVolume
		}
	}

	return override, firstErr
}

// parseCurve accepts either a number or the name of a preset (case-insensitive, e.g. "balanced")
func parseCurve(raw interface{}) (float64, error) {
	if name, ok := raw.(string); ok {
		normalized := strings.ToLower(strings.TrimSpace(name))
		for _, preset := range curvePresets {
			if normalized == preset.Key || normalized == strings.ReplaceAll(preset.Key, "_", " ") {
				return preset.Value, nil
			}
		}
	}

	curve, err := parseNumber(raw)
	if err != nil || curve < minCurve || curve > maxCurve {
		return 0, fmt.Errorf("curve must be a preset name or a number between %.1f and %.1f, got %v", minCurve, maxCurve, raw)
	}

	return curve, nil
}

func parseNumber(raw interface{}) (float64, error) {
	switch v := raw.(type) {
	case int:
		return float64(v), nil
	case int64:
		return float64(v), nil
	case float64:
		return v, nil
	case float32:
		return float64(v), nil
	case string:
		return strconv.ParseFloat(strings.TrimSuffix(strings.TrimSpace(v), "%"), 64)
	}

	return 0, fmt.Errorf("not a number: %v", raw)
}

// get returns the effective sensitivity for a slider, combining its overrides with the defaults
func (m *sensitivityMap) get(sliderIdx int) sliderSensitivity {
	m.lock.RLock()
	defer m.lock.RUnlock()

	result := m.defaultsLocked()
	override := m.overrides[sliderIdx]

	if override.Curve != nil {
		result.Curve = *override.Curve
	}

	if override.MaxVolume != nil {
		result.MaxVolume = *override.MaxVolume
	}

	return result
}

// setField changes a single field in memory, for instant feedback while the config file catches up.
// sliderIdx -1 means the default. a nil value removes the override
func (m *sensitivityMap) setField(sliderIdx int, field string, value *float64) {
	m.lock.Lock()
	defer m.lock.Unlock()

	override := m.defaults
	if sliderIdx >= 0 {
		override = m.overrides[sliderIdx]
	}

	switch field {
	case sensitivityKeyCurve:
		override.Curve = value
	case sensitivityKeyMaxVolume:
		override.MaxVolume = value
	}

	if sliderIdx < 0 {
		m.defaults = override
	} else if override.Curve == nil && override.MaxVolume == nil {
		delete(m.overrides, sliderIdx)
	} else {
		m.overrides[sliderIdx] = override
	}
}

// getDefault returns the sensitivity used by sliders that don't override anything
func (m *sensitivityMap) getDefault() sliderSensitivity {
	m.lock.RLock()
	defer m.lock.RUnlock()

	return m.defaultsLocked()
}

// getOverride returns the raw per-slider override (fields are nil when the slider uses the default)
func (m *sensitivityMap) getOverride(sliderIdx int) sensitivityOverride {
	m.lock.RLock()
	defer m.lock.RUnlock()

	return m.overrides[sliderIdx]
}

func (m *sensitivityMap) defaultsLocked() sliderSensitivity {
	result := defaultSensitivity

	if m.defaults.Curve != nil {
		result.Curve = *m.defaults.Curve
	}

	if m.defaults.MaxVolume != nil {
		result.MaxVolume = *m.defaults.MaxVolume
	}

	return result
}

func (m *sensitivityMap) String() string {
	m.lock.RLock()
	defer m.lock.RUnlock()

	return fmt.Sprintf("<default: %s, %d slider overrides>", m.defaultsLocked(), len(m.overrides))
}

// formatCurve gives a short config-friendly representation of a curve value (e.g. 2 or 2.5)
func formatCurve(curve float64) string {
	return strconv.FormatFloat(curve, 'f', -1, 64)
}
