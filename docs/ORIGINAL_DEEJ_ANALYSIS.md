# Original deej — behavioural analysis

Phase 0 deliverable for **deej Next** (the Rust/Tauri rewrite). This document records how deej actually behaves, verified against source code, not documentation.

## Sources examined

| Code base | Location | Notes |
|---|---|---|
| **Upstream deej v0.9.10** | `deej-0.9.10/` (untracked snapshot) | omriharel/deej, the "original". Go 1.14, viper 1.7.1 → yaml.v2, jacobsa/go-serial, getlantern/systray |
| **This fork, v0.10.1+** | `pkg/deej/` (git `master`, 5dca427) | What the maintainer runs today. Go 1.26, viper 1.21 → yaml.v3, go.bug.st/serial, fyne systray, WebView2 settings window |
| Arduino sketch | `arduino/deej-5-sliders-vanilla/*.ino` | Identical in both |
| README, `docs/faq/faq.md`, default `config.yaml` | repo root | User-facing documentation |

"Upstream" and "fork" below refer to those two code bases. Where they differ, both behaviours are recorded, because existing users may run either.

### How claims were verified

- Every Go source file in both trees was read in full (except the embedded icon byte arrays and the Linux PulseAudio backend, which is out of scope for a Windows rewrite and was only skimmed).
- **YAML coercion was tested empirically.** A probe program fed 26 edge-case YAML files through the exact viper getters deej uses, once against upstream's dependency set (viper 1.7.1) and once against the fork's (viper 1.21.0). The results are quoted where relevant and are listed in full in [CONFIGURATION_REFERENCE.md](CONFIGURATION_REFERENCE.md#appendix-verified-yaml-edge-cases).
- **Slider normalization was tested empirically.** `NormalizeScalar` and `SignificantlyDifferent` were copied verbatim into a test program and run over the full input range.
- **Serial line settings were read from the library sources** in the Go module cache (`jacobsa/go-serial` for upstream, `go.bug.st/serial` v1.8.0 for the fork).

Anything I could not verify is marked **(unverified)**.

---

## 1. Program structure (upstream)

```
cmd/main.go        flags (--verbose/-v), logger, NewDeej, Initialize
deej.go            wires Notifier, CanonicalConfig, SerialIO, SessionFinder, sessionMap; run()/stop()
config.go          viper: config.yaml (user) + logs/preferences.yaml (internal), file watcher
slider_map.go      map[int][]string, the slider → target mapping
serial.go          serial open, line reader goroutine, parsing, noise filter, SliderMoveEvent fan-out
session*.go        Session interface, WASAPI implementation, session map and target resolution
tray.go            systray menu
notify.go          toast notifications (beeep)
util/              noise filter maths, foreground-window lookup, open-external
```

Components communicate through **unbuffered Go channels**:

- `config.SubscribeToChanges()` → serial and session map (one `bool` per reload)
- `serial.SubscribeToSliderMoveEvents()` → session map (one `SliderMoveEvent{SliderID, PercentValue}` per changed slider)

A slider event is handled synchronously on the session map's goroutine. The serial goroutine therefore blocks while volumes are being set, which provides natural back-pressure.

### Startup sequence (upstream)

1. Parse flags. Create the logger (release: `logs/deej-latest-run.log`, info level; dev: coloured stderr, debug level).
2. `NewDeej`: notifier → config object → `SerialIO` (subscribes to config reloads) → WASAPI session finder → session map.
3. `Initialize`:
   1. `config.Load()`. If `config.yaml` is missing it shows the toast "Can't find configuration!" and **exits**. Invalid YAML shows "Invalid configuration!" and **exits**.
   2. `sessions.initialize()` does the first full session scan. **Any failure is fatal**, including having no default audio device.
   3. If `DEEJ_NO_TRAY_ICON` is set, deej runs headless and waits for Ctrl+C. Otherwise `systray.Run` takes over the main thread, and `run()` starts from the tray's `onReady`.
4. `run()`: start the config watcher, then attempt the serial connection **once**:
   - port busy (`ErrPermission`): toast "This serial port is busy…", then **quit**
   - port missing (`ErrNotExist`): toast "This serial port doesn't exist…", then **quit**
   - any other error: logged, and deej keeps running **with no serial connection and no retry**

### Shutdown

Quit (tray), SIGINT or SIGTERM → `signalStop()` → `stop()`: stop the file watcher, stop serial, release the session finder (the enumerator), quit the tray, sync the logger, `os.Exit(0)`.

The fork adds: flush pending settings writes first. Crash capture (stderr → `logs/deej-crash.log`) is set up at logger creation.

### Working directory

Upstream resolves `config.yaml` and `logs/` **relative to the current working directory**. Startup-folder shortcuts work only because the shortcut's "Start in" field is set. The fork `chdir`s to the executable's folder when `config.yaml` is there and not in the CWD.

---

## 2. Serial protocol and firmware expectations

### Wire format

- The board sends lines of ASCII decimal integers separated by `|`, one value per slider, terminated by `Serial.println` (**CRLF**).
  Example: `0|240|1023|0|483\r\n`
- The vanilla sketch reads `analogRead` (0–1023, 10-bit ADC) on each pin, sends a line, then waits `delay(10)`. That is roughly 100 lines/s; the exact rate depends on baud rate and line length.
- **One-way only.** deej never writes to the port. There is no handshake, no identification and no versioning.
- The number of sliders is **inferred from the number of values** in each line.

### Line validation

| | Upstream | Fork |
|---|---|---|
| Regex | `^\d{1,4}(\|\d{1,4})*\r\n$` | `^\d{1,4}(\|\d{1,4})*\r?\n$` |
| LF-only lines | **silently ignored** | accepted |
| Value > 1023 | rejected only if it's the **first** value; later values > 1023 pass through and produce volumes > 1.0 (WASAPI then rejects the call, which triggers a forced session refresh) | any value > 1023 rejects the whole line |
| Spaces, signs, empty fields, other text | line ignored | line ignored |

Malformed lines are dropped silently (logged only with `--verbose`). Comments in the code note that the first line after opening the port is often corrupted (for example `4558|925|…`), which is why the > 1023 check exists.

**Fork bug, not to be copied:** when the fork rejects a line because of a value > 1023 at index *k*, sliders 0…k−1 have already had their stored position updated, but their move events are thrown away. Those changes are lost until each slider moves past the noise threshold again.

There is no maximum line length (`bufio.Reader.ReadString('\n')`), so a board that never sends `\n` would make the buffer grow without bound.

### Serial line settings

| Setting | Upstream (`jacobsa/go-serial`) | Fork (`go.bug.st/serial` v1.8.0) |
|---|---|---|
| Baud | `baud_rate` from config | same |
| Framing | 8N1, no flow control | 8N1, no flow control |
| **DTR** | **asserted.** DCB `flags[0] \|= 0x10`; the code comment says "Assert DSR", but bit 0x10 is `fDtrControl = DTR_CONTROL_ENABLE` | **asserted** (`InitialStatusBits == nil` → DTR enable) |
| **RTS** | **not asserted** (`fRtsControl = DISABLE`) | **asserted** (RTS enable) |
| Driver buffers | `SetupComm(64, 64)` | driver default |
| Read timeouts | `MAXDWORD/MAXDWORD/MAXDWORD-1`, i.e. the read blocks until data arrives | library-managed |

Hardware consequences:

- **Asserting DTR resets ATmega328 boards** (Nano/Uno auto-reset through the DTR capacitor) every time the port is opened. The board then runs its bootloader for about 1–2 s before streaming. The partial first line is what the > 1023 check guards against.
- **Native-USB boards** (Pro Micro/Leonardo 32u4, and boards whose USB-CDC stack gates TX on DTR, such as TinyUSB on RP2040/ESP32-S2/S3) may send nothing unless DTR is asserted. **The rewrite must assert DTR.**
- RTS differs between upstream and fork. It matters only for boards with an RTS/DTR auto-reset circuit (typical ESP32 dev boards). See the risks in [ARCHITECTURE.md](ARCHITECTURE.md#7-technical-risks).

### Slider value processing

For every valid line:

1. If the value count differs from the last line, log "Detected sliders", and set every stored position to **−1** (a sentinel meaning "unknown", which guarantees an event for every slider).
2. For each value:
   - `position = floor((raw / 1023.0 as f32) as f64 × 100) / 100`, stored as `f32`. This is **flooring**, not rounding.
   - If `invert_sliders` is set: `position = 1 − position` (f32 arithmetic, so values such as `0.99000001` occur).
   - Emit `SliderMoveEvent{index, position}` if `SignificantlyDifferent(stored, position, noise_reduction)`, and store the new position.

Verified values (from running the original functions):

| raw | position | inverted |
|---:|---:|---:|
| 0–10 | 0.00 | 1.00 |
| 11 | 0.01 | 0.99000001 |
| 102 | 0.09 | 0.90999997 |
| 103 | 0.10 | 0.89999998 |
| 511 | 0.49000001 | 0.50999999 |
| 512 | 0.50 | 0.50 |
| 1013–1022 | 0.99000001 | 0.00999999 |
| **1023 only** | **1.00** | 0.00 |

- There are exactly **101 distinct positions**. 100% needs a raw reading of exactly 1023.
- The `f32` values are passed to WASAPI unchanged. The rewrite must reproduce this arithmetic bit-for-bit, so it will be tested against golden vectors generated from the Go code.

### Noise filter (`SignificantlyDifferent`)

```
threshold = { "high": 0.035, "low": 0.015, anything else: 0.025 }   // case-sensitive match
emit if |old − new| ≥ threshold
or if new ≈ 1.0 and old ≠ 1.0, or new ≈ 0.0 and old ≠ 0.0           // snap to the ends (≈ means |Δ| < 1e-6)
```

Verified: the smallest change that always registers is **2%** (low), **3%** (default) and **4%** (high). There is no time-based smoothing, averaging or hysteresis beyond this threshold against the last *emitted* value.

### Events that re-send all sliders

All of these set stored positions to −1, so the next line re-applies **every** slider's volume:

- slider count changes (including the first line after connecting)
- **config reload** (upstream: `lastKnownNumSliders = 0` after 50 ms; fork: `ResendAllSliders()` after 50 ms)
- fork only: reconnect, sensitivity changes (per slider), "Re-scan"

User-visible consequence: **saving `config.yaml` snaps every mapped app back to its physical slider position**, overwriting volumes changed in the Windows mixer. This is long-standing behaviour.

### Connection lifecycle

| | Upstream | Fork |
|---|---|---|
| Port missing/busy at start | toast, then **quit** | toast once per outage; retries every 2 s |
| Unplug while running | read goroutine exits silently. **deej never reconnects** and doesn't notice (`connected` stays true) | "Lost connection" toast; reconnects every 1 s; "Connected to COMx" toast when back |
| `com_port`/`baud_rate` changed in config | stop, wait 50 ms, start | close; the loop reconnects immediately |
| Status shown to user | none | tray status line, settings window |

---

## 3. Audio (Windows Core Audio)

### Session discovery (`GetAllSessions`)

Each scan:

1. `CoInitializeEx(APARTMENTTHREADED)`; `CoUninitialize` when done. The fork locks the OS thread and tolerates `RPC_E_CHANGED_MODE`.
2. Create the `IMMDeviceEnumerator` (cached).
3. `GetDefaultAudioEndpoint(eRender, **eConsole**)` → **`master`**. `GetDefaultAudioEndpoint(eCapture, eConsole)` → **`mic`**; this may be absent.
4. Register an `IMMNotificationClient` once. Only `OnDefaultDeviceChanged` does anything: it marks the `master`/`mic` sessions **stale**, with 100 ms de-bounce. It ignores the role and data-flow arguments.
5. `EnumAudioEndpoints(eAll, ACTIVE)`. For **every** active endpoint, input and output:
   - **Render endpoints:** `IAudioSessionManager2::GetSessionEnumerator`. For each session: `IAudioSessionControl2`, `GetProcessId`, `ISimpleAudioVolume`.
     - PID 0 → key **`system`** (system sounds). This is also the path for `AUDCLNT_S_NO_CURRENT_PROCESS`; UWP apps hit it too but still return their PID.
     - Otherwise the process name comes from `go-ps` (a Toolhelp snapshot, `szExeFile`), and the key is the **lowercased exe file name** (e.g. `chrome.exe`). If the process has already exited, the session is skipped.
     - **Inactive and expired sessions are included**; the enumerator returns them all.
   - **Every endpoint (render and capture):** an `IAudioEndpointVolume` "device session" keyed by the **lowercased friendly name**, e.g. `speakers (realtek high definition audio)`.
6. Any COM failure in steps 2–5 aborts the **whole** scan (upstream treats this as fatal at startup).

Consequences:

- An app that has sessions on **several render devices** (or several sessions on one device: browsers, games and Discord often do) is stored as **several sessions under one key**. **All of them get set.**
- `system` controls the system-sounds session on **every** render device.
- Per-app sessions on **capture** devices are not enumerated, so per-app microphone levels are not supported.
- Two devices with the same friendly name share a key and are both controlled.

### Session map and target resolution

Map: `key → []Session`. Target strings from the config are lowercased, then:

| Target | Resolves to |
|---|---|
| `master` | default render endpoint's `IAudioEndpointVolume::SetMasterVolumeLevelScalar` |
| `mic` | default capture endpoint's endpoint volume |
| `system` | every PID-0 session |
| `<exe name>` | every session whose process exe name matches, case-insensitively. **Exact match only: `chrome` does not match `chrome.exe`** |
| `<device friendly name>` | that endpoint's volume (Windows only) |
| `deej.current` | the foreground window's process **plus every child window's process whose PID differs** (handles UWP's `ApplicationFrameHost.exe`, Steam and similar). Cached for 350 ms |
| `deej.unmapped` | every session key **not** named by any non-special target in any slider, computed **at scan time**. `master`, `system`, `mic` and anything matching the device-name pattern `^.+ \(.+\)$` always count as mapped. The fork additionally removes `unmapped_exclude` names at resolve time |
| any other `deej.*` | nothing |

### Applying a slider move (`handleSliderMoveEvent`)

1. If the last scan was more than **45 s** ago: **forced** rescan (this is the only way new apps are found while their slider has a target).
2. Look up the slider's targets; an unmapped slider is ignored.
3. Fork: `volume = sensitivity(slider).apply(position)`. Upstream: `volume = position`.
4. For each resolved target and each matching session: **if `GetVolume() != volume` then `SetVolume(volume)`**. This exact float compare is the only redundancy check.
5. No session matched any target → **throttled** rescan (≥ 5 s since the last). A failed set (including a stale `master` or an expired session) → **forced** rescan.

Notes:

- `wcaSession.SetVolume` calls `SetMasterVolume`, **then** checks `GetState()`. If the session is `Expired`, it returns an error to force a rescan, even though the volume call itself succeeded.
- `masterSession.SetVolume` on a stale session **doesn't set anything**. It returns an error, which forces a rescan. **The move that hit the stale session is lost**; the next move applies normally.
- All volume calls pass the event-context GUID `{1ec920a1-7db8-44ba-9779-e5d28ed9f330}`. This identifies deej's own changes in volume notifications, though upstream never subscribes to those notifications.
- **Mute is never touched.** `GetMute`/`SetMute` are a TODO in the `Session` interface.

### Rescan triggers

| Trigger | Kind |
|---|---|
| startup | full scan (fatal on failure upstream; retried later in the fork) |
| config reload | **throttled** (5 s). A mapping change saved within 5 s of the last scan does not recompute the `deej.unmapped` set until a later scan |
| slider move after > 45 s | forced |
| target not found | throttled |
| set failure | forced |
| tray "Re-scan audio sessions" | forced (the fork also re-sends all sliders) |

No session notifications (`IAudioSessionNotification`, `IAudioSessionEvents`) are used at all. Discovery is poll-on-demand.

### COM threading as implemented

- Upstream calls CoInitialize/CoUninitialize on whatever goroutine/OS thread runs the scan, then uses the interface pointers **from other goroutines and threads**. This works in practice because the Core Audio interfaces involved are free-threaded. **(Unverified as a documented guarantee.)**
- Fork fixes, each from a real crash (the fork's maintainer notes, "Stability rules"):
  - one `opLock` serializes "use sessions" with "release and rescan"
  - the thread is locked during the scan
  - the cached enumerator is forgotten after a failed scan
- The `IMMNotificationClient` is a hand-built vtable whose `AddRef`/`Release`/`QueryInterface` are no-ops. It is never unregistered.

---

## 4. Configuration loading

The full key reference, with types and coercion behaviour, is in [CONFIGURATION_REFERENCE.md](CONFIGURATION_REFERENCE.md). Mechanics:

- Viper reads `./config.yaml` (and silently also searches for `config.json`, `config.toml` and other extensions, because only the base name is configured). deej first checks that `config.yaml` itself exists.
- **Undocumented second source:** `./logs/preferences.yaml`. Its `slider_mapping` is **merged** into the user mapping: targets are appended to the same slider index, with duplicates and empty strings dropped. Nothing in deej writes this file. It is a leftover hook from a planned GUI.
- Keys are case-insensitive (viper lowercases them).
- **File watching:** viper watches the config **directory**. It reacts to `Write` events (the fork also reacts to `Create`) on `config.yaml`, ignores repeats within 500 ms, then waits 50 ms and reloads.
  - Success → toast "Configuration reloaded!" and notify subscribers.
  - Failure → toast; the **previous values stay active**.
- The fork's in-app edits write via temp file + rename and then suppress the watcher for 1 s.

---

## 5. Tray, notifications, logging, misc

| | Upstream | Fork |
|---|---|---|
| Tray menu | Edit configuration (`cmd /C start /b notepad.exe config.yaml`), Re-scan audio sessions, version (disabled item), Quit | + status line, Open settings…; left-click opens settings |
| Tooltip | `deej` | `deej` + each slider's volume (Windows 11 truncates at about 64 characters) |
| Toasts | beeep, icon unpacked to `%TEMP%\deej.ico` | same |
| Logs | release: `logs/deej-latest-run.log`; dev: stderr | + `deej-previous-run.log`, `deej-crash.log` (stderr redirect), traceback `all` |
| Panic handler | `recoverFromPanic` writes `logs/deej-crash-<ts>.log` (defined, but not deferred anywhere in the code) | same |
| Flags/env | `--verbose`/`-v`, `DEEJ_NO_TRAY_ICON` | + `--settings` |
| Single instance | **none.** A second instance fails on the busy COM port and quits (upstream) or retries forever (fork) | none |
| Autostart | manual Startup-folder shortcut (README) | + `HKCU\…\Run\deej`, aware of `deej*.lnk` Startup shortcuts; Disable removes both |
| DPI | manifest: PerMonitorV2 | same |
| Sleep/resume | **no handling.** The FAQ lists "After my computer wakes up from sleep deej doesn't work anymore" as a known issue | relies on serial reconnect + 45 s rescan |
| Memory | README: about 10 MB | WebView2 adds roughly 50–100 MB while the settings window is open **(unverified figure)** |

---

## 6. Fork-only features (must also be preserved)

1. **Slider sensitivity:** `volume = max_volume × position^curve`, with a default plus per-slider overrides, curve presets, and live preview in the UI.
2. **`unmapped_exclude`:** process names that `deej.unmapped` skips.
3. **Settings window** (WebView2): sensitivity editing, COM port, invert, noise, autostart, exclusions, live knob positions. **Mappings are not editable there** ("Slider mappings live in config.yaml").
4. **Auto-reconnect** and connection status.
5. **Comment-preserving config writer:** yaml.v3 node tree, CRLF preserved, blank lines re-inserted, temp + rename, edits debounced 400 ms and merged by path.
6. **Crash capture** and log rotation.
7. **Stability fixes:** `opLock`, COM thread locking, enumerator reset, startup without an audio device or a board.

---

## 7. Undocumented or unusual behaviour (checklist)

Each item is verified in code or by probe. The rewrite must decide whether to **keep** or **intentionally change** each one; the decisions are in [COMPATIBILITY_MATRIX.md](COMPATIBILITY_MATRIX.md).

1. **Slider-index keys that aren't integers silently become slider 0.** `strconv.Atoi` errors are ignored. `foo:`, `1.5:` and `-1x:` all map to slider 0 and **overwrite** each other in random map order. `"01"` → slider 1. `-1` → slider −1 (never fires).
2. `slider_mapping: master` (a scalar instead of a map) → an empty mapping, with no error.
3. **YAML 1.1 booleans (upstream):** `invert_sliders: yes`/`on` → **true**; `no`/`off` → false. A mapping `0: no` becomes the target **`"false"`**. In the **fork** (yaml.v3), `yes`/`on` → **false** for `invert_sliders` (cast can't parse them), and `0: no` stays `"no"`. *The fork silently changed this.*
4. **Duplicate keys:** upstream uses the last value; the fork **fails to load** ("mapping key already defined").
5. Scalar targets are never split. `1: chrome.exe firefox.exe` is **one** target with a space in it (matches nothing). However, `unmapped_exclude: a.exe b.exe` (a scalar) **is** split on whitespace, by viper's `GetStringSlice`.
6. Numbers and booleans used as targets become strings: `0: 5` → `"5"`, `0: true` → `"true"`.
7. Empty or null targets (`3:`, `3: ""`, `3: ~`) are dropped. The slider exists but has no targets.
8. `noise_reduction` is matched case-sensitively: `HIGH` → default (0.025).
9. `baud_rate ≤ 0` → 9600 with a warning. A string `"115200"` is accepted. `com_port: 3` → `"3"` (an invalid port name; never connects).
10. A UTF-8 BOM, CRLF line endings and flow style (`{0: master}`) are all accepted. An empty file is valid (all defaults).
11. Unknown keys are ignored but **left untouched** in the file. Old configs may still contain `process_refresh_frequency`, which was removed in an earlier upstream version.
12. Process names are matched by **exact, case-insensitive exe name**, including `.exe`.
13. `deej.unmapped` membership is frozen at scan time (see §3), and the rescan after a config reload is throttled.
14. Saving the config re-applies all slider volumes (§2).
15. Upstream ignores LF-only lines and never reconnects after an unplug.
16. A stale `master` after a default-device change drops one slider move.
17. The `master` role is **eConsole**, not eMultimedia.
18. The first scan after start must succeed, or upstream exits.
19. The device-name heuristic `^.+ \(.+\)$` also classifies a hypothetical process key like `foo (bar)` as a device (practically irrelevant).
