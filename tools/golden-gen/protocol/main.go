// Golden-value generator for deej Next's serial protocol handling.
//
// The functions below are copied verbatim from the Go deej (util/util.go is identical in upstream and this fork;
// handleLine is the fork's, minus logging and locking). Output: testdata/golden/protocol.json
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

// ---- copied from util/util.go ----

func NormalizeScalar(v float32) float32 {
	return float32(math.Floor(float64(v)*100) / 100.0)
}

func SignificantlyDifferent(old float32, new float32, noiseReductionLevel string) bool {
	const (
		noiseReductionHigh = "high"
		noiseReductionLow  = "low"
	)
	var significantDifferenceThreshold float64
	switch noiseReductionLevel {
	case noiseReductionHigh:
		significantDifferenceThreshold = 0.035
	case noiseReductionLow:
		significantDifferenceThreshold = 0.015
	default:
		significantDifferenceThreshold = 0.025
	}
	if math.Abs(float64(old-new)) >= significantDifferenceThreshold {
		return true
	}
	if (almostEquals(new, 1.0) && old != 1.0) || (almostEquals(new, 0.0) && old != 0.0) {
		return true
	}
	return false
}

func almostEquals(a float32, b float32) bool {
	return math.Abs(float64(a-b)) < 0.000001
}

// ---- the fork's sensitivity.go apply ----

func applySensitivity(position float32, curve float64, maxVolume float64) float32 {
	p := math.Max(0, math.Min(1, float64(position)))
	v := maxVolume * math.Pow(p, curve)
	return float32(math.Max(0, math.Min(1, v)))
}

// ---- the fork's serial.go handleLine, without logging/locking ----

var expectedLinePattern = regexp.MustCompile(`^\d{1,4}(\|\d{1,4})*\r?\n$`)

type event struct {
	Slider   int    `json:"slider"`
	Position uint32 `json:"position_bits"`
}

type sio struct {
	invert              bool
	noise               string
	lastKnownNumSliders int
	current             []float32
	resendAll           bool
}

func (s *sio) handleLine(line string) []event {
	if !expectedLinePattern.MatchString(line) {
		return nil
	}
	line = strings.TrimRight(line, "\r\n")
	split := strings.Split(line, "|")
	n := len(split)
	if n != s.lastKnownNumSliders {
		s.lastKnownNumSliders = n
		s.current = make([]float32, n)
		for i := range s.current {
			s.current[i] = -1.0
		}
	}
	if s.resendAll {
		s.resendAll = false
		for i := range s.current {
			s.current[i] = -1.0
		}
	}
	events := []event{}
	for idx, str := range split {
		number, _ := strconv.Atoi(str)
		if number > 1023 {
			return nil // note: positions already updated for earlier sliders stay updated (fork bug)
		}
		dirty := float32(number) / 1023.0
		v := NormalizeScalar(dirty)
		if s.invert {
			v = 1 - v
		}
		if SignificantlyDifferent(s.current[idx], v, s.noise) {
			s.current[idx] = v
			events = append(events, event{idx, math.Float32bits(v)})
		}
	}
	return events
}

// ---- output ----

type step struct {
	Line   string  `json:"line,omitempty"`
	Resend bool    `json:"resend_all,omitempty"`
	Events []event `json:"events"`
}

type sequence struct {
	Name   string `json:"name"`
	Invert bool   `json:"invert"`
	Noise  string `json:"noise"`
	Steps  []step `json:"steps"`
}

func run(name string, invert bool, noise string, lines ...string) sequence {
	s := &sio{invert: invert, noise: noise}
	seq := sequence{Name: name, Invert: invert, Noise: noise}
	for _, l := range lines {
		if l == "RESEND" {
			s.resendAll = true
			seq.Steps = append(seq.Steps, step{Resend: true, Events: []event{}})
			continue
		}
		seq.Steps = append(seq.Steps, step{Line: l, Events: s.handleLine(l)})
	}
	return seq
}

