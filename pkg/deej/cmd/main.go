package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/omriharel/deej/pkg/deej"
)

var (
	gitCommit  string
	versionTag string
	buildType  string

	verbose      bool
	openSettings bool
)

func init() {
	flag.BoolVar(&verbose, "verbose", false, "show verbose logs (useful for debugging serial)")
	flag.BoolVar(&verbose, "v", false, "shorthand for --verbose")
	flag.BoolVar(&openSettings, "settings", false, "open the settings window on startup")
	flag.Parse()
}

func main() {

	// deej looks for config.yaml (and writes its logs) relative to the working directory. when it's launched
	// on login or from a shortcut, that's often somewhere else entirely - so prefer the executable's own folder
	useExecutableDirectory()

	// first we need a logger
	logger, err := deej.NewLogger(buildType)
	if err != nil {
		panic(fmt.Sprintf("Failed to create logger: %v", err))
	}

	named := logger.Named("main")
	named.Debug("Created logger")

	named.Infow("Version info",
		"gitCommit", gitCommit,
		"versionTag", versionTag,
		"buildType", buildType)

	// provide a fair warning if the user's running in verbose mode
	if verbose {
		named.Debug("Verbose flag provided, all log messages will be shown")
	}

	// create the deej instance
	d, err := deej.NewDeej(logger, verbose)
	if err != nil {
		named.Fatalw("Failed to create deej object", "error", err)
	}

	// if injected by build process, set version info to show up in the tray
	if buildType != "" && (versionTag != "" || gitCommit != "") {
		identifier := gitCommit
		if versionTag != "" {
			identifier = versionTag
		}

		versionString := fmt.Sprintf("Version %s-%s", buildType, identifier)
		d.SetVersion(versionString)
	}

	if openSettings {
		d.OpenSettingsOnStart()
	}

	// onwards, to glory
	if err = d.Initialize(); err != nil {
		named.Fatalw("Failed to initialize deej", "error", err)
	}
}

// useExecutableDirectory switches to the folder containing the deej executable, if config.yaml is there
// and not in the current working directory. "go run" builds into a temp folder, so this won't affect development
func useExecutableDirectory() {
	if _, err := os.Stat("config.yaml"); err == nil {
		return
	}

	executable, err := os.Executable()
	if err != nil {
		return
	}

	executableDir := filepath.Dir(executable)
	if _, err := os.Stat(filepath.Join(executableDir, "config.yaml")); err == nil {
		os.Chdir(executableDir)
	}
}
