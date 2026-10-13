//! `slider_sensitivity`: how a slider's position becomes a volume (this fork's addition to deej).
//!
//! ```yaml
//! slider_sensitivity:
//!   default:          # every slider without an override
//!     curve: 2        # 0.2–6, or a preset name
//!     max_volume: 100 # 1–100 ("%" allowed)
//!   1:                # per slider; unset fields fall back to default
//!     curve: balanced
//! ```
//!
//! Parsing follows the fork's `sensitivityMapFromConfig`: an invalid field is ignored (and reported) without
//! discarding the valid fields next to it. One deliberate difference: NaN is rejected (Go's range check let it
//! through, because comparisons with NaN are false).

use std::collections::BTreeMap;

use serde::Serialize;

use super::Diagnostics;
use super::resolve::{Scalar, resolve_node, to_string};
use crate::yaml::{Node, NodeKind};

pub const MIN_CURVE: f64 = 0.2;
pub const MAX_CURVE: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CurvePreset {
    pub key: &'static str,
    pub name: &'static str,
    pub value: f64,
    pub description: &'static str,
}

pub const CURVE_PRESETS: &[CurvePreset] = &[
    CurvePreset {
        key: "linear",
        name: "Linear (original deej)",
        value: 1.0,
        description: "Volume follows the slider exactly - 10% slider = 10% volume",
    },
    CurvePreset {
        key: "gentle",
        name: "Gentle",
        value: 1.5,
        description: "Slightly finer control at low volumes - 10% slider = ~3% volume",
    },
    CurvePreset {
        key: "balanced",
        name: "Balanced",
        value: 2.0,
        description: "Recommended - 10% slider = 1% volume, 50% slider = 25% volume",
    },
    CurvePreset {
        key: "fine",
        name: "Fine",
        value: 2.5,
        description: "Very fine control at low volumes - 50% slider = ~18% volume",
    },
    CurvePreset {
        key: "very_fine",
        name: "Very fine",
        value: 3.0,
        description: "Maximum precision at low volumes - 50% slider = ~13% volume",
    },
];

