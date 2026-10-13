# deej configuration reference

This is the complete `config.yaml` reference for upstream deej v0.9.10 and this fork, and the contract deej Next must honour. Behaviour was verified in code and with the YAML probe described in the appendix.

## File location and loading

| Item | Behaviour |
|---|---|
| File | `config.yaml` in the working directory. The fork also switches to the executable's folder if that's where the file is |
| Format | YAML. Upstream reads it as YAML 1.1 (yaml.v2), the fork as YAML 1.2-ish (yaml.v3); see the appendix |
| Missing file | toast "Can't find configuration!", then deej exits |
| Parse error at startup | toast "Invalid configuration!", then deej exits |
| Parse error on reload | toast; previous values stay active |
| Hot reload | on file write (fork: write or create), de-bounced 500 ms, 50 ms settle; toast "Configuration reloaded!" |
| Key case | keys are case-insensitive (`Slider_Mapping` works) |
| Unknown keys | ignored, and preserved when the fork writes the file |
| Secondary file | `logs/preferences.yaml`: only its `slider_mapping` is read, and **merged** into the main mapping (see below) |

## Keys (upstream)

### `slider_mapping`

| | |
|---|---|
| Type | map of slider index → target, or list of targets |
| Default | `{}` (no sliders mapped). The code also defines `0: master` as a "default slider mapping", but it is **never used** |
| Index | 0-based, by position in the serial line, **not** by Arduino pin |

```yaml
slider_mapping:
  0: master
  1: chrome.exe
  3:
    - pathofexile_x64.exe
    - rocketleague.exe
```

Parsing rules (via viper `GetStringMapStringSlice`, then `sliderMapFromConfigs`):

- A scalar value is a one-element list. It is **never split on spaces**, which is what makes device names with spaces work.
- Every value becomes a string: `5` → `"5"`, `true` → `"true"`, upstream `no` → `"false"`.
- Empty strings and null items are removed. `3:` gives slider 3 with no targets.
- Keys go through `strconv.Atoi`, **and errors are ignored**: non-integer keys become slider **0** and overwrite each other (whichever comes last in random map order wins). `"01"` → 1. `-1` → −1 (never matches a slider).
- A scalar `slider_mapping` (`slider_mapping: master`) gives an empty mapping, with no error.
- `logs/preferences.yaml` merge: for each slider there, its targets are appended to the main config's list for the same index, skipping duplicates and empty strings.

Targets are case-insensitive (lowercased before matching):

| Target | Meaning | Platform |
|---|---|---|
| `master` | default playback device volume (role eConsole) | all |
| `mic` | default recording device level (role eConsole) | all |
| `system` | "System sounds" session(s) | Windows |
| `chrome.exe` (any exe name) | all audio sessions of processes with that exe file name, on all active playback devices | all |
| `Speakers (Realtek High Definition Audio)` | that device's endpoint volume, by full friendly name; input or output | Windows |
| `deej.current` | the foreground app (plus child-window processes, e.g. UWP hosts) | Windows |
| `deej.unmapped` | every app session not mapped to any slider (excluding master/system/mic/devices and, in the fork, `unmapped_exclude`) | all |
| a list of the above | all of them (a "group") | all |

### `invert_sliders`

| Type | Default | Notes |
|---|---|---|
| bool | `false` | `position = 1 − position`. Upstream also accepts `yes`/`on`/`no`/`off` (YAML 1.1); the fork reads `yes`/`on` as **false**. `"true"` (string) and `1` → true in both |

### `com_port`

| Type | Default | Notes |
|---|---|---|
| string | `COM4` | Passed as-is to the OS. A number (`3`) becomes `"3"`, which isn't a valid Windows port name. Linux paths such as `/dev/ttyUSB0` are valid on Linux |

### `baud_rate`

| Type | Default | Notes |
|---|---|---|
| int | `9600` | Values ≤ 0 → 9600 with a warning. String numbers are accepted. Must match the sketch's `Serial.begin` |

### `noise_reduction`

| Type | Default | Values |
|---|---|---|
| string | unset (treated as `default`) | `low` (threshold 0.015, 2% steps), `default` (0.025, 3%), `high` (0.035, 4%). **Case-sensitive**; anything else means `default` |

### Removed or legacy keys

