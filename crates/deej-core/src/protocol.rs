//! deej's serial protocol: what the board sends and how it becomes slider positions.
//!
//! The board sends one line per reading: raw ADC values (0–1023) separated by `|`, ending in CRLF
//! (`Serial.println`), e.g. `0|240|1023|0|483`. The number of values is the number of sliders. deej never writes
//! to the board. See docs/ORIGINAL_DEEJ_ANALYSIS.md §2.
//!
//! The numeric steps are ported literally from the Go deej (including where it converts between f32 and f64), and
//! `tests/golden_protocol.rs` checks them bit-for-bit against values produced by the Go code.

use crate::config::NoiseReduction;

/// Highest value a 10-bit Arduino ADC reports.
pub const MAX_RAW: u16 = 1023;

/// A longer "line" means the board isn't sending deej data; it's dropped (the Go deej buffered it indefinitely).
pub const MAX_LINE_LENGTH: usize = 4096;

/// Position of a slider whose value hasn't been seen yet. Any real reading counts as a change from it.
pub const UNKNOWN_POSITION: f32 = -1.0;

/// Parses one line (without its `\n`; a trailing `\r` is accepted). Returns `None` for anything that isn't a
/// valid deej line: the Go regex `^\d{1,4}(\|\d{1,4})*\r?\n$` plus "every value ≤ 1023".
///
/// A value above 1023 rejects the whole line. The first line after the port opens is often half a line
/// (e.g. `4558|925|41`), which is what this check was for in the original.
pub fn parse_line(line: &str) -> Option<Vec<u16>> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return None;
    }

    line.split('|')
        .map(|field| {
            let valid = (1..=4).contains(&field.len()) && field.bytes().all(|b| b.is_ascii_digit());
            valid.then(|| field.parse::<u16>().ok()).flatten().filter(|&v| v <= MAX_RAW)
        })
        .collect()
}

/// Raw ADC value → position 0.00–1.00, floored to two decimals, optionally inverted.
/// Exactly the Go deej's arithmetic: `float32(raw) / 1023.0`, then `float32(math.Floor(float64(v)*100) / 100)`.
pub fn normalize(raw: u16, invert: bool) -> f32 {
    let dirty = f32::from(raw) / 1023.0_f32;
    let position = ((f64::from(dirty) * 100.0).floor() / 100.0) as f32;
    if invert { 1.0 - position } else { position }
}

/// The Go deej's `SignificantlyDifferent`: is `new` far enough from `old` to count as a move?
/// Moves smaller than the noise threshold are ignored, except that reaching exactly 0 or 1 always counts.
pub fn significantly_different(old: f32, new: f32, noise: NoiseReduction) -> bool {
    let threshold = match noise {
        NoiseReduction::Low => 0.015,
        NoiseReduction::Default => 0.025,
        NoiseReduction::High => 0.035,
    };

    if f64::from(old - new).abs() >= threshold {
        return true;
    }

    let almost = |a: f32, b: f32| f64::from(a - b).abs() < 0.000001;
    (almost(new, 1.0) && old != 1.0) || (almost(new, 0.0) && old != 0.0)
}

/// A slider moved to a new position (0.0–1.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderMove {
    pub slider: usize,
    pub position: f32,
}

/// Tracks the last position of every slider on one board and turns readings into moves.
#[derive(Debug, Clone)]
pub struct SliderState {
    positions: Vec<f32>,
    /// Sliders whose next reading must produce a move even if unchanged (e.g. after a sensitivity change).
    resend: Vec<bool>,
    resend_all: bool,
    pub invert: bool,
    pub noise: NoiseReduction,
}

impl SliderState {
    pub fn new(invert: bool, noise: NoiseReduction) -> SliderState {
        SliderState { positions: Vec::new(), resend: Vec::new(), resend_all: false, invert, noise }
    }

    /// Last known position of each slider ([`UNKNOWN_POSITION`] until it's been read).
    pub fn positions(&self) -> &[f32] {
        &self.positions
    }

