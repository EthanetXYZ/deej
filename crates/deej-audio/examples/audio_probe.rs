//! Prints what the audio engine sees, then every change, for a number of seconds. Read-only: it never changes
//! a volume.
//!
//! ```text
//! cargo run -p deej-audio --example audio_probe -- 30
//! ```
//!
//! Useful at a review stop: switch the default playback device, change an app's volume in the Windows mixer,
//! start or close an app that plays sound, and watch the events arrive.

use std::time::Duration;

use deej_audio::{AudioEngine, AudioEvent, AudioSnapshot};

fn print_snapshot(s: &AudioSnapshot) {
    println!("== snapshot: available={}", s.available);
    for e in &s.endpoints {
        println!(
            "  {:?}{} {:<70} vol {:>5.1}%{}",
            e.flow,
            if e.is_default { "*" } else { " " },
            e.name,
            e.volume * 100.0,
            if e.muted { " muted" } else { "" }
        );
    }
    for x in &s.sessions {
        let device = s.endpoint(&x.endpoint_id).map(|e| e.name.as_str()).unwrap_or("?");
        println!(
            "  session {:>3} {:<24} pid {:>6} {:?} vol {:>5.1}%{}  on {device}",
            x.id,
            x.key,
            x.pid,
            x.state,
            x.volume * 100.0,
            if x.muted { " muted" } else { "" }
        );
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("DEEJ_LOG").unwrap_or_else(|_| "info".into())).init();

    let seconds: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(5);

    let engine = AudioEngine::start(Box::new(|event| match event {
        AudioEvent::Snapshot(s) => print_snapshot(&s),
        AudioEvent::VolumeChanged { target, volume, muted, external } => println!(
            "  volume {target:?} -> {:.1}%{}{}",
            volume * 100.0,
            if muted { " muted" } else { "" },
            if external { " (external)" } else { "" }
        ),
    }))
    .expect("start audio engine");

    std::thread::sleep(Duration::from_secs(seconds));
    engine.shutdown();
    println!("== engine stopped cleanly");
}
