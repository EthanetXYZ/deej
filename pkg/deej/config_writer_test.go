package deej

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/spf13/viper"
)

func TestEditConfigFilePreservesCommentsAndRoundTrips(t *testing.T) {
	original := `# a comment at the top
slider_mapping:
  0: master
  3:
    - a.exe
    - b.exe

# keep this comment
invert_sliders: false # inline comment
com_port: COM4
`
	path := filepath.Join(t.TempDir(), "config.yaml")
	if err := os.WriteFile(path, []byte(original), 0644); err != nil {
		t.Fatal(err)
	}

	err := editConfigFile(path,
		configEdit{Path: []string{"invert_sliders"}, Value: true},
		configEdit{Path: []string{"slider_sensitivity", "default", "curve"}, Value: 2.5},
		configEdit{Path: []string{"slider_sensitivity", "1", "max_volume"}, Value: 60},
		configEdit{Path: []string{"com_port"}, Value: "COM7"},
	)
	if err != nil {
		t.Fatal(err)
	}

	edited, _ := os.ReadFile(path)
	for _, want := range []string{"# a comment at the top", "# keep this comment", "# inline comment", "invert_sliders: true", "com_port: COM7"} {
		if !strings.Contains(string(edited), want) {
			t.Errorf("edited config is missing %q:\n%s", want, edited)
		}
	}

	v := viper.New()
	v.SetConfigFile(path)
	if err := v.ReadInConfig(); err != nil {
		t.Fatalf("viper can't read edited config: %v\n%s", err, edited)
	}

	m, errs := sensitivityMapFromConfig(v.GetStringMap(configKeySliderSensitivity))
	if len(errs) > 0 {
		t.Fatal(errs)
	}

	if got := m.get(1); !floatEquals(got.Curve, 2.5) || !floatEquals(got.MaxVolume, 0.6) {
		t.Errorf("slider 1 sensitivity = %s", got)
	}

	if got := v.GetStringMapStringSlice(configKeySliderMapping)["3"]; len(got) != 2 {
		t.Errorf("slider mapping was damaged: %v", got)
	}

	// removing the only override for slider 1 should remove its (now empty) section
	if err := editConfigFile(path, configEdit{Path: []string{"slider_sensitivity", "1", "max_volume"}, Value: nil}); err != nil {
		t.Fatal(err)
	}

	edited, _ = os.ReadFile(path)
	if strings.Contains(string(edited), "1: {}") || strings.Contains(string(edited), "max_volume") {
		t.Errorf("override wasn't removed cleanly:\n%s", edited)
	}

	t.Logf("final config:\n%s", edited)
}

func TestEditConfigFileWritesLists(t *testing.T) {
	path := filepath.Join(t.TempDir(), "config.yaml")
	original := "# apps to leave alone\nunmapped_exclude: []\n\ncom_port: COM4\n"
	if err := os.WriteFile(path, []byte(original), 0644); err != nil {
		t.Fatal(err)
	}

	read := func() []string {
		t.Helper()
		v := viper.New()
		v.SetConfigFile(path)
		if err := v.ReadInConfig(); err != nil {
			t.Fatal(err)
		}
		return normalizeProcessNames(v.GetStringSlice(configKeyUnmappedExclude))
	}

	if err := editConfigFile(path, configEdit{Path: []string{configKeyUnmappedExclude}, Value: []string{"mpvnet.exe", "spotify.exe"}}); err != nil {
		t.Fatal(err)
	}

	if got := read(); strings.Join(got, ",") != "mpvnet.exe,spotify.exe" {
		t.Errorf("unmapped_exclude = %v", got)
	}

	// emptying the list keeps the key and its comment
	if err := editConfigFile(path, configEdit{Path: []string{configKeyUnmappedExclude}, Value: []string{}}); err != nil {
		t.Fatal(err)
	}

	edited, _ := os.ReadFile(path)
	if len(read()) != 0 || !strings.Contains(string(edited), "# apps to leave alone\nunmapped_exclude: []") {
		t.Errorf("emptied list wasn't written cleanly:\n%s", edited)
	}
}
