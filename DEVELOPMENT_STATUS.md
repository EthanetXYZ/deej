# deej Next: development status

_Last updated: 2026-10-13_

> Layout note: development happened in a `deej-next/` folder beside the Go fork, so paths below may start with `deej-next/`. On `main` that folder is the repository root (D28).

## Current phase

**Phase 8: Release. v0.1.2 built** (installer, portable ZIP, notes and checksums in `releases/deej-next-v0.1.2/`, not published). Phase 7 was shortened at the owner's request (2026-10-13); see "Known issues" for what it found.

## Completed work

### Phase 0: Analysis (commit 7f23905)

- The four analysis documents in `docs/`.
- Owner decisions recorded as D12 in [ARCHITECTURE.md §9](docs/ARCHITECTURE.md#9-decisions-recorded-so-far).

### Phase 1: Foundation

- **Toolchain:** Rust 1.99.0 stable (MSVC) installed. Node 22 and npm were already present.
- **Workspace** `deej-next/`:
  - `crates/deej-core` (platform-independent)
  - `app/` (React 19 + TypeScript 5.9 + Tailwind 4 on Vite 8)
  - `app/src-tauri` (Tauri 2.12)
- **YAML parser:** after a spike, `saphyr-parser` was chosen (D9), because it gives exact source spans for the comment-preserving writer.
- **Config loader** (`deej-core::config`):
  - viper/yaml.v2/cast-compatible value rules: YAML 1.1 booleans, Go `ParseInt` base 0, octal, underscores, and float fallbacks.
  - Duplicate keys: last wins.
  - `logs/preferences.yaml` merge.
  - Fork keys (`slider_sensitivity`, `unmapped_exclude`) and the new `deej_next.serial` options.
  - Diagnostics with line numbers for everything ignored or unusual.
- **Golden testing** (`tools/golden-gen`): a Go program built against both upstream's and the fork's dependencies records what real deej does with 52 fixture configs. The Rust tests compare against that output, and the deliberate differences (C10, C13) are listed explicitly in the test. A mutation check confirmed the test catches a broken rule.
- **Config location:** next to the exe, then the working directory, then `%APPDATA%\deej`. A new config goes to AppData if the exe folder is read-only.
- **App startup:** never fails on config. A missing file is created from the default; an invalid one means defaults are used and the error is shown.
- **Logging** (`tracing`): `logs/deej-latest-run.log`, rotated to `deej-previous-run.log`. A panic hook writes `deej-crash.log` (and keeps the previous crash log). `--verbose`/`-v` and `DEEJ_LOG=<filter>` are supported.
- **IPC:** typed commands `get_config`, `get_config_status` and `get_app_info`, with the TypeScript wrappers in `app/src/ipc.ts`.
- **Placeholder window** showing the config file, port and diagnostics. The real UI is Phase 5.

### Phase 2: Audio engine

- **`crates/deej-audio`:**
  - `model`: plain endpoint, session, command and event types, plus the `AudioControl` trait.
  - `fake`: an in-memory backend for testing the Phase 4 logic.
  - The Windows engine.
- **Engine thread:** one dedicated MTA thread owns every Core Audio object. Commands and Windows notifications share one queue, and callbacks only post messages (D1).
- **Coverage:**
  - Endpoints: all active devices, both playback and recording, with friendly name, default flag (console role), volume and mute.
  - Sessions: on every playback device, with stable ids, the Go deej's key rules (`system`, lowercased exe name), PID, exe path, state, volume and mute.
  - Commands: set volume (single or batch), set mute, rescan, snapshot, reconcile interval. `EndpointRef::Default` resolves at execution time, and unchanged volumes are skipped (A12).
  - Notifications: device added, removed, state change and rename; default device changed; endpoint volume; session created; session volume, state and disconnect.
  - Recovery: device gone → rescan; audio service gone → drop everything and retry every 5 s; a 10 s reconcile catches missed notifications.
- **`examples/audio_probe.rs`:** a read-only tool that prints what the engine sees and every change.
- **Bug found by the tests and fixed:** late notifications of deej's own changes overwrote newer values (D13).

### Phase 3: Hardware engine

- **`deej-core::protocol`:** line parsing, normalization, the noise filter and `SliderState` (resends), ported literally from Go and checked bit-for-bit against `tools/golden-gen/protocol`:
  - all 1024 raw values, plain and inverted
  - every noise-filter decision between all 143 reachable positions, for each level
  - 12 line sequences
- **Line splitting:** `LineSplitter` caps lines at 4 KiB. `CountGuard` requires a new slider count to be confirmed by the next line (D14).
- **`crates/deej-serial`:** one connection thread per board.
  - The port is opened 8N1 with DTR and RTS set explicitly.
  - It reconnects when the port is missing, busy or lost, and tells busy from missing using the port list.
  - A silence watchdog catches unplugs that drivers don't report.
  - It applies settings, options and resend commands, and keeps diagnostics: line counts, unconfirmed lines, the last 20 raw lines, last error and connect count.
  - It never writes to the board.
- **`examples/board_probe.rs`:** a read-only tool that shows ports, status, moves and diagnostics, with `--no-dtr` and `--rts`.
- **On the real board:** an Arduino Uno R3 on COM3 with 3 sliders streaming about 60 lines/s. Its garbled startup lines led to D14.

### Phase 4: Mapping and compatibility

- **`deej-core::mapping`:** target resolution with the original rules (`master`, `mic`, `system`, exe names, device names, `deej.current`, `deej.unmapped` + exclusions, groups), plus new-target detection.
- **Sensitivity curve:** bit-exact against Go on 5,148 cases.
- **Config writer (`config::writer`):** text-level edits that keep comments, order, quoting, CRLF and BOM. It re-parses to verify before writing, and refuses an edit that would change anything else. Tested on every fixture and, read-only, on the owner's own config.
- **Config store:** atomic writes, one backup per run in `logs/config-backups` (the last 5 are kept), and own-write detection.
- **`crates/deej-platform`:**
  - process names, shared with `deej-audio`
  - the foreground app for `deej.current`: owner process plus child-window processes, cached 350 ms
- **Coordinator (`app/src-tauri/src/coordinator.rs`):**
  - slider moves → resolved targets → curved volumes → audio engine
  - new apps and a changed default device get their slider's volume immediately
  - external changes are tracked
  - config hot reload (debounced; own writes ignored; a broken file keeps the previous settings)
  - UI edits are applied at once and saved 400 ms later
- **Services:** start the audio engine, serial connection, config watcher and coordinator at launch, and stop them cleanly on exit, saving pending edits first.
- **Tauri commands:** `get_state`, `rescan`, `reconnect_hardware`. UI events go out on `deej://event`.

### Phase 5: User interface

- **Backend commands:** state, hardware info (ports and diagnostics), curve presets, app icons (exe icon → PNG data URL, cached), volume and mute, slider targets, sensitivity, allow-listed settings, exclusions, rescan, reconnect, open config/logs, UI preferences, and UI error/ready reporting.
- **The coordinator publishes:**
  - what each slider controls (for the UI's slider badges)
  - a `configChanged` event after any change
- **UI preferences** live in `deej-next-ui.json` next to the config, never in `config.yaml`.
- **Frontend** (React 19 + TypeScript + Tailwind 4), Windows 11 / Fluent style, dark by default, light or follow-Windows:
  - **Mixer:** master and microphone, apps grouped by executable with icons, volume, mute, "Slider N" badges, and a glow when the controlling hardware slider moves. Other devices are listed too, and Windows changes show live.
  - **Mappings:** per-slider cards with live position and resulting volume, target chips, a searchable picker (special targets, running apps, devices, or any typed exe name), and a per-slider response curve with a graph. Edits save automatically.
  - **Hardware:** connection status, port picker with USB ids, baud rate, reconnect, live slider meters, and diagnostics including the latest raw lines.
  - **Settings:** General, Hardware, Audio (deej.unmapped exclusions), Slider mappings (default curve), Appearance, Advanced (DTR/RTS/reconnect interval), Diagnostics (config notes with line numbers, status, logs).
- **Dev-only mock backend** for the browser preview.

### Owner feedback after Phase 5

- **The live response curve is back** (commit e65c478), as in the fork's settings window. Every slider card shows its curve with the live position as a gliding dot, plus a "position → volume" readout. The editor uses range sliders and a large live graph.
- **The Windows-style mixer is removed (D22).** The Sliders page is the home page, and each target chip shows what it's doing now ("Discord 83%", "All other apps · 3 apps"). Hardware keeps connection and diagnostics only. The UI's volume and mute commands are removed.

### Phase 6: Windows integration

- **Tray** (`tray.rs`): left-click opens the window; menu with the board status, Open deej, Mute playback, Playback device ▸, Restart board connection, Re-scan audio, Edit config.yaml, Start with Windows, version and Quit deej. A `deej-tray` thread refreshes it twice a second and only touches the tray when something visible changed. The tooltip is the fork's `buildTooltip`, ported with its tests.
- **Window lifecycle** (`window.rs`): no window in the config; it's created on demand and destroyed on close (WebView memory released). Position, size and maximised state are restored, but only onto a monitor that's still there. `--minimized` starts in the tray.
- **Single instance:** a second launch opens the running copy's window and exits. A named mutex keeps it from touching the running copy's log files (D25; this was a bug found in the live test).
- **Start with Windows** (`deej-platform::autostart`): own `deej-next` Run value with `--minimized`; Startup shortcuts are resolved through `IShellLink`; only entries for this exe count. Other deej entries show as a warning in Settings with an "Open Startup folder" button (D23). Corrected at startup if the exe moved.
- **Sleep/resume** (`deej-platform::power`): `PowerRegisterSuspendResumeNotification` callback → the coordinator reconnects the board and re-scans audio 3 s after waking.
- **Default playback device** (`deej-audio`, `IPolicyConfig`): set for all three roles; the engine's normal default-device notification updates everything, and the master slider's volume follows to the new device.
- **Notifications:** registered AppUserModelID so they show "deej" even unpackaged; board outages (once, after 5 s), config errors and reloads while the window is closed, a one-time "still running in the tray" hint. Routine ones can be turned off (D26).
- **`DEEJ_NO_TRAY_ICON`:** no tray; closing the window quits.
- **Settings → General:** Start with Windows, the other-deej warning, what closing the window does, routine notifications.

### Phase 7: Testing and optimisation (shortened)

- **9-hour soak**, window closed, real board with `noise_reduction: low` jitter: average CPU 0.41 % of one core (target < 0.5 %), handles flat at 247, threads flat.
- **CPU fix:** the serial thread woke several times per line because USB serial delivers a line in pieces. It now waits 5 ms after a read that ends mid-line: serial thread about 1.2 % → 0.2–0.3 %.
- **Latency logging:** with `--verbose`, "Slider moved" has `since_line_us` and the audio engine logs `queued_us`/`apply_us` per volume command. The p95 hasn't been computed yet.
- **Memory growth found, not fixed:** see Known issues.
- `deej-audio` `leak_probe` example: re-scans every 100 ms and 50 volume sets/s stayed flat at 1.7 MB.

### Phase 8: Release

- NSIS installer (per-user, no admin, `%LOCALAPPDATA%deej`), portable ZIP (exe + default `config.yaml` + README), release notes and SHA-256 checksums.
- User README: install, moving over from the original deej, troubleshooting.
- The version string now follows new commits (the build script watched only `.git/HEAD`).

## Outstanding work

Phases 2–8 per [ARCHITECTURE.md §8](docs/ARCHITECTURE.md#8-implementation-plan). Config-related items left for Phase 4: the file watcher (hot reload), the surgical writer, and backups.

## Known issues

- **Fixed in 0.1.1:** any copy that started took over Start with Windows (the ported "deej was moved" fix re-pointed the `deej-next` Run value to whichever exe started; it happened on 2026-10-13 with the smoke-test copy). The value is now re-pointed only when the program it names no longer exists (`autostart::should_refresh`, unit-tested; checked live: a second copy left the installed copy's entry alone).
- **Unexplained:** after installing, the installed copy logged "Start with Windows changed enabled=true" for its own path, but the Run value still named the smoke-test copy. Calling the same `autostart::set` from the `autostart_set` example then wrote it correctly. Not reproduced yet.

- **Fixed after 0.1.1:** memory grew by about 2.3 MB per hour while idle (8 → 32 MB over 9 hours). It came from the tray tooltip, which was updated up to twice a second while slider jitter changed the volumes it listed; with `DEEJ_NO_TRAY_ICON` memory stayed flat. The owner had the per-slider tooltip removed (D27); it's now just "deej". Checked: 15 minutes with the tray on, the real board and the owner's config: 8.03 → 7.96 MB (the old build grew about 0.6 MB in that time).
- Not measured: slider → volume latency p95, AudioSrv restart, device disable, unplug during a run, Explorer restart, clean-VM install.

- The Phase 1 window is a placeholder. It doesn't follow the light/dark setting yet.
- The installer bundle (`npx tauri build` without `--no-bundle`) hasn't been tried yet. That's planned for Phase 8.
- Generated TypeScript types (`ts-rs`/`specta`) aren't set up yet. The two types in `ipc.ts` are hand-written; this is to be decided when the IPC surface grows in Phase 4/5.

### Bugs found in the original code (not replicated)

- **Fork:** rejecting a line with a value > 1023 leaves earlier sliders' positions updated but drops their events. `invert_sliders: yes`/`on` reads as false. Duplicate keys make the config fail to load.
- **Upstream:** never reconnects after an unplug; ignores LF-only lines; values > 1023 after the first field produce volumes > 1.0.
- **Both:** non-integer slider keys silently map to slider 0; a stale `master` after a default-device change drops one slider move; `recoverFromPanic` is never called.

## Tests completed

| Suite | Count | Result |
|---|---|---|
| `deej-core` unit tests (yaml tree, value resolution, config, sensitivity, location) | 22 | pass |
| `deej-core` golden tests (52 fixtures vs upstream and fork) | 3 | pass |
| `deej-next` app unit tests (log rotation, crash log, default config creation, invalid config fallback, preferences merge, args) | 6 | pass |
| `cargo clippy --workspace --all-targets -D warnings`, `cargo fmt --check`, `tsc -b` | — | clean |
| Manual: debug exe run from a scratch folder | — | Window opened; config loaded; diagnostics logged with line numbers; UI → backend IPC call logged; 36 MB working set (host process only) |
| `deej-audio` unit tests (model, fake backend) | 6 | pass |
| `deej-audio` real-device integration tests (`--ignored`): devices and sessions found, endpoint volume and mute set and restored, default-device target resolution, external change flagged, new app session found by notification (rescan disabled) and dropped on exit, start/stop 5× | 6 | pass, 3 consecutive runs |
| Mutation check: session-created notification disabled → discovery test fails | — | confirmed |
| Engine idle, 30 s, debug build | — | 0.10 % of one core, 1.8 MB private, 7 threads, clean exit |
| `deej-core` protocol unit tests + golden protocol tests | 8 + 3 | pass; mutation check (round instead of floor) caught |
| `deej-serial` fake-board tests (connect, split reads, retry until present, busy, lost then reconnect with resend, silent unplug, settings and options changes, diagnostics, garbled lines, prompt stop) + port sorting | 12 | pass, 3 consecutive runs |
| Real board, COM3: DTR on (default), DTR off, RTS on | — | Connects; 3 sliders; DTR on → 1.5 s reset delay; DTR off → garbled lines are no longer applied |
| `deej-core` mapping (9), sensitivity golden (1), writer (14), store (4) | 28 | pass; every fixture edits cleanly |
| Coordinator with fakes: moves → volumes with curve, unmapped/current/groups, new app gets its slider's volume, new default device, external changes, file reload, broken file, UI edits merged and saved without self-reload, refused edit, hardware status | 11 | pass |
| **End to end** on the owner's PC: real board on COM3, the owner's config, real apps | — | `deej.unmapped` hit steam/javaw/benilla but not the excluded mpvnet, mapped Discord, or system; Discord followed slider 2 through curve 2 (91 % → 82.8 %, confirmed in Windows); live reload, broken file kept the previous settings, recovery; graceful shutdown; 12 MB private memory |
| `deej-platform` icon extraction (explorer.exe → PNG), PNG/CRC/Adler/base64 | 3 | pass |
| UI prefs file, JSON shape contract, slider targets published | 3 | pass |
| UI against the mock in the browser pane: all four pages, dark and light, the picker adds Spotify / chip removes Brave (exact `set_slider_targets` calls), curve preset → `set_sensitivity` 2.5, volume drag → `set_volume` (frame-throttled), theme saved | — | pass, no console errors |
| Real window, owner's PC, real board and config | — | "UI rendered page=mixer endpoints=6 app_rows=6 sliders=3", no UI errors |
| Production bundle excludes the mock | — | verified |
| Tray tooltip (the fork's 3 tests + names, volumes, unknown positions), outage notifications, window placement, prefs merging, args, coordinator mute/default device/resume | 13 | pass |
| `deej-platform` autostart parsing and status, instance lock | 3 | pass |
| Release exe on the owner's PC: `--minimized` start | — | no window, board on COM3, tooltip "deej / unmapped 30% / brave 2.3% / discord 100%", 31 MB working set |
| Release exe: second launch, close the window, reopen | — | second copy exits (code 0); first opens its window and renders; close → still running, position saved and restored (683,360); log intact after the D25 fix |
| Autostart detection against the real Startup folder | — | the Go deej's `deej.exe.lnk` resolved to `C:\deej\deej.exe` and reported as another deej |
| UI against the mock: Start with Windows toggle, other-deej warning, routine-notification toggle (partial prefs save keeps the theme) | — | pass |
| Owner, hands-on: tray Mute playback, Playback device switch and back, Restart board connection, Re-scan audio, sleep and wake | — | "All good" (2026-10-12) |
| Owner: reboot with Start with Windows on (installed 0.1.2) | — | deej started in the tray (2026-10-13) |
| Total automated | 121 + 6 hardware-only | pass |

## Tests outstanding

Everything in [COMPATIBILITY_MATRIX.md](docs/COMPATIBILITY_MATRIX.md) not marked as implemented, plus the performance budget in [ARCHITECTURE.md §6](docs/ARCHITECTURE.md#6-performance-budget).

### Environment limits for testing

- The sandboxed shell can't see audio devices or open GUI windows, so app and audio runs happen unsandboxed. The machine has 6 active audio endpoints.
- COM3 (the real board, "USB Serial Device") is used by the installed deej. There's a standing OK to stop it for hardware tests; it gets restarted afterwards.
- Screenshots can't capture WebView2 content. UI checks use the log and IPC traces, or a mock preview page in the browser pane.
- No Windows 10 VM is known to be available.

## Architectural decisions

D1–D12 in [ARCHITECTURE.md §9](docs/ARCHITECTURE.md#9-decisions-recorded-so-far).

### Not yet verified in Phase 2

- A real audio service restart (`Restart-Service audiosrv` needs an elevated shell).
- A default-device switch and unplugging a device. Both are handled in code; they need a person at the machine.
- No audio devices at all (Remote Desktop).
- The mic level is never changed by the tests, deliberately.

### Not yet verified

- An unplug and replug of the real board while deej Next runs (covered by fake-board tests only).
- Sleep and resume (implemented in Phase 6; needs a person to put the PC to sleep).
- An Explorer restart (does the tray icon come back? R12). (A reboot with Start with Windows on was checked by the owner on 2026-10-13 with 0.1.2: deej started in the tray.)
- Boards other than the owner's Uno.
- Phase 2's manual audio checks: default-device switch, audio service restart, unplugging an audio device.

### Observation from the end-to-end run

With the owner's `noise_reduction: low`, sliders 0 and 2 jitter by ±2 % (833 volume updates in about 70 s while untouched). The Go deej behaves identically with this setting. `default` (3 % steps) would stop it; that's the owner's choice to make.

### Known limits after Phase 5

- The real WebView2 window can't be screenshotted or driven by the tools available here. Its rendering was checked through the mock and the render report; the owner's own look at it is the real visual check.
- (Resolved in Phase 6: closing the window now keeps deej in the tray; Start with Windows is in Settings → General. A separate "start minimised" setting wasn't needed: autostart always starts in the tray, a manual start opens the window.)

## Next steps

1. Remaining Phase 6 checks that fit into normal use: an Explorer restart, a reboot with Start with Windows on.
2. Phase 7, testing and optimisation: 24 h soak with the window closed, CPU and memory measurements, leak check, fault injection (unplug, audio service restart, device disable).
