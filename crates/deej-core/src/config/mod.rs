//! deej's `config.yaml`: the settings model and the loader.
//!
//! The loader accepts every file the original Go deej (upstream v0.9.10 and this fork) accepts, and reads it
//! the same way. The exceptions are deliberate and listed in `docs/COMPATIBILITY_MATRIX.md`:
//!
//! - C10: YAML 1.1 boolean words used as targets stay literal (`0: no` targets an app called "no", not "false").
//! - C12: duplicate keys use the last value, like upstream (the fork refused to load the file).
//! - C13: slider keys that aren't integers are ignored with a warning (originally they silently became slider 0).
//!
//! Anything ignored or unusual is reported as a [`Diagnostic`] instead of being dropped silently.

pub mod resolve;
pub mod sensitivity;
pub mod store;
pub mod writer;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::yaml::{self, Node, NodeKind};
use resolve::{Scalar, is_null, resolve_node, scalar_to_string, to_bool, to_int, to_string};
pub use sensitivity::{CURVE_PRESETS, CurvePreset, Sensitivity, SensitivityConfig, SensitivityOverride};

pub const DEFAULT_COM_PORT: &str = "COM4";
pub const DEFAULT_BAUD_RATE: u32 = 9600;
pub const DEFAULT_RECONNECT_INTERVAL_MS: u32 = 2000;

/// The config file shipped with deej Next (the fork's default, which upstream deej also reads correctly).
pub const DEFAULT_CONFIG_YAML: &str = include_str!("../../assets/default-config.yaml");

/// Top-level keys deej understands.
const KNOWN_KEYS: &[&str] = &[
    "slider_mapping",
    "invert_sliders",
    "com_port",
    "baud_rate",
    "noise_reduction",
    "slider_sensitivity",
    "unmapped_exclude",
    "deej_next",
];

/// Keys older deej versions used. Kept in the file, otherwise ignored.
const LEGACY_KEYS: &[&str] = &["process_refresh_frequency"];

/// How much jitter is filtered from slider readings. Matched case-sensitively, like the original;
/// anything other than `low` or `high` means [`NoiseReduction::Default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseReduction {
    Low,
    #[default]
    Default,
    High,
}

impl NoiseReduction {
    pub fn from_config(value: &str) -> NoiseReduction {
        match value {
            "low" => NoiseReduction::Low,
            "high" => NoiseReduction::High,
            _ => NoiseReduction::Default,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            NoiseReduction::Low => "low",
            NoiseReduction::Default => "default",
            NoiseReduction::High => "high",
        }
    }
}

/// Serial line options added by deej Next (`deej_next.serial`). The Go builds ignore this section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SerialOptions {
    /// Assert DTR when opening the port. Both Go versions did; native-USB boards may not send without it.
    pub dtr: bool,
    /// Assert RTS when opening the port. Upstream didn't, the fork did.
    pub rts: bool,
    pub reconnect_interval_ms: u32,
}

impl Default for SerialOptions {
    fn default() -> Self {
        SerialOptions { dtr: true, rts: false, reconnect_interval_ms: DEFAULT_RECONNECT_INTERVAL_MS }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct NextOptions {
    pub serial: SerialOptions,
}

/// Everything deej reads from `config.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    /// Slider index → targets, as written (case is preserved; matching is case-insensitive).
    /// A slider can be present with no targets (`3:`).
    pub slider_mapping: BTreeMap<usize, Vec<String>>,
    pub invert_sliders: bool,
    pub com_port: String,
    pub baud_rate: u32,
    pub noise_reduction: NoiseReduction,
    pub slider_sensitivity: SensitivityConfig,
    /// Lowercased, trimmed and de-duplicated process names that `deej.unmapped` leaves alone.
    pub unmapped_exclude: Vec<String>,
    pub next: NextOptions,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            slider_mapping: BTreeMap::new(),
            invert_sliders: false,
            com_port: DEFAULT_COM_PORT.to_string(),
            baud_rate: DEFAULT_BAUD_RATE,
            noise_reduction: NoiseReduction::Default,
            slider_sensitivity: SensitivityConfig::default(),
            unmapped_exclude: Vec::new(),
            next: NextOptions::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something was ignored or interpreted differently from what the user probably meant.
    Warning,
    /// Worth knowing, nothing was lost (e.g. a key from an older deej version).
    Info,
}

/// A problem or oddity found while reading the config. Shown in the Diagnostics screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Dotted key path, e.g. `slider_mapping.foo`.
    pub key: String,
    /// 1-based line in `config.yaml` (or `preferences.yaml`), when known.
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("config.yaml isn't valid YAML ({0})")]
    Syntax(#[from] yaml::SyntaxError),
    #[error("config.yaml must contain key: value settings at the top level")]
    NotAMapping,
}

