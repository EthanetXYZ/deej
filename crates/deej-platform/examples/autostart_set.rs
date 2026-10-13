//! Turns Start with Windows on or off for an executable: `cargo run -p deej-platform --example autostart_set -- <exe> on|off`

fn main() {
    let mut args = std::env::args().skip(1);
    let exe = std::path::PathBuf::from(args.next().expect("exe"));
    let on = args.next().as_deref() != Some("off");
    match deej_platform::autostart::set(&exe, on) {
        Ok(()) => println!("set {on}: {:?}", deej_platform::autostart::status(&exe)),
        Err(err) => println!("failed: {err}"),
    }
}
