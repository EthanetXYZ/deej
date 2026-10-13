//! Prints what starts with Windows for an executable: `cargo run -p deej-platform --example autostart_probe -- <exe>`
//! (default: this example's own path). Read-only.

fn main() {
    let exe = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| std::env::current_exe().unwrap());
    let status = deej_platform::autostart::status(&exe);
    println!("exe: {}", exe.display());
    println!("enabled: {} (via shortcut: {})", status.enabled, status.via_shortcut);
    for other in &status.others {
        println!("other deej: {other}");
    }
}
