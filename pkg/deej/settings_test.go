package deej

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestQueueConfigEditsMergesByPath(t *testing.T) {
	d := &Deej{}
	b := d.settingsBackend()

	// a long delay so nothing gets flushed during the test
	const delay = time.Hour

	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "1", "curve"}, Value: 1.5})
	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "1", "curve"}, Value: 2.5})
	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "1", "max_volume"}, Value: 80})
	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "2", "curve"}, Value: 3.0})

	// resetting slider 1 supersedes both of its pending edits, and comes after slider 2's
	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "1"}, Value: nil})

	// a later edit for slider 1 must be applied after the reset
	d.queueConfigEdits(delay, configEdit{Path: []string{"slider_sensitivity", "1", "curve"}, Value: 2.0})

	b.lock.Lock()
	defer b.lock.Unlock()
	b.saveTimer.Stop()

	var got []string
	for _, edit := range b.pendingEdits {
		got = append(got, strings.Join(edit.Path, ".")+"="+formatEditValue(edit.Value))
	}

	want := []string{
		"slider_sensitivity.2.curve=3",
		"slider_sensitivity.1=nil",
		"slider_sensitivity.1.curve=2",
	}

	if strings.Join(got, " ") != strings.Join(want, " ") {
		t.Errorf("pending edits = %v, want %v", got, want)
	}
}

func formatEditValue(v interface{}) string {
	if v == nil {
		return "nil"
	}

	return fmt.Sprint(v)
}
