package deej

import (
	"fmt"
	"strings"
	"testing"
)

func TestShortTargetName(t *testing.T) {
	cases := []struct {
		targets   []string
		maxLength int
		want      string
	}{
		{[]string{"deej.unmapped"}, 12, "unmapped"},
		{[]string{"deej.current"}, 12, "current app"},
		{[]string{"firefox.exe", "brave.exe"}, 12, "firefox+1"},
		{[]string{"discord.exe"}, 12, "discord"},
		{[]string{"pathofexile_x64.exe"}, 12, "pathofexile~"},
		{[]string{"discord.exe"}, 5, "disc~"},
	}

	for _, c := range cases {
		if got := shortTargetName(c.targets, c.maxLength); got != c.want {
			t.Errorf("shortTargetName(%v, %d) = %q, want %q", c.targets, c.maxLength, got, c.want)
		}
	}
}

func entries(names ...string) []tooltipEntry {
	result := make([]tooltipEntry, len(names))
	for idx, name := range names {
		result[idx] = tooltipEntry{targets: strings.Split(name, ","), volume: "100%"}
	}

	return result
}

func checkTooltip(t *testing.T, tooltip string) {
	t.Helper()

	if len(tooltip) > maxTooltipLength {
		t.Errorf("tooltip is %d characters, over the %d limit:\n%s", len(tooltip), maxTooltipLength, tooltip)
	}

	for _, line := range strings.Split(tooltip, "\n")[1:] {
		if !strings.HasSuffix(line, "%") && !strings.HasSuffix(line, " more") {
			t.Errorf("entry was cut: %q", line)
		}
	}
}

func TestBuildTooltipFewSliders(t *testing.T) {

	// the reported setup, with everything maxed out: full names, nothing dropped
	tooltip := buildTooltip("deej", entries("deej.unmapped", "firefox.exe,brave.exe", "discord.exe", "mpvnet.exe"))
	checkTooltip(t, tooltip)

	want := "deej\nunmapped 100%\nfirefox+1 100%\ndiscord 100%\nmpvnet 100%"
	if tooltip != want {
		t.Errorf("tooltip = %q, want %q", tooltip, want)
	}
}

func TestBuildTooltipShortensNamesToFit(t *testing.T) {
	tooltip := buildTooltip("deej", entries("spotify.exe", "discord.exe", "firefox.exe", "chrome.exe", "steam.exe"))
	checkTooltip(t, tooltip)

	if strings.Count(tooltip, "\n") != 5 || strings.Contains(tooltip, "more") {
		t.Errorf("expected all 5 sliders with shorter names:\n%s", tooltip)
	}
}

func TestBuildTooltipManySliders(t *testing.T) {
	var names []string
	for idx := 0; idx < 16; idx++ {
		names = append(names, fmt.Sprintf("app%02d.exe", idx))
	}

	tooltip := buildTooltip("deej", entries(names...))
	checkTooltip(t, tooltip)

	lines := strings.Split(tooltip, "\n")
	shown := len(lines) - 2 // minus the header and the "+N more" line
	if want := fmt.Sprintf("+%d more", 16-shown); lines[len(lines)-1] != want {
		t.Errorf("last line = %q, want %q:\n%s", lines[len(lines)-1], want, tooltip)
	}
}