/// The result of reading a config: the settings plus anything worth telling the user about.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses `config.yaml` source text. `preferences` is the optional `logs/preferences.yaml`, whose
/// `slider_mapping` the original deej merged into the main mapping.
pub fn parse(source: &str, preferences: Option<&str>) -> Result<Loaded, ConfigError> {
    let root = yaml::parse(source)?;
    if root.as_map().is_none() {
        return Err(ConfigError::NotAMapping);
    }

    let mut d = Diagnostics::default();
    let mut config = Config::default();

    report_duplicates(&root, "", &mut d);
    report_unknown_keys(&root, &mut d);

    // a null value counts as absent, so the default applies (viper behaves the same way)
    let get = |key: &str| root.get(key).filter(|node| !is_null(node));

    if let Some(node) = get("slider_mapping") {
        config.slider_mapping = parse_slider_mapping(node, "slider_mapping", &mut d);
    }

    if let Some(prefs_source) = preferences {
        merge_preferences(&mut config.slider_mapping, prefs_source, &mut d);
    }

    if let Some(node) = get("invert_sliders") {
        config.invert_sliders = to_bool(node);
        if !matches!(resolve_node(node), Some(Scalar::Bool(_))) {
            d.warn("invert_sliders", node.line, format!("expected true or false; read as {}", config.invert_sliders));
        }
    }

    if let Some(node) = get("com_port") {
        config.com_port = to_string(node).unwrap_or_default();
        if config.com_port.is_empty() {
            d.warn("com_port", node.line, "is empty or isn't a port name");
        }
    }

    if let Some(node) = get("baud_rate") {
        let value = to_int(node);
        match u32::try_from(value) {
            Ok(rate) if rate > 0 => config.baud_rate = rate,
            _ => d.warn("baud_rate", node.line, format!("{value} isn't a valid baud rate; using {DEFAULT_BAUD_RATE}")),
        }
    }

    if let Some(node) = get("noise_reduction") {
        let value = to_string(node).unwrap_or_default();
        config.noise_reduction = NoiseReduction::from_config(&value);
        if !matches!(value.as_str(), "low" | "default" | "high") {
            d.warn(
                "noise_reduction",
                node.line,
                format!("{value:?} isn't one of low, default or high (lowercase); using default"),
            );
        }
    }

    if let Some(node) = get("slider_sensitivity") {
        config.slider_sensitivity = sensitivity::parse(node, &mut d);
    }

    if let Some(node) = get("unmapped_exclude") {
        config.unmapped_exclude = parse_name_list(node, "unmapped_exclude", &mut d);
    }

    if let Some(node) = get("deej_next") {
        config.next = parse_next_options(node, &mut d);
    }

    Ok(Loaded { config, diagnostics: d.0 })
}

/// Collects diagnostics while parsing.
#[derive(Default)]
pub(crate) struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub(crate) fn warn(&mut self, key: impl Into<String>, line: usize, message: impl Into<String>) {
        self.push(Severity::Warning, key, Some(line), message);
    }

    pub(crate) fn info(&mut self, key: impl Into<String>, line: usize, message: impl Into<String>) {
        self.push(Severity::Info, key, Some(line), message);
    }

    fn push(&mut self, severity: Severity, key: impl Into<String>, line: Option<usize>, message: impl Into<String>) {
        self.0.push(Diagnostic { severity, key: key.into(), line, message: message.into() });
    }
}

fn join_key(parent: &str, key: &str) -> String {
    if parent.is_empty() { key.to_string() } else { format!("{parent}.{key}") }
}

/// Warns about keys that appear more than once in any mapping, recursively.
fn report_duplicates(node: &Node, path: &str, d: &mut Diagnostics) {
    match &node.kind {
        NodeKind::Map(entries) => {
            for (key, lines) in node.duplicate_keys() {
                let full = join_key(path, &key);
                let line = *lines.last().unwrap_or(&node.line);
                d.warn(full, line, format!("appears {} times (lines {lines:?}); the last one is used", lines.len()));
            }

            for (key, value) in entries {
                let name = to_string(key).unwrap_or_default();
                report_duplicates(value, &join_key(path, &name), d);
            }
        }
        NodeKind::Seq(items) => items.iter().for_each(|item| report_duplicates(item, path, d)),
        NodeKind::Scalar { .. } => {}
    }
}

