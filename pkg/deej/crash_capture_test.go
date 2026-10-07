package deej

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const crashCaptureChildEnv = "DEEJ_TEST_CRASH_CAPTURE_PATH"

func TestCaptureCrashOutput(t *testing.T) {

	// child mode: capture stderr, then crash the way a real fault would (an unrecovered panic in a goroutine)
	if path := os.Getenv(crashCaptureChildEnv); path != "" {
		captureCrashOutput(path)

		done := make(chan bool)
		go func() {
			panic("deliberate test crash")
		}()
		<-done
	}

	dir := t.TempDir()
	path := filepath.Join(dir, "deej-crash.log")

	// a leftover crash from a "previous run" should be kept aside
	if err := os.WriteFile(path, []byte("old crash"), 0644); err != nil {
		t.Fatal(err)
	}

	cmd := exec.Command(os.Args[0], "-test.run=^TestCaptureCrashOutput$")
	cmd.Env = append(os.Environ(), crashCaptureChildEnv+"="+path)
	if err := cmd.Run(); err == nil {
		t.Fatal("child process was expected to crash")
	}

	crash, _ := os.ReadFile(path)
	if !strings.Contains(string(crash), "deliberate test crash") || !strings.Contains(string(crash), "goroutine") {
		t.Errorf("crash log doesn't contain the crash:\n%s", crash)
	}

	previous, _ := os.ReadFile(filepath.Join(dir, "deej-crash-previous.log"))
	if string(previous) != "old crash" {
		t.Errorf("previous crash log = %q, want %q", previous, "old crash")
	}
}
