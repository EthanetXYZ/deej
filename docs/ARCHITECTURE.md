# deej Next — architecture

**Status: approved 2026-10-12. Phases 1–5 (foundation, audio engine, hardware engine, mapping and compatibility, user interface) are implemented**; later phases are still design. Statements about third-party crates are marked **(to verify in Phase 1)** where I haven't confirmed them against current documentation or source.

## 1. Goals that drive the design

1. **Bit-for-bit behavioural compatibility** with deej's serial processing and target semantics ([COMPATIBILITY_MATRIX.md](COMPATIBILITY_MATRIX.md)).
2. **COM safety by construction.** Every crash this fork has fixed came from COM objects being used and released on different threads. In the new design, exactly one thread owns all Core Audio objects.
3. **Event-driven, with reconciliation as a fallback.** Session and volume notifications replace upstream's poll-on-slider-move, and a slow periodic reconcile catches anything missed.
4. **A headless core.** Hardware and audio keep running with no window. The WebView exists only while the window is open.
5. **Testable without hardware.** Every platform boundary sits behind a trait with a fake implementation.

## 2. Repository layout

The Rust app is the repository root of the `main` branch (D28). It was developed in a `deej-next/` folder beside the Go code, which stays on `master` as the reference implementation; `tools/golden-gen` still records its behaviour as test vectors.

```
(repository root)
  Cargo.toml                 # workspace
  crates/
    deej-core/               # pure Rust, no Windows deps, builds and tests on any OS
      config/                #   YAML load, coercion (viper-compatible), validation, surgical writer
      protocol/              #   serial line parser, normaliser, noise filter
      mapping/               #   target resolution, sensitivity, unmapped/current logic
      model/                 #   shared types (SliderId, Target, SessionInfo, events, snapshots)
    deej-audio/              # Windows Core Audio engine (windows crate), AudioBackend trait + fake
    deej-serial/             # serial connection manager (serialport crate), PortOpener trait + fake
    deej-platform/           # autostart, power notifications, foreground window, process info, icons
  app/                       # React + TypeScript + Tailwind UI (Vite), standard Tauri layout
    src-tauri/               #   the Tauri 2 application: coordinator, commands/events, tray, window lifecycle
  tools/golden-gen/          # Go program that records the original deej's behaviour (built against upstream's and the fork's dependencies)
  arduino/                   # the board sketch (unchanged from the original deej)
  docs/                      # FAQ and design documents
  testdata/
    golden/                  # what the Go implementation produced for each fixture
    configs/                 # real-world and edge-case config.yaml files
```

The crates `deej-audio`, `deej-serial` and `deej-platform` are created in the phases that need them (2, 3 and 6).

## 3. Runtime architecture

```
                         ┌──────────────────────────────────────────────┐
  serial thread ────────►│                                              │◄──── config watcher (notify)
  (deej-serial)  events  │          Coordinator (tokio task)            │
  Line/Status            │  owns: Config, MappingEngine, SliderState,   │◄──── UI commands (Tauri invoke)
                         │        AudioSnapshot, HardwareStatus         │
  audio thread  ◄────────┤                                              ├────► UI events (Tauri emit, throttled)
  (deej-audio)  commands │  pure decisions: which sessions get which    │
         │      ────────►│  volume; when to reconnect; what to save     ├────► tray (menu + tooltip)
         │      events   └──────────────────────────────────────────────┘
         ▼
  COM callbacks (any MTA thread) ──► post to audio thread queue only
```

### 3.1 Threads and ownership

| Thread | Owns | Blocking? | Notes |
|---|---|---|---|
| **main** | Tauri/tao event loop, tray, windows | event loop | Required by Tauri on Windows |
| **coordinator** (`deej-coordinator`, a plain thread; D16) | all application state | waits on its queue | A single-writer actor. No locks around business state; the UI reads a published copy (`Shared`) |
| **audio** (dedicated OS thread, `CoInitializeEx(MULTITHREADED)` once for its lifetime) | **every** Core Audio COM pointer | waits on its command queue with a timeout | The only thread that calls Core Audio methods. Rebuilds everything on device invalidation or audio service restart |
| **COM notification threads** (owned by Windows) | nothing | must return fast | `IMMNotificationClient`, `IAudioSessionNotification`, `IAudioSessionEvents`, `IAudioEndpointVolumeCallback` implementations only push a small message to the audio queue. They never call back into Core Audio, which avoids the documented callback deadlocks |
| **serial** (dedicated OS thread per controller) | the port handle | blocking reads with a short timeout | Open → read lines → parse → send events. Reconnect loop with back-off. Never touches audio |
| **config watcher** (`notify` crate) | file watch | — | De-bounce 500 ms, ignore self-writes |

