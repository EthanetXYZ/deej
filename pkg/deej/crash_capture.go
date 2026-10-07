package deej

import (
	"os"
	"path/filepath"
	"runtime/debug"
	"strings"
)

// captureCrashOutput sends this process's stderr to a file, so a fatal crash (which the go runtime reports on
// stderr) leaves a trace even without a console window. a non-empty file from the previous run means it
// crashed, so that one is kept as "<name>-previous" before starting a new one
func captureCrashOutput(path string) {
	if info, err := os.Stat(path); err == nil && info.Size() > 0 {
		ext := filepath.Ext(path)
		previous := strings.TrimSuffix(path, ext) + "-previous" + ext

		os.Remove(previous)
		os.Rename(path, previous)
	}

	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0644)
	if err != nil {
		return
	}

	if err := redirectStderr(f); err != nil {
		f.Close()
		return
	}

	os.Stderr = f

	// include every goroutine in crash reports, since crashes are often caused by two of them interacting
	debug.SetTraceback("all")
}