    /// Forgets everything, e.g. after reconnecting: the next reading reports every slider.
    pub fn reset(&mut self) {
        self.positions.clear();
        self.resend.clear();
        self.resend_all = false;
    }

    /// The next reading reports every slider (after a config reload, so new mappings take effect immediately).
    pub fn resend_all(&mut self) {
        self.resend_all = true;
    }

    /// The next reading reports this slider (e.g. its sensitivity changed).
    pub fn resend(&mut self, slider: usize) {
        if self.resend.len() <= slider {
            self.resend.resize(slider + 1, false);
        }
        self.resend[slider] = true;
    }

    /// Feeds one parsed line. Returns the sliders that moved, in slider order.
    pub fn update(&mut self, values: &[u16]) -> Vec<SliderMove> {
        // a different number of values: the board changed (or the first line); report everything
        if values.len() != self.positions.len() {
            self.positions = vec![UNKNOWN_POSITION; values.len()];
        }

        if std::mem::take(&mut self.resend_all) {
            self.positions.fill(UNKNOWN_POSITION);
        }

        for (slider, flag) in self.resend.iter_mut().enumerate() {
            if std::mem::take(flag) {
                if let Some(position) = self.positions.get_mut(slider) {
                    *position = UNKNOWN_POSITION;
                }
            }
        }

        let mut moves = Vec::new();
        for (slider, &raw) in values.iter().enumerate() {
            let position = normalize(raw, self.invert);
            if significantly_different(self.positions[slider], position, self.noise) {
                self.positions[slider] = position;
                moves.push(SliderMove { slider, position });
            }
        }

        moves
    }
}

/// Holds back a reading with a different number of sliders until the next reading confirms that number.
///
/// Right after a port opens, and sometimes after a reconnect, the stream contains pieces of two lines run together
/// (seen on a real Uno: `99|65|0|88` and `0` between proper 3-value lines). Those are valid deej lines with the
/// wrong slider count, and the Go deej applied them, briefly sending sliders to wrong volumes. A real change of
/// slider count repeats on every following line, so waiting for one confirmation costs about one line (~15 ms).
/// See COMPATIBILITY_MATRIX S5b.
#[derive(Debug, Clone, Default)]
pub struct CountGuard {
    current: Option<usize>,
    candidate: Option<usize>,
}

impl CountGuard {
    /// Whether a reading with `count` values should be used.
    pub fn accept(&mut self, count: usize) -> bool {
        if self.current == Some(count) {
            self.candidate = None;
            return true;
        }

        if self.candidate == Some(count) {
            self.current = Some(count);
            self.candidate = None;
            return true;
        }

        self.candidate = Some(count);
        false
    }

    /// Forgets the slider count, e.g. after reconnecting.
    pub fn reset(&mut self) {
        *self = CountGuard::default();
    }
}

/// What came out of a chunk of serial data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A complete line (without `\n`), not yet validated.
    Text(String),
    /// A line that wasn't valid UTF-8, or was longer than [`MAX_LINE_LENGTH`]. Counted for diagnostics.
    Garbage,
}

/// Splits a byte stream into lines. Bytes arrive in arbitrary chunks, so partial lines are kept until their `\n`.
#[derive(Debug, Default)]
pub struct LineSplitter {
    buffer: Vec<u8>,
    /// Inside an over-long line: drop everything until the next `\n`.
    discarding: bool,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();

        for &byte in bytes {
            if byte == b'\n' {
                if std::mem::take(&mut self.discarding) {
                    self.buffer.clear();
                    continue;
                }

                let raw = std::mem::take(&mut self.buffer);
                lines.push(match String::from_utf8(raw) {
                    Ok(text) => Line::Text(text),
                    Err(_) => Line::Garbage,
                });
                continue;
            }

            if self.discarding {
                continue;
            }

            if self.buffer.len() >= MAX_LINE_LENGTH {
                self.buffer.clear();
                self.discarding = true;
                lines.push(Line::Garbage);
                continue;
            }

            self.buffer.push(byte);
        }

