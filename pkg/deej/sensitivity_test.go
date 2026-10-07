package deej

import (
	"math"
	"testing"
)

func TestSensitivityApply(t *testing.T) {
	cases := []struct {
		s        sliderSensitivity
		position float32
		want     float32
	}{
		{sliderSensitivity{Curve: 1, MaxVolume: 1}, 0.5, 0.5},
		{sliderSensitivity{Curve: 2, MaxVolume: 1}, 0.1, 0.01},
		{sliderSensitivity{Curve: 2, MaxVolume: 1}, 0.5, 0.25},
		{sliderSensitivity{Curve: 2, MaxVolume: 0.5}, 1, 0.5},
		{sliderSensitivity{Curve: 3, MaxVolume: 1}, 0, 0},
		{sliderSensitivity{Curve: 3, MaxVolume: 1}, 1, 1},
		{sliderSensitivity{Curve: 1, MaxVolume: 1}, 1.5, 1},
	}

	for _, c := range cases {
		if got := c.s.apply(c.position); math.Abs(float64(got-c.want)) > 0.0001 {
			t.Errorf("%s.apply(%v) = %v, want %v", c.s, c.position, got, c.want)
		}
	}
}

func TestSensitivityMapFromConfig(t *testing.T) {
	m, errs := sensitivityMapFromConfig(map[string]interface{}{
		"default": map[string]interface{}{"curve": 2, "max_volume": 90},
		"1":       map[string]interface{}{"curve": "very fine"},
		"2":       map[string]interface{}{"max_volume": "50%"},
		"3":       map[string]interface{}{"curve": 100},
	})

	if len(errs) != 1 {
		t.Fatalf("expected exactly one error (slider 3's bad curve), got %v", errs)
	}

	check := func(idx int, curve, maxVolume float64) {
		t.Helper()
		got := m.get(idx)
		if !floatEquals(got.Curve, curve) || !floatEquals(got.MaxVolume, maxVolume) {
			t.Errorf("slider %d: got %s, want curve %v max %v", idx, got, curve, maxVolume)
		}
	}

	check(0, 2, 0.9)  // default
	check(1, 3, 0.9)  // preset name, inherits max
	check(2, 2, 0.5)  // inherits curve
	check(3, 2, 0.9)  // invalid curve falls back to the default
	check(99, 2, 0.9) // unconfigured slider
}

func floatEquals(a, b float64) bool {
	return math.Abs(a-b) < 0.0001
}
