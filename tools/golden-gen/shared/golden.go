// Golden-value generator for deej Next's config loader.
//
// Reads every testdata/configs/*.yaml (skipping *.preferences.yaml) through viper exactly the way deej does,
// applies deej's own post-processing (copied verbatim from deej where noted), and prints one JSON object per
// fixture. Built twice: once against upstream deej's dependency set (viper 1.7.1, yaml.v2) and once against
// this fork's (viper 1.21.0, yaml.v3). See tools/golden-gen/README.md.
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"github.com/spf13/viper"
)

type result struct {
	File              string              `json:"file"`
	Error             string              `json:"error,omitempty"`
	SliderMapping     map[string][]string `json:"slider_mapping,omitempty"`
	InvertSliders     bool                `json:"invert_sliders"`
	COMPort           string              `json:"com_port"`
	BaudRate          int                 `json:"baud_rate"`
	NoiseReduction    string              `json:"noise_reduction"`
	UnmappedExclude   []string            `json:"unmapped_exclude,omitempty"`
	Sensitivity       *sensitivityResult  `json:"slider_sensitivity,omitempty"`
	NondeterministicMapping bool          `json:"nondeterministic_mapping,omitempty"`
}

type override struct {
	Curve     *float64 `json:"curve,omitempty"`
	MaxVolume *float64 `json:"max_volume,omitempty"`
}

type sensitivityResult struct {
	Default   override            `json:"default"`
	Overrides map[string]override `json:"overrides"`
	Errors    int                 `json:"error_count"`
}

func main() {
	dir := os.Args[1]
	files, _ := filepath.Glob(filepath.Join(dir, "*.yaml"))
	sort.Strings(files)

	var results []result
	for _, f := range files {
		if strings.HasSuffix(f, ".preferences.yaml") {
			continue
		}
		results = append(results, load(f))
	}

	out, _ := json.MarshalIndent(results, "", "  ")
	fmt.Println(string(out))
}

func newViper() *viper.Viper {
	v := viper.New()
	v.SetConfigType("yaml")
	return v
}

func load(path string) result {
	r := result{File: filepath.Base(path)}

	user := newViper()
	// defaults exactly as in deej's NewConfig
	user.SetDefault("slider_mapping", map[string][]string{})
	user.SetDefault("invert_sliders", false)
	user.SetDefault("com_port", "COM4")
	user.SetDefault("baud_rate", 9600)

	fh, err := os.Open(path)
	if err != nil {
		r.Error = err.Error()
		return r
	}
	err = user.ReadConfig(fh)
	fh.Close()
	if err != nil {
		r.Error = err.Error()
		return r
	}

	internal := newViper()
	prefsPath := strings.TrimSuffix(path, ".yaml") + ".preferences.yaml"
	if ph, err := os.Open(prefsPath); err == nil {
		internal.ReadConfig(ph)
		ph.Close()
	}

	userMapping := user.GetStringMapStringSlice("slider_mapping")
	r.SliderMapping, r.NondeterministicMapping = sliderMapFromConfigs(userMapping, internal.GetStringMapStringSlice("slider_mapping"))

	r.COMPort = user.GetString("com_port")
	r.BaudRate = user.GetInt("baud_rate")
	if r.BaudRate <= 0 {
		r.BaudRate = 9600
	}
	r.InvertSliders = user.GetBool("invert_sliders")
	r.NoiseReduction = user.GetString("noise_reduction")

	if forkVariant {
		r.UnmappedExclude = normalizeProcessNames(user.GetStringSlice("unmapped_exclude"))
		r.Sensitivity = sensitivityFromConfig(user.GetStringMap("slider_sensitivity"))
	}

	return r
}

// sliderMapFromConfigs: deej's logic (slider_map.go), with targets kept in order. Keys that collide after
// strconv.Atoi (errors ignored, as in deej) make the result depend on Go's random map order; that is reported
// rather than hidden.
func sliderMapFromConfigs(userMapping map[string][]string, internalMapping map[string][]string) (map[string][]string, bool) {
	result := map[int][]string{}
	seen := map[int]int{}

	for sliderIdxString, targets := range userMapping {
		sliderIdx, _ := strconv.Atoi(sliderIdxString)
		seen[sliderIdx]++

		filtered := []string{}
		for _, t := range targets {
			if t != "" {
				filtered = append(filtered, t)
			}
		}
		result[sliderIdx] = filtered
	}

	for sliderIdxString, targets := range internalMapping {
		sliderIdx, _ := strconv.Atoi(sliderIdxString)

		existing, ok := result[sliderIdx]
		if !ok {
			existing = []string{}
		}

		for _, t := range targets {
			if t == "" || contains(existing, t) {
				continue
			}
			existing = append(existing, t)
		}
		result[sliderIdx] = existing
	}

	nondeterministic := false
	for _, n := range seen {
		if n > 1 {
			nondeterministic = true
		}
	}

	out := map[string][]string{}
	for k, v := range result {
		out[strconv.Itoa(k)] = v
	}
	return out, nondeterministic
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// normalizeProcessNames: copied from the fork's config.go
func normalizeProcessNames(names []string) []string {
	result := []string{}
	seen := map[string]bool{}
	for _, name := range names {
		name = strings.ToLower(strings.TrimSpace(name))
		if name == "" || seen[name] {
			continue
		}
		seen[name] = true
		result = append(result, name)
	}
	return result
}

// sensitivity parsing: copied from the fork's sensitivity.go (sensitivityMapFromConfig and helpers), except that
// a NaN curve is rejected (deej Next does; Go's range check let NaN through)
var curvePresets = []struct {
	Key   string
	Value float64
}{{"linear", 1.0}, {"gentle", 1.5}, {"balanced", 2.0}, {"fine", 2.5}, {"very_fine", 3.0}}

func sensitivityFromConfig(raw map[string]interface{}) *sensitivityResult {
	res := &sensitivityResult{Overrides: map[string]override{}}
	for key, value := range raw {
		fields, ok := value.(map[string]interface{})
		if !ok {
			res.Errors++
			continue
		}
		ov, err := parseOverride(fields)
		if err != nil {
			res.Errors++
		}
		if strings.EqualFold(key, "default") {
			res.Default = ov
			continue
		}
		idx, err := strconv.Atoi(key)
		if err != nil || idx < 0 {
			res.Errors++
			continue
		}
		res.Overrides[strconv.Itoa(idx)] = ov
	}
	return res
}

func parseOverride(fields map[string]interface{}) (override, error) {
	var ov override
	var firstErr error
	if raw, ok := fields["curve"]; ok && raw != nil {
		if c, err := parseCurve(raw); err != nil {
			firstErr = err
		} else {
			ov.Curve = &c
		}
	}
	if raw, ok := fields["max_volume"]; ok && raw != nil {
		if m, err := parseNumber(raw); err != nil || m < 1 || m > 100 {
			if firstErr == nil {
				firstErr = fmt.Errorf("bad max_volume")
			}
		} else {
			m /= 100
			ov.MaxVolume = &m
		}
	}
	return ov, firstErr
}

func parseCurve(raw interface{}) (float64, error) {
	if name, ok := raw.(string); ok {
		n := strings.ToLower(strings.TrimSpace(name))
		for _, p := range curvePresets {
			if n == p.Key || n == strings.ReplaceAll(p.Key, "_", " ") {
				return p.Value, nil
			}
		}
	}
	c, err := parseNumber(raw)
	if err != nil || c < 0.2 || c > 6.0 || math.IsNaN(c) {
		return 0, fmt.Errorf("bad curve")
	}
	return c, nil
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
	return 0, fmt.Errorf("not a number")
}