func main() {
	out := map[string]interface{}{}

	// normalization of every raw value, plain and inverted (as float32 bit patterns)
	normalized := [][2]uint32{}
	positions := map[uint32]bool{math.Float32bits(-1): true}
	for raw := 0; raw <= 1023; raw++ {
		v := NormalizeScalar(float32(raw) / 1023.0)
		inv := 1 - v
		normalized = append(normalized, [2]uint32{math.Float32bits(v), math.Float32bits(inv)})
		positions[math.Float32bits(v)] = true
		positions[math.Float32bits(inv)] = true
	}
	out["normalize"] = normalized

	// every noise-filter decision between every position that can occur (incl. the -1 "unknown" sentinel)
	all := []uint32{}
	for p := range positions {
		all = append(all, p)
	}
	sort.Slice(all, func(i, j int) bool { return all[i] < all[j] })
	out["positions_bits"] = all

	noise := map[string]string{}
	for _, level := range []string{"low", "default", "high", "HIGH"} {
		var b strings.Builder
		for _, o := range all {
			for _, n := range all {
				if SignificantlyDifferent(math.Float32frombits(o), math.Float32frombits(n), level) {
					b.WriteByte('1')
				} else {
					b.WriteByte('0')
				}
			}
		}
		noise[level] = b.String()
	}
	out["significantly_different"] = noise

	// sensitivity curve for every reachable position and a spread of curves and maximums
	sens := [][4]float64{}
	for _, p := range all {
		for _, c := range []float64{1, 1.5, 2, 2.5, 3, 0.2, 6, 1.6, 0.75} {
			for _, m := range []float64{1, 0.5, 0.8, 0.01} {
				sens = append(sens, [4]float64{float64(p), c, m, float64(math.Float32bits(applySensitivity(math.Float32frombits(p), c, m)))})
			}
		}
	}
	out["sensitivity"] = sens

	out["sequences"] = []sequence{
		run("basic moves and jitter", false, "default",
			"0|512|1023\r\n", "1|513|1023\r\n", "0|540|1023\r\n", "0|541|1000\r\n", "0|543|1023\r\n", "30|543|1023\r\n", "0|543|1023\r\n"),
		run("slider count change re-sends all", false, "default",
			"100|200|300\r\n", "100|200|300\r\n", "100|200\r\n", "100|200|300|400\r\n"),
		run("garbage and malformed lines are ignored", false, "default",
			"512|512\r\n", "hello\r\n", "\r\n", "512||512\r\n", "-1|512\r\n", "512 |512\r\n", "12345|1\r\n", "600|600\r\n", "700|700\r\r\n"),
		run("LF-only lines", false, "default",
			"100|900\n", "300|700\n"),
		run("value over 1023 in the first field", false, "default",
			"4558|925|41\r\n", "500|925|41\r\n"),
		run("value over 1023 in a later field (fork bug case)", false, "default",
			"100|100|100\r\n", "600|600|1024\r\n", "600|600|100\r\n"),
		run("inverted", true, "default",
			"0|1023|512\r\n", "11|1013|511\r\n", "40|980|480\r\n"),
		run("noise low", false, "low",
			"500\r\n", "520\r\n", "540\r\n", "541\r\n", "1023\r\n", "1013\r\n", "0\r\n", "10\r\n"),
		run("noise high", false, "high",
			"500\r\n", "530\r\n", "540\r\n", "541\r\n", "1012\r\n", "1023\r\n"),
		run("unknown noise level means default", false, "HIGH",
			"500\r\n", "530\r\n"),
		run("resend after config reload", false, "default",
			"300|400\r\n", "300|400\r\n", "RESEND", "300|400\r\n", "301|400\r\n"),
		run("snap to the ends", false, "default",
			"1000\r\n", "1023\r\n", "1022\r\n", "15\r\n", "0\r\n"),
	}

	b, _ := json.Marshal(out)
	fmt.Println(string(b))
}