        lines
    }

    /// Drops a partial line, e.g. when the port is reopened.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.discarding = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_lines_only() {
        assert_eq!(parse_line("0|240|1023|0|483\r"), Some(vec![0, 240, 1023, 0, 483]));
        assert_eq!(parse_line("512"), Some(vec![512]));
        assert_eq!(parse_line("0007|1"), Some(vec![7, 1]));

        for bad in [
            "", "\r", "1||2", "|1", "1|", "-1|2", " 1|2", "1 |2", "12345|1", "1024|0", "0|1024", "a|1", "1|2\r\r",
            "１|2",
        ] {
            assert_eq!(parse_line(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn normalization_floors_to_two_decimals() {
        assert_eq!(normalize(0, false), 0.0);
        assert_eq!(normalize(10, false), 0.0);
        assert_eq!(normalize(11, false), 0.01);
        assert_eq!(normalize(512, false), 0.5);
        assert_eq!(normalize(1022, false), 0.99);
        assert_eq!(normalize(1023, false), 1.0);
        assert_eq!(normalize(1023, true), 0.0);
    }

    #[test]
    fn rejected_lines_leave_no_trace() {
        let mut state = SliderState::new(false, NoiseReduction::Default);
        state.update(&[100, 100, 100]);

        // the Go fork updated sliders 0 and 1 here before rejecting the line, losing these moves (S3)
        assert_eq!(parse_line("600|600|1024"), None);

        let moves = state.update(&parse_line("600|600|100").unwrap());
        assert_eq!(moves.iter().map(|m| m.slider).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn resend_reports_unchanged_sliders_once() {
        let mut state = SliderState::new(false, NoiseReduction::Default);
        state.update(&[300, 400, 500]);
        assert!(state.update(&[300, 400, 500]).is_empty());

        state.resend(1);
        state.resend(7); // beyond the board's sliders: ignored
        assert_eq!(state.update(&[300, 400, 500]).iter().map(|m| m.slider).collect::<Vec<_>>(), vec![1]);
        assert!(state.update(&[300, 400, 500]).is_empty());

        state.resend_all();
        assert_eq!(state.update(&[300, 400, 500]).len(), 3);
    }

    #[test]
    fn supports_more_than_64_sliders() {
        let mut state = SliderState::new(false, NoiseReduction::Default);
        let values = vec![512; 80];
        assert_eq!(state.update(&values).len(), 80);
        state.resend(70);
        assert_eq!(state.update(&values), vec![SliderMove { slider: 70, position: 0.5 }]);
    }

    #[test]
    fn count_changes_need_confirmation() {
        let mut guard = CountGuard::default();
        assert!(!guard.accept(3), "the first line after connecting is never trusted on its own");
        assert!(guard.accept(3));
        assert!(guard.accept(3));

        // a garbled line with another count, then normal lines: never applied
        assert!(!guard.accept(4));
        assert!(guard.accept(3));
        assert!(!guard.accept(1));
        assert!(guard.accept(3));

        // a real change repeats
        assert!(!guard.accept(5));
        assert!(guard.accept(5));
        assert!(guard.accept(5));

        guard.reset();
        assert!(!guard.accept(5));
    }

    #[test]
    fn splits_lines_across_chunks() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"12|3"), vec![]);
        assert_eq!(splitter.push(b"4\r\n5|6\r\n7"), vec![Line::Text("12|34\r".into()), Line::Text("5|6\r".into())]);
        assert_eq!(splitter.push(b"\n"), vec![Line::Text("7".into())]);
        assert_eq!(splitter.push(&[0xff, 0xfe, b'\n']), vec![Line::Garbage]);
    }

    #[test]
    fn over_long_lines_are_dropped_up_to_the_next_newline() {
        let mut splitter = LineSplitter::default();
        let long = vec![b'1'; MAX_LINE_LENGTH + 10];

        assert_eq!(splitter.push(&long), vec![Line::Garbage]);
        assert_eq!(splitter.push(b"999\n1|2\n"), vec![Line::Text("1|2".into())]);
    }
}
