//! Stress modes for finding memory growth; watch the process's private bytes from outside.
//!
//! ```text
//! cargo run --release -p deej-audio --example leak_probe -- rescan 600   # full re-scan every 100 ms
//! cargo run --release -p deej-audio --example leak_probe -- volume 600   # 50 volume sets/s on an idle app
//! cargo run --release -p deej-audio --example leak_probe -- idle 600
//! ```
//!
//! `volume` nudges the system sounds session by ±1 % around its current volume and restores it at the end.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deej_audio::{AudioEngine, AudioEvent, AudioSnapshot, Command, VolumeTarget};
use deej_core::audio::AudioControl;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "idle".into());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let snapshot = Arc::new(Mutex::new(AudioSnapshot::default()));
    let events = Arc::new(Mutex::new(0u64));
    let (s, e) = (snapshot.clone(), events.clone());
    let engine = AudioEngine::start(Box::new(move |event| {
        *e.lock().unwrap() += 1;
        if let AudioEvent::Snapshot(snap) = event {
            *s.lock().unwrap() = snap;
        }
    }))
    .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    engine.send(Command::RequestSnapshot);
    std::thread::sleep(Duration::from_secs(1));

    let target = snapshot.lock().unwrap().sessions.iter().find(|s| s.is_system).cloned();
    println!("pid {} mode {mode} for {seconds}s, target {:?}", std::process::id(), target.as_ref().map(|t| &t.key));

    if mode == "rescan" {
        engine.send(Command::SetReconcileInterval(Duration::from_millis(100)));
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut flip = false;
    while Instant::now() < end {
        match (mode.as_str(), &target) {
            ("volume", Some(t)) => {
                flip = !flip;
                let volume = (t.volume + if flip { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
                engine.send(Command::SetVolume { target: VolumeTarget::Session(t.id), volume });
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    if let (true, Some(t)) = (mode == "volume", &target) {
        engine.send(Command::SetVolume { target: VolumeTarget::Session(t.id), volume: t.volume });
        println!("restored {} to {:.0}%", t.key, t.volume * 100.0);
    }
    std::thread::sleep(Duration::from_millis(300));
    println!("events {}", events.lock().unwrap());
}
