//! Shows what deej's config writer would do to a file, without changing it.
//!
//! ```text
//! cargo run -p deej-core --example edit_preview -- C:\deej\config.yaml
//! ```
//!
//! Applies a fixed set of typical edits and prints the result.

use deej_core::config::writer::{Edit, Value, apply_edits};

fn main() {
    let path = std::env::args().nth(1).expect("path to a config.yaml");
    let source = std::fs::read_to_string(&path).expect("read the file");

    let edits = [
        Edit::set(&["slider_sensitivity", "default", "curve"], Value::Float(2.5)),
        Edit::set(&["slider_mapping", "5"], Value::Str("deej.current".into())),
        Edit::set(&["unmapped_exclude"], Value::List(vec!["mpvnet.exe".into(), "vlc.exe".into()])),
        Edit::set(&["deej_next", "serial", "rts"], Value::Bool(false)),
    ];

    match apply_edits(&source, &edits) {
        Ok(out) => print!("{out}"),
        Err(err) => eprintln!("refused: {err}"),
    }
}