This replaces the fork's lock-based rules (`opLock`, `LockOSThread`, enumerator reset) with ownership. A released session can't be used concurrently, because only one thread can touch it.

### 3.2 Audio engine (`deej-audio`)

Interfaces used: `IMMDeviceEnumerator`, `IMMDevice`, `IMMEndpoint`, `IPropertyStore` (friendly name), `IAudioEndpointVolume` + `IAudioEndpointVolumeCallback`, `IAudioSessionManager2` + `IAudioSessionNotification`, `IAudioSessionEnumerator`, `IAudioSessionControl2` + `IAudioSessionEvents`, `ISimpleAudioVolume`, `IMMNotificationClient`.

- **Model:** the engine keeps a table of endpoints (id, friendly name, flow, state, endpoint volume + mute) and sessions (stable internal id, endpoint id, PID, exe name lowercased, display name, icon path, state, volume, mute). The coordinator gets snapshots and deltas, never COM pointers.
- **Discovery:** a full enumeration at start and after any invalidation. Afterwards `OnSessionCreated` triggers a re-enumeration of that device's sessions (diffed by session instance id, so ids stay stable), and `OnStateChanged(Expired)`/`OnSessionDisconnected` remove sessions. An idle session whose process has exited is also dropped, as the Go deej did.
  - The engine registers for session notifications and then enumerates, which satisfies the reported "enumerate once before notifications start" rule. Tested: a new app's session is reported with the periodic rescan disabled, and the test fails when the notification is switched off.
- **Process naming:** a Toolhelp32 snapshot (`szExeFile`), the same source as go-ps, so names match the Go version exactly, including for elevated processes whose path can't be opened. Exe path and icon use `QueryFullProcessImageNameW` when permitted, with a generic icon otherwise.
- **Setting volume:** `SetMasterVolume(v, &DEEJ_EVENT_CONTEXT)` with the original GUID `{1ec920a1-7db8-44ba-9779-e5d28ed9f330}`. Notifications carrying that context are recognised as our own and don't bounce back as "external change".
- **Errors:**
  - `AUDCLNT_E_DEVICE_INVALIDATED` or `E_NOTFOUND` → re-scan devices.
  - `AUDCLNT_E_SERVICE_NOT_RUNNING`, `RPC_E_DISCONNECTED`, `RPC_S_SERVER_UNAVAILABLE` and `RPC_S_CALL_FAILED` → drop everything and retry every 5 s.
  - **Not yet verified:** which HRESULTs a real `AudioSrv` restart produces. That needs an elevated restart of the service, so it's a manual check at the Phase 2 review.
- **Reconciliation:** every 10 s by default (`Command::SetReconcileInterval`; the app will shorten it while the window is open), the engine re-enumerates and reports any volume or session differences a notification missed. Measured on the debug build: 0.10 % of one core and 1.8 MB private memory over 30 s idle.
- **Own-change echoes:** Windows notifies about deej's own volume changes too, and those notifications can arrive late. They're ignored rather than applied, because a late echo of an earlier change could otherwise overwrite a newer value (found by the integration tests; D13).
- **Default device changes:** `OnDefaultDeviceChanged(eRender|eCapture, eConsole)` → re-bind `master`/`mic`, then ask the coordinator to re-apply those sliders (fixes A15).

### 3.3 Serial engine (`deej-serial`)

- `serialport` crate: open with 8N1, no flow control, then **explicitly** set DTR (default on) and RTS (default off, matching upstream). Verified in the library source: `serialport` 4.10 leaves DTR **off** by default, so relying on it would have broken boards that need DTR.
- Busy and missing ports both come back from Windows as `NoDevice`; the engine tells them apart by checking the port list.
- A slider-count change must be confirmed by the next line before it's applied (`CountGuard`, D14).
- If no data arrives for 2 s, the engine checks that the port still exists, because some USB drivers keep timing out reads after an unplug instead of failing them.
- Read loop: 50 ms read timeout, an accumulator capped at 4 KiB, split on `\n`, pass lines to `deej-core::protocol`. The slider state lives on the serial thread, which sends only actual moves (plus all current positions) to the coordinator.
- The connection state machine `Disconnected → Opening → Connected → Lost → (back-off) → Opening` is a pure function in `deej-core`, unit-tested with fake time, and driven by the thread.
- **Diagnostics:** lines/s, invalid lines (with the last 20 raw lines in a ring buffer), last error, reconnect count, detected slider count, USB VID/PID from port enumeration.
- **Multiple controllers:** the thread-per-port design supports it. The config schema is in §5.3, pending your decision.

