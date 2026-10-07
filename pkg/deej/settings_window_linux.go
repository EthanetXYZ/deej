package deej

// openSettingsWindow isn't available on Linux yet, so fall back to opening the config file
func (d *Deej) openSettingsWindow() {
	if err := d.runAction("editConfig"); err != nil {
		d.logger.Warnw("Failed to open config file for editing", "error", err)
	}
}
