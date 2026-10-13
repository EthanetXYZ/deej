//! Connects to the board and prints connection changes and slider moves. Read-only.
//!
//! ```text
//! cargo run -p deej-serial --example board_probe -- COM3 9600 30
//! ```
//!
//! Arguments: port (default COM3), baud rate (default 9600), seconds to run (default 10).
//! `--rts` asserts RTS too, `--no-dtr` leaves DTR off (to see what a board needs).

use std::time::Duration;

use deej_core::config::NoiseReduction;
use deej_serial::{ReadingOptions, SerialConnection, SerialEvent, SerialSettings, available_ports};

fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("DEEJ_LOG").unwrap_or_else(|_| "info".into())).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let port = positional.first().map(|s| s.to_string()).unwrap_or_else(|| "COM3".into());
    let baud = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(9600);
    let seconds = positional.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);

    println!("== ports");
    for p in available_ports() {
        println!("  {} usb={:04x?} {:?} {:?}", p.name, p.usb, p.manufacturer, p.product);
    }

    let settings = SerialSettings {
        port,
        baud_rate: baud,
        dtr: !args.iter().any(|a| a == "--no-dtr"),
        rts: args.iter().any(|a| a == "--rts"),
        reconnect_interval: Duration::from_secs(2),
    };
    println!("== connecting with {settings:?}");

    let connection = SerialConnection::start(
        settings,
        ReadingOptions { invert: false, noise: NoiseReduction::Default },
        Box::new(|event| match event {
            SerialEvent::Status(s) => println!("  status: {s:?}"),
            SerialEvent::Moves { moves, positions, .. } => {
                let moved: Vec<String> =
                    moves.iter().map(|m| format!("{}→{:.0}%", m.slider, m.position * 100.0)).collect();
                let all: Vec<String> = positions.iter().map(|p| format!("{:>3.0}", p * 100.0)).collect();
                println!("  moved {:<30} positions [{}]", moved.join(" "), all.join(" "));
            }
        }),
    )
    .expect("start serial connection");

    std::thread::sleep(Duration::from_secs(seconds));

    let d = connection.diagnostics();
    println!(
        "== diagnostics: {} valid lines ({:.0}/s), {} invalid, {} sliders, {} bytes, connects {}, last error {:?}",
        d.valid_lines,
        d.valid_lines as f64 / seconds as f64,
        d.invalid_lines,
        d.slider_count,
        d.bytes,
        d.connects,
        d.last_error
    );
    println!("   recent lines: {:?}", d.recent_lines.iter().rev().take(3).collect::<Vec<_>>());
    connection.stop();
    println!("== stopped cleanly");
}