fn report_unknown_keys(root: &Node, d: &mut Diagnostics) {
    let Some(entries) = root.as_map() else { return };

    for (key, _) in entries {
        let name = to_string(key).unwrap_or_default();
        let lower = name.to_lowercase();

        if LEGACY_KEYS.contains(&lower.as_str()) {
            d.info(name, key.line, "is from an older deej version and no longer has any effect");
        } else if !KNOWN_KEYS.contains(&lower.as_str()) {
            d.info(name, key.line, "isn't a deej setting; it's kept in the file but ignored");
        }
    }
}

/// `slider_mapping`, as viper's `GetStringMapStringSlice` + deej's `sliderMapFromConfigs` read it.
fn parse_slider_mapping(node: &Node, path: &str, d: &mut Diagnostics) -> BTreeMap<usize, Vec<String>> {
    let mut result = BTreeMap::new();

    let Some(entries) = node.as_map() else {
        d.warn(path, node.line, "should list sliders as `index: target`; ignored");
        return result;
    };

    // which source line each slider index came from, to report collisions like "1" and "01"
    let mut defined_at: BTreeMap<usize, usize> = BTreeMap::new();

    for (key, value) in entries {
        let key_text = to_string(key).unwrap_or_default();
        let key_path = join_key(path, &key_text);

        let index = match key_text.parse::<i64>() {
            Ok(i) if i >= 0 => i as usize,
            Ok(_) => {
                d.warn(key_path, key.line, "slider indexes start at 0; this entry is ignored");
                continue;
            }
            Err(_) => {
                d.warn(key_path, key.line, "isn't a slider index (0, 1, 2, ...); this entry is ignored");
                continue;
            }
        };

        if let Some(previous) = defined_at.insert(index, key.line) {
            if previous != key.line && !is_same_key_repeated(entries, key, previous) {
                d.warn(
                    key_path.clone(),
                    key.line,
                    format!("slider {index} is also defined on line {previous}; this one is used"),
                );
            }
        }

        result.insert(index, parse_targets(value));
    }

    result
}

/// Exact repeats of a key (same text, ignoring case) are already reported by `report_duplicates`.
fn is_same_key_repeated(entries: &[(Node, Node)], key: &Node, other_line: usize) -> bool {
    let raw = |n: &Node| n.as_scalar().map(|(text, _)| text.to_lowercase());
    entries.iter().any(|(k, _)| k.line == other_line && raw(k) == raw(key))
}

/// A slider's targets: one scalar or a list. Non-scalar items become empty and are dropped, as in the original.
pub(crate) fn parse_targets(node: &Node) -> Vec<String> {
    let items: Vec<String> = match &node.kind {
        NodeKind::Seq(items) => items.iter().map(target_text).collect(),
        NodeKind::Scalar { .. } => vec![target_text(node)],
        NodeKind::Map(_) => Vec::new(),
    };

    items.into_iter().filter(|t| !t.is_empty()).collect()
}

/// A target's text. YAML 1.1 boolean words stay as written (C10); numbers are formatted as Go did (`010` → "8").
fn target_text(node: &Node) -> String {
    match (resolve_node(node), node.as_scalar()) {
        (Some(scalar), Some((raw, _))) if scalar.is_bool() => raw.to_string(),
        (Some(scalar), _) => scalar_to_string(&scalar),
        (None, _) => String::new(),
    }
}

/// Merges `logs/preferences.yaml`'s mapping: its targets are appended to the same slider, skipping duplicates.
fn merge_preferences(mapping: &mut BTreeMap<usize, Vec<String>>, source: &str, d: &mut Diagnostics) {
    let root = match yaml::parse(source) {
        Ok(root) => root,
        Err(err) => {
            // the original treated an unreadable preferences file as absent
            d.push(Severity::Warning, "preferences.yaml", Some(err.line), format!("couldn't be read: {err}"));
            return;
        }
    };

    let Some(node) = root.get("slider_mapping").filter(|n| !is_null(n)) else { return };

    let mut scratch = Diagnostics::default();
    let extra = parse_slider_mapping(node, "preferences.slider_mapping", &mut scratch);
    d.0.extend(scratch.0);

    for (index, targets) in extra {
        let existing = mapping.entry(index).or_default();
        for target in targets {
            if !existing.contains(&target) {
                existing.push(target);
            }
        }
    }
}

