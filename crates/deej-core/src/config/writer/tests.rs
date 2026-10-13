use super::*;
use crate::config::{self, DEFAULT_CONFIG_YAML};

fn edit(source: &str, edits: &[Edit]) -> String {
    apply_edits(source, edits).unwrap_or_else(|e| panic!("{e}\n--- source:\n{source}"))
}

fn str(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn list(items: &[&str]) -> Value {
    Value::List(items.iter().map(|s| s.to_string()).collect())
}

const SAMPLE: &str = "# process names are case-insensitive
slider_mapping:
  0: master
  1: chrome.exe   # browser
  3:
    - pathofexile_x64.exe
    - rocketleague.exe

# set this to true if you want the controls inverted
invert_sliders: false

# settings for connecting to the arduino board
com_port: COM4
baud_rate: 9600

slider_sensitivity:
  default:
    curve: 1
    max_volume: 100
";

#[test]
fn changing_a_scalar_touches_only_that_value() {
    let out = edit(SAMPLE, &[Edit::set(&["invert_sliders"], Value::Bool(true))]);
    assert_eq!(out, SAMPLE.replace("invert_sliders: false", "invert_sliders: true"));

    let out = edit(SAMPLE, &[Edit::set(&["slider_mapping", "1"], str("Spotify.exe"))]);
    assert_eq!(
        out,
        SAMPLE.replace("1: chrome.exe   # browser", "1: Spotify.exe   # browser"),
        "keeps the line comment"
    );
}

#[test]
fn the_shipped_default_config_survives_every_kind_of_edit() {
    let edits = [
        Edit::set(&["com_port"], str("COM3")),
        Edit::set(&["slider_mapping", "2"], list(&["spotify.exe", "vlc.exe"])),
        Edit::set(&["slider_mapping", "7"], str("deej.unmapped")),
        Edit::remove(&["slider_mapping", "4"]),
        Edit::set(&["unmapped_exclude"], list(&["mpvnet.exe"])),
        Edit::set(&["slider_sensitivity", "1", "curve"], Value::Float(2.5)),
        Edit::set(&["deej_next", "serial", "rts"], Value::Bool(true)),
    ];

    let out = edit(DEFAULT_CONFIG_YAML, &edits);

    // every comment line of the original is still there, in order
    let comments =
        |s: &str| s.lines().filter(|l| l.trim_start().starts_with('#')).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(comments(&out), comments(DEFAULT_CONFIG_YAML));

    let c = config::parse(&out, None).unwrap().config;
    assert_eq!(c.com_port, "COM3");
    assert_eq!(c.slider_mapping[&2], vec!["spotify.exe", "vlc.exe"]);
    assert_eq!(c.slider_mapping[&7], vec!["deej.unmapped"]);
    assert!(!c.slider_mapping.contains_key(&4));
    assert_eq!(c.unmapped_exclude, vec!["mpvnet.exe"]);
    assert_eq!(c.slider_sensitivity.sliders[&1].curve, Some(2.5));
    assert_eq!(c.slider_sensitivity.default.curve, Some(1.0));
    assert!(c.next.serial.rts);
}

#[test]
fn new_slider_keys_go_in_numeric_order() {
    let out = edit(SAMPLE, &[Edit::set(&["slider_mapping", "2"], str("discord.exe"))]);
    assert!(out.contains("  1: chrome.exe   # browser\n  2: discord.exe\n  3:\n"), "{out}");

    let out = edit(SAMPLE, &[Edit::set(&["slider_mapping", "9"], str("vlc.exe"))]);
    assert!(out.contains("    - rocketleague.exe\n  9: vlc.exe\n\n# set this"), "{out}");
}

#[test]
fn lists_are_written_as_blocks_and_empty_lists_as_brackets() {
    let out = edit(SAMPLE, &[Edit::set(&["slider_mapping", "1"], list(&["chrome.exe", "firefox.exe"]))]);
    assert!(out.contains("  1:\n    - chrome.exe\n    - firefox.exe\n  3:\n"), "{out}");

    let out = edit(SAMPLE, &[Edit::set(&["slider_mapping", "3"], str("game.exe"))]);
    assert!(out.contains("  3: game.exe\n\n# set this"), "{out}");

    let out = edit("unmapped_exclude:\n  - a.exe\n  - b.exe\nx: 1\n", &[Edit::set(&["unmapped_exclude"], list(&[]))]);
    assert_eq!(out, "unmapped_exclude: []\nx: 1\n");
}

#[test]
fn removing_keys_and_sections_left_empty() {
    let out = edit(SAMPLE, &[Edit::remove(&["slider_mapping", "3"])]);
    assert!(out.contains("  1: chrome.exe   # browser\n\n# set this"), "{out}");

    // the last field of a section removes the section (the fork did the same)
    let out = edit(SAMPLE, &[Edit::remove(&["slider_sensitivity", "default", "curve"])]);
    assert!(out.ends_with("slider_sensitivity:\n  default:\n    max_volume: 100\n"), "{out}");
    let out = edit(&out, &[Edit::remove(&["slider_sensitivity", "default", "max_volume"])]);
    assert!(!out.contains("slider_sensitivity"), "{out}");

    // except slider_mapping, which stays (empty) under its comments
    let one = "# mappings\nslider_mapping:\n  0: master\ncom_port: COM3\n";
    assert_eq!(
        edit(one, &[Edit::remove(&["slider_mapping", "0"])]),
        "# mappings\nslider_mapping: {}\ncom_port: COM3\n"
    );

    // removing something that isn't there changes nothing
    assert_eq!(edit(SAMPLE, &[Edit::remove(&["slider_mapping", "8"])]), SAMPLE);
}

#[test]
fn creates_missing_sections() {
    let out = edit(SAMPLE, &[Edit::set(&["deej_next", "serial", "dtr"], Value::Bool(false))]);
    assert!(out.ends_with("    max_volume: 100\n\ndeej_next:\n  serial:\n    dtr: false\n"), "{out}");

    let out = edit("", &[Edit::set(&["com_port"], str("COM3"))]);
    assert_eq!(out, "com_port: COM3\n");

    let out = edit("# only a comment\n", &[Edit::set(&["slider_mapping", "0"], str("master"))]);
    assert_eq!(out, "# only a comment\nslider_mapping:\n  0: master\n");
}

#[test]
fn null_and_flow_sections_become_blocks() {
    let out = edit("slider_mapping:\ncom_port: COM3\n", &[Edit::set(&["slider_mapping", "0"], str("master"))]);
    assert_eq!(out, "slider_mapping:\n  0: master\ncom_port: COM3\n");

    let out =
        edit("slider_mapping: {0: master, 1: [a.exe, b.exe]}\n", &[Edit::set(&["slider_mapping", "2"], str("c.exe"))]);
    assert_eq!(out, "slider_mapping:\n  0: master\n  1:\n    - a.exe\n    - b.exe\n  2: c.exe\n");

    let out = edit(
        "slider_mapping:\n  0: master\n  3:\nx: 1\n",
        &[Edit::set(&["slider_mapping", "3"], list(&["a.exe", "b.exe"]))],
    );
    assert_eq!(out, "slider_mapping:\n  0: master\n  3:\n    - a.exe\n    - b.exe\nx: 1\n");
}

#[test]
fn line_endings_and_bom_are_kept() {
    let crlf = SAMPLE.replace('\n', "\r\n");
    let out = edit(&crlf, &[Edit::set(&["slider_mapping", "2"], list(&["a.exe", "b.exe"]))]);
    assert!(!out.replace("\r\n", "").contains('\n'), "only CRLF line endings");

    let bom = format!("\u{feff}{SAMPLE}");
    let out = edit(&bom, &[Edit::set(&["com_port"], str("COM9"))]);
    assert!(out.starts_with("\u{feff}# process names"));
    assert!(out.contains("com_port: COM9"));
}

#[test]
fn strings_are_quoted_only_when_needed() {
    for s in ["chrome.exe", "Speakers (Realtek(R) Audio)", "deej.unmapped", "COM3", "My App.exe"] {
        assert_eq!(render_string(s), s);
    }

    for s in ["yes", "No", "010", "1.5", "~", "", " lead", "a: b", "#hash", "x #y", "- dash", "tab\there"] {
        let rendered = render_string(s);
        assert_ne!(rendered, s, "{s:?} must be quoted");
        let out = edit("x: 1\n", &[Edit::set(&["x"], str(s))]);
        let root = yaml::parse(&out).unwrap();
        assert_eq!(root.get("x").unwrap().as_scalar().unwrap().0, s, "{s:?} round-trips");
    }
}

#[test]
fn unusual_strings_round_trip() {
    for s in ["quote\"d", "back\\slash", "émoji 🎚", "a'b", "50%", "C:\\deej\\x.exe"] {
        let out = edit("x: 1\n", &[Edit::set(&["x"], str(s))]);
        assert_eq!(yaml::parse(&out).unwrap().get("x").unwrap().as_scalar().unwrap().0, s, "{s:?}");
    }
}

#[test]
fn keys_written_as_other_number_forms_are_found() {
    let out = edit("slider_mapping:\n  01: a.exe\n", &[Edit::set(&["slider_mapping", "1"], str("b.exe"))]);
    assert_eq!(out, "slider_mapping:\n  01: b.exe\n");
}

#[test]
fn every_fixture_can_be_edited_without_changing_anything_else() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/configs");

    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(&path).unwrap();

        let Ok(before) = config::parse(&source, None) else { continue };
        if yaml::parse(&source).unwrap().get("slider_mapping").is_some_and(|n| matches!(n.kind, NodeKind::Seq(_))) {
            continue; // not a section; editing it is refused (tested below)
        }

        match apply_edits(
            &source,
            &[Edit::set(&["com_port"], str("COM7")), Edit::set(&["slider_mapping", "20"], str("x.exe"))],
        ) {
            Ok(out) => {
                let after = config::parse(&out, None).unwrap_or_else(|e| panic!("{name}: {e}\n{out}"));
                assert_eq!(after.config.com_port, "COM7", "{name}");
                assert_eq!(after.config.slider_mapping[&20], vec!["x.exe"], "{name}");

                let mut expected = before.config.clone();
                expected.com_port = "COM7".into();
                expected.slider_mapping.insert(20, vec!["x.exe".into()]);
                assert_eq!(after.config, expected, "{name}: nothing else may change\n{out}");
            }
            Err(WriteError::NotASection(_)) => {}
            Err(err) => panic!("{name}: {err}"),
        }
    }
}

#[test]
fn refuses_to_turn_a_value_into_a_section() {
    let err =
        apply_edits("slider_mapping: master\n", &[Edit::set(&["slider_mapping", "0"], str("master"))]).unwrap_err();
    assert_eq!(err, WriteError::NotASection("slider_mapping".into()));
}
