//! Checks the serial protocol port bit-for-bit against the Go deej (`tools/golden-gen/protocol`).

use std::path::Path;

use deej_core::config::NoiseReduction;
use deej_core::protocol::{Line, LineSplitter, SliderState, normalize, parse_line, significantly_different};
use serde_json::Value;

fn golden() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/golden/protocol.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn normalization_matches_go_for_every_raw_value() {
    let g = golden();
    let table = g["normalize"].as_array().unwrap();
    assert_eq!(table.len(), 1024);

    for (raw, pair) in table.iter().enumerate() {
        let expected = pair.as_array().unwrap();
        let plain = normalize(raw as u16, false).to_bits();
        let inverted = normalize(raw as u16, true).to_bits();
        assert_eq!(u64::from(plain), expected[0].as_u64().unwrap(), "raw {raw}");
        assert_eq!(u64::from(inverted), expected[1].as_u64().unwrap(), "raw {raw} inverted");
    }
}

#[test]
fn noise_filter_matches_go_for_every_pair_of_positions() {
    let g = golden();
    let positions: Vec<f32> =
        g["positions_bits"].as_array().unwrap().iter().map(|b| f32::from_bits(b.as_u64().unwrap() as u32)).collect();

    for (level, decisions) in g["significantly_different"].as_object().unwrap() {
        let noise = NoiseReduction::from_config(level);
        let decisions = decisions.as_str().unwrap().as_bytes();
        assert_eq!(decisions.len(), positions.len() * positions.len());

        let mut index = 0;
        for &old in &positions {
            for &new in &positions {
                let expected = decisions[index] == b'1';
                assert_eq!(significantly_different(old, new, noise), expected, "{level}: {old} -> {new}");
                index += 1;
            }
        }
    }
}

/// Where deej Next intentionally differs: the fork updated earlier sliders before rejecting a line with a value
/// over 1023 in a later field, so the next valid line didn't report them (COMPATIBILITY_MATRIX S3).
fn expected_override(sequence: &str, step: usize) -> Option<Vec<usize>> {
    match (sequence, step) {
        ("value over 1023 in a later field (fork bug case)", 2) => Some(vec![0, 1]),
        _ => None,
    }
}

#[test]
fn line_sequences_produce_the_same_moves_as_go() {
    let g = golden();

    for sequence in g["sequences"].as_array().unwrap() {
        let name = sequence["name"].as_str().unwrap();
        let mut state = SliderState::new(
            sequence["invert"].as_bool().unwrap(),
            NoiseReduction::from_config(sequence["noise"].as_str().unwrap()),
        );
        let mut splitter = LineSplitter::default();

        for (index, step) in sequence["steps"].as_array().unwrap().iter().enumerate() {
            if step.get("resend_all").is_some() {
                state.resend_all();
                continue;
            }

            // feed the line through the splitter, as it arrives from the port
            let line = step["line"].as_str().unwrap();
            let lines = splitter.push(line.as_bytes());
            assert_eq!(lines.len(), 1, "{name} step {index}");

            let moves = match &lines[0] {
                Line::Text(text) => parse_line(text).map(|values| state.update(&values)).unwrap_or_default(),
                Line::Garbage => Vec::new(),
            };

            let expected: Vec<(usize, u32)> = step["events"]
                .as_array()
                .map(|events| {
                    events
                        .iter()
                        .map(|e| (e["slider"].as_u64().unwrap() as usize, e["position_bits"].as_u64().unwrap() as u32))
                        .collect()
                })
                .unwrap_or_default();
            let actual: Vec<(usize, u32)> = moves.iter().map(|m| (m.slider, m.position.to_bits())).collect();

            match expected_override(name, index) {
                Some(sliders) => {
                    assert_eq!(actual.iter().map(|m| m.0).collect::<Vec<_>>(), sliders, "{name} step {index}")
                }
                None => assert_eq!(actual, expected, "{name} step {index} ({line:?})"),
            }
        }
    }
}

#[test]
fn sensitivity_curve_matches_go() {
    use deej_core::config::Sensitivity;

    let g = golden();
    let cases = g["sensitivity"].as_array().unwrap();
    let mut mismatches = Vec::new();

    for case in cases {
        let c = case.as_array().unwrap();
        let position = f32::from_bits(c[0].as_f64().unwrap() as u32);
        let sensitivity = Sensitivity { curve: c[1].as_f64().unwrap(), max_volume: c[2].as_f64().unwrap() };
        let expected = c[3].as_f64().unwrap() as u32;

        let actual = sensitivity.apply(position).to_bits();
        if actual != expected {
            mismatches.push((
                position,
                sensitivity.curve,
                sensitivity.max_volume,
                f32::from_bits(actual),
                f32::from_bits(expected),
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} of {} differ, e.g. {:?}",
        mismatches.len(),
        cases.len(),
        &mismatches[..mismatches.len().min(5)]
    );
}