/// One section of `slider_sensitivity`. `None` means "not set, use the default".
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct SensitivityOverride {
    pub curve: Option<f64>,
    /// 0.01–1.0 (the config file stores 1–100).
    pub max_volume: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SensitivityConfig {
    pub default: SensitivityOverride,
    pub sliders: BTreeMap<usize, SensitivityOverride>,
}

pub(crate) fn parse(node: &Node, d: &mut Diagnostics) -> SensitivityConfig {
    let mut result = SensitivityConfig::default();

    let Some(entries) = node.as_map() else {
        d.warn("slider_sensitivity", node.line, "should contain a `default:` section and/or slider sections; ignored");
        return result;
    };

    for (key, value) in entries {
        let name = to_string(key).unwrap_or_default().to_lowercase();
        let path = format!("slider_sensitivity.{name}");

        if !matches!(value.kind, NodeKind::Map(_)) {
            d.warn(path, value.line, "expected a section with curve and/or max_volume; ignored");
            continue;
        }

        let parsed = parse_override(value, &path, d);

        if name == "default" {
            result.default = parsed;
            continue;
        }

        match name.parse::<i64>() {
            Ok(index) if index >= 0 => {
                result.sliders.insert(index as usize, parsed);
            }
            _ => d.warn(path, key.line, "isn't a slider index or \"default\"; ignored"),
        }
    }

    result
}

fn parse_override(node: &Node, path: &str, d: &mut Diagnostics) -> SensitivityOverride {
    let mut result = SensitivityOverride::default();

    if let Some(value) = node.get("curve").filter(|n| !super::resolve::is_null(n)) {
        match parse_curve(value) {
            Some(curve) => result.curve = Some(curve),
            None => d.warn(
                format!("{path}.curve"),
                value.line,
                format!("must be a preset name or a number between {MIN_CURVE} and {MAX_CURVE}; ignored"),
            ),
        }
    }

    if let Some(value) = node.get("max_volume").filter(|n| !super::resolve::is_null(n)) {
        match parse_number(value) {
            Some(max) if (1.0..=100.0).contains(&max) => result.max_volume = Some(max / 100.0),
            _ => d.warn(format!("{path}.max_volume"), value.line, "must be a number between 1 and 100; ignored"),
        }
    }

    result
}

/// A preset name (case-insensitive, `very fine` also matches `very_fine`) or a number in range.
fn parse_curve(node: &Node) -> Option<f64> {
    if let Some(Scalar::Str(name)) = resolve_node(node) {
        let normalized = name.trim().to_lowercase();
        if let Some(preset) =
            CURVE_PRESETS.iter().find(|p| normalized == p.key || normalized == p.key.replace('_', " "))
        {
            return Some(preset.value);
        }
    }

    parse_number(node).filter(|c| (MIN_CURVE..=MAX_CURVE).contains(c))
}

/// The fork's `parseNumber`: numbers as-is, strings parsed as floats after trimming spaces and one `%` suffix.
fn parse_number(node: &Node) -> Option<f64> {
    let value = match resolve_node(node)? {
        Scalar::Int(i) => i as f64,
        Scalar::Uint(u) => u as f64,
        Scalar::Float(f) => f,
        Scalar::Str(s) => s.trim().strip_suffix('%').unwrap_or(s.trim()).parse::<f64>().ok()?,
        Scalar::Null | Scalar::Bool(_) => return None,
    };

    (!value.is_nan()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn presets_numbers_and_percent() {
        let loaded = config::parse(
            "slider_sensitivity:\n  default:\n    curve: Very Fine\n    max_volume: 80%\n  1:\n    curve: 1.6\n",
            None,
        )
        .unwrap();

        let s = loaded.config.slider_sensitivity;
        assert_eq!(s.default, SensitivityOverride { curve: Some(3.0), max_volume: Some(0.8) });
        assert_eq!(s.sliders[&1], SensitivityOverride { curve: Some(1.6), max_volume: None });
        assert!(loaded.diagnostics.is_empty());
    }

    #[test]
    fn invalid_fields_are_dropped_individually() {
        let loaded =
            config::parse("slider_sensitivity:\n  2:\n    curve: 9\n    max_volume: 50\n  x:\n    curve: 1\n", None)
                .unwrap();

        assert_eq!(
            loaded.config.slider_sensitivity.sliders[&2],
            SensitivityOverride { curve: None, max_volume: Some(0.5) }
        );
        assert_eq!(loaded.diagnostics.len(), 2);
    }
}

/// The curve and maximum that apply to one slider, after combining its section with the default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sensitivity {
    pub curve: f64,
    /// 0.01–1.0
    pub max_volume: f64,
}

impl Sensitivity {
    /// The original deej's behaviour: volume = position.
    pub const LINEAR: Sensitivity = Sensitivity { curve: 1.0, max_volume: 1.0 };

    /// Slider position (0–1) → volume (0–1): `max_volume × position^curve`, as the fork computes it
    /// (`float32(max(0, min(1, max × pow(clamp(p), curve))))`).
    pub fn apply(&self, position: f32) -> f32 {
        let p = f64::from(position).clamp(0.0, 1.0);
        let v = self.max_volume * p.powf(self.curve);
        v.clamp(0.0, 1.0) as f32
    }
}

impl SensitivityConfig {
    /// What sliders without their own section use.
    pub fn default_sensitivity(&self) -> Sensitivity {
        Sensitivity {
            curve: self.default.curve.unwrap_or(Sensitivity::LINEAR.curve),
            max_volume: self.default.max_volume.unwrap_or(Sensitivity::LINEAR.max_volume),
        }
    }

    /// What applies to `slider`: its own fields where set, the default otherwise.
    pub fn for_slider(&self, slider: usize) -> Sensitivity {
        let base = self.default_sensitivity();
        let own = self.sliders.get(&slider).copied().unwrap_or_default();
        Sensitivity { curve: own.curve.unwrap_or(base.curve), max_volume: own.max_volume.unwrap_or(base.max_volume) }
    }
}