### 3.4 Core logic (`deej-core`)

- **protocol:** `parse_line(&str) -> Result<Vec<u16>>` (S1–S4); `normalize(raw, invert) -> f32` and `significantly_different(old, new, level)`. These are ported literally, with Go's f32/f64 conversion order kept intact, and tested against golden vectors.
- **SliderState:** per controller, positions with a −1 sentinel and the resend mask (fork semantics: count change, reload, reconnect and sensitivity changes force a resend).
- **mapping:** pure resolution of `targets × sessions snapshot × foreground info × unmapped_exclude → session ids`, then `volume = sensitivity(slider).apply(position)`. `deej.unmapped` is recomputed whenever the mapping or the session list changes (A8).
- **config:** see §4.

### 3.5 Coordinator (app crate)

- Receives `SliderMoved`, `SessionAdded/Removed/VolumeChanged`, `DeviceChanged`, `ConfigReloaded`, `UiCommand`, `PowerResumed`. It updates state and emits `AudioCommand::SetVolumes(Vec<(SessionId, f32)>)`, skipping values equal to the last value set or read (A12).
- **New session for a mapped slider:** apply that slider's current volume right away (A14, pending approval).
- **UI publishing:** only while a window exists. Slider positions are coalesced to ≤ 30 Hz; volumes and sessions are sent as deltas. Snapshots are requested on window creation.
- Owns the debounced config save queue (400 ms, merge by path), ported from the fork.

### 3.6 Windows integration (`deej-platform` + app)

