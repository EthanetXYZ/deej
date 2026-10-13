# deej

The new deej: a rewrite of [deej](https://github.com/omriharel/deej) for Windows, in Rust with a Tauri UI ("deej Next"). It works with the same
Arduino firmware and the same `config.yaml`, so an existing deej setup carries over unchanged.

- Physical sliders set the volume of apps, devices, the master volume, the microphone or the focused app.
- Response curves per slider (finer control at low volume), shown live as you move the slider.
- A window for mappings, curves, the board connection and settings; everything saves to `config.yaml` with your
  comments and formatting kept.
- Runs in the tray: mute, playback device switching, Start with Windows. Recovers from unplugging the board, sleep,
  and audio devices coming and going.

Building the hardware: the [FAQ](docs/FAQ.md) and the [Arduino sketch](arduino/deej-5-sliders-vanilla/deej-5-sliders-vanilla.ino). Status and test results: [DEVELOPMENT_STATUS.md](DEVELOPMENT_STATUS.md).

The previous Go version (Windows and Linux) stays available on the [`master`](https://github.com/EthanetXYZ/deej/tree/master) branch.

## Install

Requirements: Windows 10 or 11 (64-bit) and the Microsoft Edge WebView2 runtime, which Windows 11 and up-to-date
Windows 10 already have.

**Installer:** run `deej_<version>_x64-setup.exe`. It installs for your user only (no admin rights) into
`%LOCALAPPDATA%\deej`, with a Start menu entry. `config.yaml` and `logs\` are created next to the program.

**Portable ZIP:** unzip anywhere you can write to (for example `C:\deej-next`) and run `deej-next.exe`. Put your
`config.yaml` next to it, or let deej create the default one.

deej looks for `config.yaml` next to the exe, then in the folder it was started from, then in `%APPDATA%\deej`.

## Moving over from the original deej

1. Quit the original deej (right-click its tray icon → Quit). Only one program can use the board's COM port.
2. Copy your `config.yaml` next to `deej-next.exe` (or install deej Next into the same folder). Nothing in it needs
   changing; settings from the fork (`slider_sensitivity`, `unmapped_exclude`) work as they are.
3. Start deej Next. If the original deej started with Windows, Settings → General shows it under "Another deej also
   starts with Windows". Remove that entry (the button opens the Startup folder), then turn on Start with Windows for
   deej Next.

Going back is just as simple: deej Next only adds an optional `deej_next:` section to `config.yaml`, which the original
deej ignores, and keeps its own window settings in `deej-next-ui.json`.

Differences worth knowing:
- A missing or broken `config.yaml` doesn't stop deej: a missing one is created from the default, and while the file
  has an error the previous settings stay active and the error is shown.
- An app that starts while its slider is set gets that slider's volume straight away.
- Slider keys that aren't whole numbers (`1.5:`) are ignored with a note, instead of silently becoming slider 0.
- The full list is in [COMPATIBILITY_MATRIX.md](docs/COMPATIBILITY_MATRIX.md).

## Using it

- **Left-click the tray icon** to open the window. Closing the window keeps deej running; **right-click → Quit deej**
  stops it.
- **Sliders page:** each slider's targets and response curve. Move a physical slider to see which card it is.
- **Hardware page:** connection, port, baud rate and the raw lines the board sends.
- **Settings:** Start with Windows, notifications, invert, noise reduction, apps left alone by "All other apps",
  appearance, and serial options for boards that need them.
- Command line: `--minimized` (start in the tray), `--verbose` (detailed log), `--settings` (accepted for
  compatibility). `DEEJ_NO_TRAY_ICON=1` runs without a tray icon; closing the window then quits.

## Troubleshooting

| What you see | What to do |
|---|---|
| "COM3 is in use by another program" | The original deej, the Arduino IDE's serial monitor, or another copy is holding the port. Close it; deej reconnects by itself. |
| "COM3 not found" | Check the port in Hardware (the list shows USB boards). The board may have a new COM number after moving it to another USB port. |
| Connected but sliders don't move | Hardware → recent lines should show `512|1023|…`. Nothing at all: some boards (Pro Micro, RP2040, ESP32) need DTR, set in Settings → Advanced. Garbled text: the baud rate doesn't match the sketch. |
| Volumes drift while nobody touches the sliders | Raise noise reduction (Settings → Hardware). `low` lets 2 % of jitter through. |
| A slider is upside down | Settings → Hardware → Invert sliders. |
| An app isn't controlled | Mappings use the exe name (`spotify.exe`). The Sliders page shows what each slider controls right now. |
| No window appears | deej may already be running in the tray (overflow arrow next to the clock). Starting it again opens the existing window. |
| No notifications | Windows Settings → System → Notifications → deej. Routine ones can be turned off in deej's Settings. |
| Anything else | Settings → Diagnostics → Open logs folder. `deej-latest-run.log` and `deej-crash.log` (if present) explain most problems; start with `--verbose` for more detail. |

## Building from source

Requirements: Rust (stable, MSVC), Visual Studio C++ build tools, Node 22, and the WebView2 runtime.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Development (hot-reloads the UI; `npm run dev` alone runs the UI in a browser against a mock backend):

```bash
cd app && npm install && npx tauri dev
```

Release exe and installer (`target/release/deej-next.exe`, `target/release/bundle/nsis/`):

```bash
cd app && npx tauri build
```

## Layout

| Path | What |
|---|---|
| `crates/deej-core` | Platform-independent logic: config (viper-compatible), protocol, mapping, sensitivity |
| `crates/deej-audio` | Windows Core Audio engine (one thread owns every COM object) |
| `crates/deej-serial` | Board connection: reconnects, noise filter, diagnostics |
| `crates/deej-platform` | Autostart, sleep/resume, foreground app, icons, single instance |
| `app/` | React + TypeScript + Tailwind UI (Vite) |
| `app/src-tauri` | The Tauri app: coordinator, commands, tray, window |
| `arduino/` | The board sketch (unchanged from the original deej) |
| `docs/` | FAQ, architecture, compatibility matrix, config reference, analysis of the original |
| `testdata/` | Config fixtures and what the Go deej produced for them ([tools/golden-gen](tools/golden-gen/README.md)) |

Design docs: [architecture](docs/ARCHITECTURE.md), [compatibility matrix](docs/COMPATIBILITY_MATRIX.md),
[config reference](docs/CONFIGURATION_REFERENCE.md), [analysis of the original](docs/ORIGINAL_DEEJ_ANALYSIS.md).
