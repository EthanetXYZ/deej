package util

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"golang.org/x/sys/windows/registry"
)

const (
	autostartRegistryKey   = `Software\Microsoft\Windows\CurrentVersion\Run`
	autostartRegistryValue = "deej"
)

// AutostartSupported returns true if deej can register itself to run on login on this platform
func AutostartSupported() bool {
	return true
}

// AutostartEnabled returns true if deej is set to run when the current user logs in,
// either through its registry entry or through a shortcut the user placed in their Startup folder
func AutostartEnabled() bool {
	return autostartRegistryEnabled() || len(startupFolderShortcuts()) > 0
}

func autostartRegistryEnabled() bool {
	key, err := registry.OpenKey(registry.CURRENT_USER, autostartRegistryKey, registry.QUERY_VALUE)
	if err != nil {
		return false
	}
	defer key.Close()

	_, _, err = key.GetStringValue(autostartRegistryValue)

	return err == nil
}

// startupFolderShortcuts finds shortcuts to deej in the user's Startup folder (the classic way of setting up autostart)
func startupFolderShortcuts() []string {
	appData := os.Getenv("APPDATA")
	if appData == "" {
		return nil
	}

	startupDir := filepath.Join(appData, "Microsoft", "Windows", "Start Menu", "Programs", "Startup")

	entries, err := os.ReadDir(startupDir)
	if err != nil {
		return nil
	}

	var result []string
	for _, entry := range entries {
		name := strings.ToLower(entry.Name())
		if strings.HasPrefix(name, "deej") && strings.HasSuffix(name, ".lnk") {
			result = append(result, filepath.Join(startupDir, entry.Name()))
		}
	}

	return result
}

// SetAutostart registers (or unregisters) the running deej executable to start when the current user logs in
func SetAutostart(enabled bool) error {
	key, _, err := registry.CreateKey(registry.CURRENT_USER, autostartRegistryKey, registry.QUERY_VALUE|registry.SET_VALUE)
	if err != nil {
		return fmt.Errorf("open autostart registry key: %w", err)
	}
	defer key.Close()

	if !enabled {
		if err := key.DeleteValue(autostartRegistryValue); err != nil && !errors.Is(err, registry.ErrNotExist) {
			return fmt.Errorf("delete autostart registry value: %w", err)
		}

		// also remove Startup folder shortcuts, otherwise deej would still launch on login
		for _, shortcut := range startupFolderShortcuts() {
			if err := os.Remove(shortcut); err != nil {
				return fmt.Errorf("remove startup shortcut %s: %w", shortcut, err)
			}
		}

		return nil
	}

	// a Startup folder shortcut already takes care of it - don't launch deej twice
	if len(startupFolderShortcuts()) > 0 {
		return nil
	}

	executable, err := os.Executable()
	if err != nil {
		return fmt.Errorf("get executable path: %w", err)
	}

	if resolved, err := filepath.EvalSymlinks(executable); err == nil {
		executable = resolved
	}

	if err := key.SetStringValue(autostartRegistryValue, fmt.Sprintf(`"%s"`, executable)); err != nil {
		return fmt.Errorf("set autostart registry value: %w", err)
	}

	return nil
}

// RefreshAutostart points an existing registry autostart entry at the running executable, in case deej was moved
func RefreshAutostart() error {
	if !autostartRegistryEnabled() {
		return nil
	}

	return SetAutostart(true)
}