| Feature | Approach |
|---|---|
| Tray | Tauri 2 `TrayIconBuilder` with menu: status, Open deej, Toggle master mute, Playback device ▸ (radio list), Restart hardware connection, Re-scan audio, Edit configuration, Start with Windows ✓, version, Quit. Tooltip via the ported `buildTooltip` |
| Background | `RunEvent::ExitRequested` → `prevent_exit` unless the exit came from the Quit menu. Closing the window **destroys** the WebView to release memory; the tray recreates it on demand (W14) |
| Single instance | `tauri-plugin-single-instance`; a second launch focuses or creates the window (unless it was started with `--minimized`). A named mutex taken before logging keeps the second copy from rotating the first one's log (D25) |
| Autostart | Own implementation (port of the fork's, using the `windows` crate registry API): `HKCU\…\Run\deej-next` with `--minimized`. Startup-folder shortcuts are resolved through `IShellLink`, so only ones that start this exe count (D23). If the exe moved, the value is corrected at startup, like the fork's `RefreshAutostart` |
| Window state | Small JSON (`deej-next-ui.json`) with size, position, maximised and theme, validated against current monitors on restore |
| Sleep/resume | `PowerRegisterSuspendResumeNotification` (callback, no window needed) → on resume: force a serial reconnect and an audio rebuild after a short delay |
| Playback device selection (tray/UI) | **Requires the undocumented `IPolicyConfig` COM interface.** No public API sets the default endpoint. See risk R7 |
| Toasts | `tauri-plugin-notification`. deej registers its AppUserModelID (`com.ethanetxyz.deej`, name and icon) under `HKCU\Software\Classes\AppUserModelId` at startup, so notifications show "deej" without an installer shortcut (R10). Kinds per D26 |
| Logging | `tracing` + `tracing-subscriber` + `tracing-appender`; files `logs/deej-latest-run.log`, `deej-previous-run.log`; panic hook writes `deej-crash.log` |
| Icons | Exe icon via `SHGetFileInfoW`/`ExtractIconExW` → PNG data URL, cached per exe path, extracted on the audio-free platform thread |

### 3.7 Frontend

- React 18 + TypeScript + Tailwind, built with Vite. No UI component library: a small set of in-house Fluent-style components (slider, toggle, select, list row, tabs), plus Fluent System Icons SVGs (MIT) copied inline.
- **Screens:** Sliders (home: mappings, live response curves, what each target is doing) · Hardware · Settings (General, Hardware, Audio, Slider mappings, Appearance, Advanced, Diagnostics). There is no Windows-style mixer (D22).
- **State:** one store fed by Tauri events. Commands are typed wrappers around `invoke`. Local edits beat polled values for a hold window (the fork's `HOLD_MS` idea). Physical slider movement highlights the matching row.
- **Theming:** CSS variables; dark by default; light and system options. Mica/Acrylic window backdrop is optional (R11).
- The TypeScript types for commands and events are hand-written in `app/src/ipc.ts`. A Rust test (`json_shapes_match_what_the_ui_expects`) pins every JSON field name the UI reads, so the contract can't drift unnoticed (D19).
- In a browser (`npm run dev`) a mock backend (`app/src/mock.ts`) stands in for Tauri. It's loaded only in development builds, never bundled into a release (D20), and used to check the UI visually, since WebView2 windows can't be screenshotted.

## 4. Configuration manager

1. **Load:** find the file (C1/C2) → parse with a YAML 1.2 parser that reports source positions → run a **viper-compatibility coercion layer** that reproduces every row of the probe appendix (YAML 1.1 bools for bool keys only, last-wins duplicates, scalar→string, `Atoi`-like index rules) → validate → produce `Config` + a `Vec<Diagnostic>` (unknown keys, ignored values, legacy keys).
2. **Write: surgical text edits, not re-serialisation.** Using the parser's source spans, replace only the value text of edited keys, insert new keys at the end of their mapping with matching indentation, and remove keys including their own line(s). Everything else stays byte-identical, including comments, blank lines, quotes, line endings and BOM.
   - Restricted to the paths deej edits: `slider_mapping.*`, `invert_sliders`, `com_port`, `baud_rate`, `noise_reduction`, `slider_sensitivity.*`, `unmapped_exclude`, `deej_next.*`.
   - Every write re-parses the result and asserts that the semantic diff equals the intended edits before the atomic rename. Otherwise it aborts and leaves the original untouched.
3. **Backups:** before the first write in each run, `logs/config-backups/config-<UTC timestamp>.yaml`; the last 5 are kept. Writes go to a temp file that is renamed over `config.yaml`. The file watcher ignores a change if the file holds exactly what deej last wrote.
4. **Parser crate: `saphyr-parser` 0.2** (decision D9, chosen after a spike). Its event API gives exact start and end positions for every scalar and collection, including quotes. Duplicate keys don't raise an error at the event level, so deej keeps all of them and applies last-wins itself. It doesn't strip a UTF-8 BOM, so the loader does that first. `serde_yaml` (archived) and `serde_yml` (deprecated) were rejected, and `yaml-rust2`'s spans only have start positions.

## 5. Data contracts

### 5.1 Tauri commands (initial set)

`get_snapshot`, `set_master_volume`, `set_master_mute`, `set_session_volume`, `set_session_mute`, `set_default_playback_device`, `set_slider_targets`, `set_sensitivity`, `set_option`, `set_unmapped_exclude`, `reconnect_hardware`, `rescan_audio`, `open_config_in_editor`, `open_logs_folder`, `get_diagnostics`, `export_diagnostics`.

### 5.2 Events

`snapshot`, `sliders` (≤ 30 Hz), `audio-delta`, `hardware-status`, `config-changed`, `diagnostic`, `toast`.

### 5.3 Multiple controllers (proposal, opt-in)

The legacy single-board keys stay as they are and describe controller "0". Extra boards would be declared under `deej_next.controllers` with their own port, baud, invert and mapping offset (e.g. board 2's slider 0 = global slider 5), so `slider_mapping` keeps one flat index space. **Needs your decision** on whether this is in scope for v1.

## 6. Performance budget

| Metric | Target | How measured |
|---|---|---|
| Idle CPU, window closed, board streaming at 100 lines/s | < 0.5 % of one core | Process Explorer / `GetProcessTimes` over 10 min |
| Memory, window closed | ≤ 25 MB private bytes | Task Manager / `GetProcessMemoryInfo` |
| Memory, window open | WebView2-bound; measured and reported | same |
| Slider → volume latency | < 15 ms p95 from line received to `SetMasterVolume` returning | tracing spans |
| Redundant COM calls | 0 when sliders are idle | counter in diagnostics |

## 7. Technical risks

| # | Risk | Area | Likelihood/impact | Mitigation |
|---|---|---|---|---|
| R1 | ~~Rust toolchain not installed~~ **Resolved:** Rust 1.99.0 stable (MSVC) installed 2026-10-12 with permission | build | — | — |
| R2 | COM callback re-entrancy and deadlocks (calling Core Audio or unregistering from inside a callback) | Core Audio / COM threading | medium / hangs | Callbacks only enqueue messages; unregister happens on the audio thread; the `windows` crate `#[implement]` macro gives correct `AddRef`/`Release`, unlike the fork's no-op vtable |
| R3 | Session notifications missed or never firing (the enumerator-first rule, sessions created on endpoints that appear later) | Core Audio | medium / stale lists | Register per endpoint on `OnDeviceAdded`/state change; 10 s reconciliation; integration test that launches a tone-playing child process |
| R4 | No mature comment-preserving YAML editor in Rust | YAML | high / lost user comments | Surgical span-based editor with re-parse verification and backups (§4). Fork's writer tests ported as fixtures. Spike first in Phase 1 |
| R5 | YAML 1.1 vs 1.2 differences (`yes/no/on/off`, duplicate keys, octal-looking numbers like `010`) | YAML | medium / silent misconfiguration | Coercion layer driven by the probe table; golden tests against both viper versions for every fixture |
| R6 | **DTR/RTS defaults differ by library; native-USB boards need DTR; ESP32 auto-reset circuits react to RTS+DTR.** | serial | **Mitigated** | Both lines are set explicitly and configurable (`deej_next.serial`). Measured on the owner's board (Arduino Uno R3, USB 2341:0043, which shows up as "USB Serial Device"): DTR on resets it and data starts 1.5 s after opening (same as the Go deej); DTR off means no reset, but a burst of garbled lines (handled by D14); RTS makes no difference |
| R7 | Default playback device switching needs the undocumented `IPolicyConfig` interface (stable in practice since Windows 7, but unsupported) | Core Audio | low–medium / feature breaks on some Windows build | Isolated module with feature detection; failure disables the menu item instead of crashing |
| R8 | WebView2 memory (tens of MB) vs the "low memory" goal | Tauri background | certain while open | Destroy the window on close; the backend never depends on the WebView |
| R9 | Tauri exits when the last window closes; tray events and menus run on the main thread | Tauri background / tray | low with correct setup | `prevent_exit`; keep handlers non-blocking (send to the coordinator) |
| R10 | Unpackaged/portable apps can show toasts with a wrong or missing app name (AppUserModelID) | Windows tray/notifications | medium / cosmetic | Set an AUMID at startup and register it in HKCU (the installer does this too); verify on both the portable ZIP and the installer |
| R11 | Mica/acrylic backdrops vary across Windows 10/11 builds | UI | low / cosmetic | Optional; fall back to a solid background |
| R12 | Tray icon hidden in the overflow on Windows 11; Explorer restart drops the icon | tray | medium / UX | Tauri re-creates the icon on `TaskbarCreated` (to verify in Phase 6); first-run hint to pin it |
| R13 | Elevated or protected processes: `OpenProcess` fails, so no exe path or icon | Core Audio / processes | certain for some apps | Toolhelp name (as in Go) + generic icon |
| R14 | Audio integration tests need a real audio device: CI and RDP have none; the sandboxed shell can't see devices | testing | certain | Fake backend for logic tests; real-device tests tagged `#[ignore]` and run locally unsandboxed; documented in DEVELOPMENT_STATUS |
| R15 | COM3 is held by the installed deej at `C:\deej` | hardware testing | certain | Hardware tests need you to quit the installed deej first, or use a second board. **I won't stop it without asking** |
| R16 | Sleep/resume: USB-serial handles become invalid; audio endpoints may be re-created | lifecycle | medium | Resume hook + reconnect state machine + device-invalidated handling; manual sleep test |
| R17 | Mute semantics vs sliders (should moving a slider unmute?) | UX | low | Proposal: sliders never touch mute (A18); a mute badge is shown on the slider row |

## 8. Implementation plan

Each phase ends with: everything compiles, all tests pass, DEVELOPMENT_STATUS.md is updated, and a separate commit on branch `deej-next`. I'll **stop for your review at the end of Phase 2 (audio) and Phase 3 (hardware)**, because those need real-device checks.

| Phase | Deliverables | Exit criteria |
|---|---|---|
| **1 Foundation** | Install Rust (with permission). Workspace + Tauri 2 + React/TS/Tailwind skeleton. `tracing` logging. Error types. **YAML spike → parser choice.** `deej-core::config` loader with viper-compatibility coercion and diagnostics. Golden-vector generator (Go program in `testdata/golden/gen/`) | `cargo test` green; all 26 probe configs + upstream/fork defaults load with the expected values; `pnpm build`/`npm run build` and `cargo tauri build --debug` succeed; empty window opens |
| **2 Audio engine** | `deej-audio`: audio thread, enumeration, endpoint + session volume/mute, all four notification types, invalidation recovery, reconciliation, fake backend. Diagnostics CLI (`deej-next-audio-probe`) to exercise it | Unit tests on the fake backend; `#[ignore]` integration tests run locally: set/get master, mic and app volume (test tone child process), external change notification, default-device change, session appears/disappears. **Review stop** |
| **3 Hardware engine** | `deej-serial` + `deej-core::protocol`: parser, normaliser, noise filter, state machine, DTR/RTS control, diagnostics | Golden vectors pass bit-exact; mock-port tests for reconnect/garbage/long lines; **real-board test on COM3** (your go-ahead needed to stop the installed deej). **Review stop** |
| **4 Mapping and compatibility** | Coordinator; all targets (`master`, `mic`, `system`, exe, device, `deej.current`, `deej.unmapped` + exclude); sensitivity; resend rules; config hot reload; surgical writer + backups | Every row of COMPATIBILITY_MATRIX §1–3 has a passing test or a documented manual check; side-by-side run against the Go deej with the same config on a scratch port |
| **5 User interface** | Mixer, Hardware, Mappings editor, Settings (7 sections), themes, live slider highlighting | Manual UI checklist; mock-backend preview for screenshots; keyboard navigation and DPI scaling checked |
| **6 Windows integration** | Tray menu, autostart (shortcut-aware), single instance, window lifecycle, window state, sleep/resume, default-device selection, toasts | Manual checklist incl. reboot autostart, second-launch, sleep/resume, Explorer restart |
| **7 Testing and optimisation** | 24 h soak (window closed), CPU/memory measurement, leak check (handle count + private bytes over time), fault injection (unplug, AudioSrv restart, device disable) | Budgets in §6 met or deviations documented |
| **8 Release** | NSIS installer + portable ZIP + exe (Tauri bundler), README, install guide, troubleshooting, config migration guide, checksums | Clean-VM install/uninstall test on Windows 10 and 11 (or documented if no VM is available) |

## 9. Decisions recorded so far

| ID | Decision | Rationale |
|---|---|---|
| D1 | One thread owns all COM objects; callbacks only enqueue | Eliminates the fork's crash class by construction |
| D2 | Pure `deej-core` crate with no Windows dependencies | Fast, deterministic tests; golden-vector parity with Go |
| D3 | Port numerics literally (f32/f64 order) and test against Go-generated vectors | "Do not assume": compatibility is proven, not argued |
| D4 | Surgical span-based config writer + verify-after-write + backups | Preserves comments without a lossless YAML library |
| D5 | UI-only preferences in a separate JSON; behavioural additions under `deej_next:` | Go deej keeps reading the same `config.yaml` unchanged |
| D6 | Destroy the WebView on close | Meets the low-memory background goal |
| D7 | Own autostart implementation instead of `tauri-plugin-autostart` | Must keep the fork's Startup-shortcut awareness |
| D8 | Rust app in `deej-next/` on a `deej-next` branch; Go code untouched | The Go code remains the reference and the vector generator |
| D9 | YAML via `saphyr-parser` events into deej's own tree (`deej-core::yaml`), with viper/yaml.v2/cast rules re-implemented on top (`config::resolve`) | Exact spans for the writer; full control over YAML 1.1 coercion |
| D10 | Config behaviour is proven against `tools/golden-gen` output from both Go dependency sets, with deliberate differences listed in the test | Turns "compatible" into a checked claim; 52 fixtures today |
| D11 | Where the two Go versions disagree on a value that only matters in edge cases, upstream's behaviour is used (`invert_sliders: 1.0` → false, `baud_rate: "115200.0"` → falls back to 9600 with a warning) | One consistent rule; every case shows a diagnostic |
| D23 | Start with Windows uses its own Run value, `deej-next` (with `--minimized`), and only counts or removes entries that start *this* executable. Entries that start another deej (the Go deej's `deej` value or a `deej*.lnk` shortcut) are listed as a warning in Settings and left alone | The two programs live side by side during the switch; deej Next must not silently disable or overwrite the original's autostart |
| D24 | The window is created in code on demand (none in `tauri.conf.json`) and destroyed on close. Its geometry is saved in `deej-next-ui.json` in physical pixels and only restored if at least 160×80 px of it lands on a connected monitor | Implements D6; a laptop undocked from its second screen still gets a visible window |
| D25 | A named mutex (`Local\com.ethanetxyz.deej`) is taken before logging starts. A second copy doesn't rotate the log files; it only hands its launch to the running copy (Tauri's single-instance plugin) | Found in testing: the second copy rotated the running copy's log away before the plugin made it exit |
| D26 | Notifications are either "problem" (board gone for 5 s or more, broken config.yaml: always shown) or "routine" (reconnected, reloaded, the one-time tray hint: can be turned off in Settings). With the window open, config events use the in-app toasts instead | W4's "setting to turn non-error toasts off"; the 5 s grace keeps USB re-enumeration at sign-in quiet |
| D28 | Owner decision 2026-10-13: the Rust version is published as the new default branch `main` of EthanetXYZ/deej, starting from a fresh history with only its own files (no Go code), as version 1.0.0. The Go fork stays on `master` | deej Next becomes "the new deej" without mixing the two code bases |
| D27 | Owner decision 2026-10-13: the tray tooltip is just "deej"; the fork's per-slider volume tooltip (W3) is removed | Updating the tooltip up to twice a second leaked memory (about 2.3 MB per hour, from `tray-icon`'s tooltip update path), and the owner doesn't need the tooltip |
| D22 | Owner decision 2026-10-12: no Windows-style mixer page and no UI volume/mute controls. The UI focuses on the hardware: sliders, mappings, response curves and the board. Each slider's target chips show the target's current volume (or how many apps it covers) instead | Changes the original brief's "main mixer" screen and A18 mute buttons; master mute stays planned for the tray (Phase 6) |
| D19 | Hand-written TypeScript IPC types plus a Rust JSON-shape contract test, instead of `ts-rs`/`specta` | About 20 commands; no extra dependency or build step; the test fails on any rename |
| D20 | Dev-only mock backend for the UI | Enables visual checks in the browser pane and UI work without hardware; excluded from release bundles (verified) |
| D21 | App icons are extracted from the executable (SHGetFileInfo) and encoded by a 60-line PNG encoder (stored deflate) | Avoids an image dependency for 4 KB icons |
| D16 | The coordinator is a plain thread with a channel, not a tokio task | Every engine is synchronous; a thread keeps the design simple and testable (the tests drive `Coordinator::handle` directly) |
| D17 | UI edits are applied to the file text in memory and re-parsed immediately, then written 400 ms after the last edit | One code path for "what does the config mean", whether it came from the file or the UI; volumes follow a dragged control at once |
| D18 | A user's edit to the file while UI edits are still unsaved wins; the unsaved edits are dropped (logged) | The file is the source of truth, and merging the two could produce something neither intended |
| D14 | A reading with a different slider count is only applied once the next reading confirms it (`CountGuard`) | A real Uno sent valid-looking garbled lines (`99|65|0|88`, `0`) right after the port opened; the Go deej applied them and briefly set wrong volumes. Costs one line (~15 ms) on a real count change |
| D15 | No 64-slider limit (the fork's limit came from its 64-bit resend mask) | Upstream had no limit; per-slider resend flags are a `Vec<bool>` |
| D13 | Volume notifications carrying deej's event context are ignored; the engine records its own changes when it makes them | Late echoes otherwise overwrote newer values (seen in testing: set 75 % then 100 %, read back 75 %) |
| D12 | Owner decisions 2026-10-12: Windows-only (W15); ignore non-integer slider keys (C13); apply a slider's volume when a mapped app starts (A14); UI mute buttons that sliders never change (A18); multiple boards after v1 (S18); standing OK to stop the installed deej to free COM3 for hardware tests, restarting it afterwards (R15) | Recorded so later phases don't re-ask |
