package util

import "errors"

// AutostartSupported returns true if deej can register itself to run on login on this platform
func AutostartSupported() bool {
	return false
}

// AutostartEnabled returns true if deej is registered to run when the current user logs in
func AutostartEnabled() bool {
	return false
}

// SetAutostart registers (or unregisters) the running deej executable to start when the current user logs in
func SetAutostart(enabled bool) error {
	return errors.New("not implemented")
}

// RefreshAutostart points an existing autostart entry at the running executable, in case deej was moved
func RefreshAutostart() error {
	return nil
}