/// A list of process names, as viper's `GetStringSlice` reads it (a single string is split on whitespace),
/// then normalized like the fork's `normalizeProcessNames`.
fn parse_name_list(node: &Node, path: &str, d: &mut Diagnostics) -> Vec<String> {
    let raw: Vec<String> = match (&node.kind, resolve_node(node)) {
        (NodeKind::Seq(items), _) => items.iter().map(|item| to_string(item).unwrap_or_default()).collect(),
        (_, Some(Scalar::Str(s))) => s.split_whitespace().map(str::to_string).collect(),
        _ => {
            d.warn(path, node.line, "should be a list of app names; ignored");
            return Vec::new();
        }
    };

    let mut result: Vec<String> = Vec::new();
    for name in raw {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !result.contains(&name) {
            result.push(name);
        }
    }

    result
}

fn parse_next_options(node: &Node, d: &mut Diagnostics) -> NextOptions {
    let mut options = NextOptions::default();

    let Some(serial) = node.get("serial").filter(|n| !is_null(n)) else { return options };

    if let Some(n) = serial.get("dtr").filter(|n| !is_null(n)) {
        options.serial.dtr = to_bool(n);
    }

    if let Some(n) = serial.get("rts").filter(|n| !is_null(n)) {
        options.serial.rts = to_bool(n);
    }

    if let Some(n) = serial.get("reconnect_interval_ms").filter(|n| !is_null(n)) {
        let value = to_int(n);
        match u32::try_from(value) {
            Ok(ms) if (250..=60_000).contains(&ms) => options.serial.reconnect_interval_ms = ms,
            _ => d.warn(
                "deej_next.serial.reconnect_interval_ms",
                n.line,
                format!("{value} is outside 250–60000; using {DEFAULT_RECONNECT_INTERVAL_MS}"),
            ),
        }
    }

    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(src: &str) -> Loaded {
        parse(src, None).unwrap()
    }

    #[test]
    fn defaults_when_empty() {
        assert_eq!(load("").config, Config::default());
        assert!(load("# just a comment\n").diagnostics.is_empty());
    }

    #[test]
    fn shipped_default_config_loads_cleanly() {
        let loaded = load(DEFAULT_CONFIG_YAML);
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        assert_eq!(loaded.config.slider_mapping[&0], vec!["master"]);
        assert_eq!(loaded.config.slider_mapping[&3], vec!["pathofexile_x64.exe", "rocketleague.exe"]);
    }

    #[test]
    fn bool_words_as_targets_stay_literal() {
        let loaded = load("slider_mapping:\n  0: no\n  1: [off, 010]\n");
        assert_eq!(loaded.config.slider_mapping[&0], vec!["no"]);
        assert_eq!(loaded.config.slider_mapping[&1], vec!["off", "8"]);
    }

    #[test]
    fn non_integer_slider_keys_are_ignored_with_a_warning() {
        let loaded = load("slider_mapping:\n  0: master\n  foo: chrome.exe\n  -1: x.exe\n");
        assert_eq!(loaded.config.slider_mapping.len(), 1);
        assert_eq!(loaded.config.slider_mapping[&0], vec!["master"]);

        let keys: Vec<_> = loaded.diagnostics.iter().map(|d| (d.key.as_str(), d.line)).collect();
        assert_eq!(keys, vec![("slider_mapping.foo", Some(3)), ("slider_mapping.-1", Some(4))]);
    }

    #[test]
    fn colliding_slider_keys_use_the_last_and_warn() {
        let loaded = load("slider_mapping:\n  1: a.exe\n  01: b.exe\n");
        assert_eq!(loaded.config.slider_mapping[&1], vec!["b.exe"]);
        assert_eq!(loaded.diagnostics.len(), 1);
    }

    #[test]
    fn unknown_and_legacy_keys_are_reported_not_fatal() {
        let loaded = load("process_refresh_frequency: 5\nmy_note: hi\n");
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.diagnostics.len(), 2);
        assert!(loaded.diagnostics.iter().all(|d| d.severity == Severity::Info));
    }

    #[test]
    fn deej_next_serial_options() {
        let loaded = load("deej_next:\n  serial:\n    dtr: no\n    rts: yes\n    reconnect_interval_ms: 5\n");
        assert_eq!(loaded.config.next.serial, SerialOptions { dtr: false, rts: true, reconnect_interval_ms: 2000 });
        assert_eq!(loaded.diagnostics.len(), 1);
    }

    #[test]
    fn root_must_be_a_mapping() {
        assert_eq!(parse("- a\n- b\n", None), Err(ConfigError::NotAMapping));
        assert!(matches!(parse("a: [\n", None), Err(ConfigError::Syntax(_))));
    }
}