- `process_refresh_frequency`: removed upstream before v0.9 (a code comment says it's now a constant 5 s/45 s). It may still exist in old configs. It must be **tolerated and preserved**.

## Keys added by this fork

### `slider_sensitivity`

```yaml
slider_sensitivity:
  default:          # every slider without an override
    curve: 2        # 0.2–6, or a preset name
    max_volume: 100 # 1–100; a "%" suffix is accepted
  1:                # per-slider override; unset fields fall back to default
    curve: 1.6
```

- `volume = max_volume/100 × clamp(position, 0, 1)^curve`, clamped to 0–1, then passed to WASAPI as f32.
- The code default is curve **1**, max **100**, which is identical to upstream.
- Presets (case-insensitive; `very fine` with a space is also accepted): `linear` 1, `gentle` 1.5, `balanced` 2, `fine` 2.5, `very_fine` 3.
- Invalid values: toast "Invalid slider sensitivity setting!" plus a log entry, and **only that field** is ignored. A key that isn't `default` or a non-negative integer → error, ignored.

### `unmapped_exclude`

```yaml
unmapped_exclude:
  - mpvnet.exe
```

- List of process names (trimmed, lowercased, de-duplicated) that `deej.unmapped` leaves alone. A scalar string is **split on whitespace**.
- The settings window writes an empty list as `[]` rather than deleting the key.

## Implicit defaults summary

| Key | Default when absent |
|---|---|
| `slider_mapping` | `{}` |
| `invert_sliders` | `false` |
| `com_port` | `COM4` |
| `baud_rate` | `9600` |
| `noise_reduction` | `default` |
| `slider_sensitivity.default.curve` | `1` |
| `slider_sensitivity.default.max_volume` | `100` |
| `unmapped_exclude` | `[]` |

## deej Next additions

These rules follow the "new features must not break legacy configs" requirement. They were approved on 2026-10-12. `deej_next.serial` is parsed as of Phase 1; `smoothing` is still a proposal.

1. `config.yaml` stays the single source of truth for everything the **Go deej also understands**, so a user can go back to the Go version at any time.
2. **New behavioural settings** are optional keys under one new top-level section that the Go builds ignore:

   ```yaml
   deej_next:
     serial:
       dtr: true            # default: assert DTR (matches both Go versions)
       rts: false           # default: upstream behaviour
       reconnect_interval_ms: 2000
     smoothing: none        # none | light | strong (default none = original behaviour)
   ```

3. **UI-only state** (theme, window size and position, start minimised, last tab) goes in a separate `deej-next-ui.json`, never in `config.yaml`.
4. Unknown keys are preserved on write and listed under Diagnostics as "not recognised".

## Appendix: verified YAML edge cases

These results come from a probe that runs each file through the same viper getters deej uses (`GetStringMapStringSlice`, `GetBool`, `GetString`, `GetInt`, `GetStringSlice`, `GetStringMap`). Upstream uses viper 1.7.1, the fork viper 1.21.0. Slider-index handling (`Atoi`) is applied afterwards by deej, as described above.

| # | Input | Upstream result | Fork result |
|---|---|---|---|
| 01 | basic map, list group | as written | same |
| 02 | `0: Speakers (Realtek …)`, `1: chrome.exe firefox.exe` | single strings, not split | same |
| 03 | `0:` / `1: ""` / `2: ["", x.exe]` | `[""]`, `[""]`, `["", "x.exe"]` (empty strings dropped later) | same |
| 04 | keys `foo`, `"01"`, `-1`, `1.5` | kept as strings, so `Atoi` → 0, 1, −1, 0 | same |
| 05 | `invert_sliders: yes` | **true** | **false** |
| 06 | `invert_sliders: "true"` | true | true |
| 07 | `invert_sliders: on` | **true** | **false** |
| 08 | `invert_sliders: 1` | true | true |
| 09 | `baud_rate: "115200"`, `com_port: 3` | 115200, `"3"` | same |
| 10 | `baud_rate: -5` | −5 (deej then substitutes 9600) | same |
| 11 | `Slider_Mapping`, `COM_PORT`, `MASTER` | keys case-insensitive; target kept as `MASTER` (lowercased at match time) | same |
| 12 | targets `5`, `true`, `1.5` | `"5"`, `"true"`, `"1.5"` | same |
| 13 | duplicate key `0:` twice | **last wins** | **load error** |
| 14 | `slider_mapping: master` | empty mapping | same |
| 15 | `noise_reduction: HIGH` | `"HIGH"` (treated as default) | same |
| 16 | tab indentation | parse error | parse error |
| 17 | empty file | all defaults | same |
| 18 | `unmapped_exclude: mpvnet.exe` (scalar) | n/a (key unknown upstream) | `["mpvnet.exe"]` |
| 19 | unknown keys | ignored | ignored |
| 20 | `invert_sliders: True` | true | true |
| 21 | flow style `{0: master, 1: [a, b]}` | as written | same |
| 22 | CRLF line endings | as written | same |
| 23 | UTF-8 BOM | as written | same |
| 24 | `0: ~`, `1: null` | `[""]` (dropped later) | same |
| 25 | sensitivity with `balanced`, `"80%"` | raw values preserved for deej to parse | same |
| 26 | `invert_sliders: no`, `0: no`, `1: off` | false; targets **`"false"`, `"false"`** | false; targets `"no"`, `"off"` |

**Compatibility decision required:** rows 05/07/26 and 13 differ between upstream and the fork. The proposal in [COMPATIBILITY_MATRIX.md](COMPATIBILITY_MATRIX.md) is to follow upstream (YAML 1.1 booleans for **boolean-typed keys only**, last-wins duplicates with a warning), because that's the more permissive behaviour and accepts every file that either Go version accepts.
