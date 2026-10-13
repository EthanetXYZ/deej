# golden-gen

Records what the original Go deej does with each config in `testdata/configs/`, so the Rust loader is tested against real behaviour instead of a reading of the Go source.

The same program (`shared/golden.go`) is built twice:

| Folder | Dependencies | Output |
|---|---|---|
| `upstream/` | viper 1.7.1 → yaml.v2, cast 1.3.0 (upstream deej v0.9.10) | `testdata/golden/config-upstream.json` |
| `fork/` | viper 1.21.0 → yaml.v3 (this fork) | `testdata/golden/config-fork.json` |

It reads each file through the same viper getters deej uses. It then applies deej's own post-processing, which is copied from the Go source: `sliderMapFromConfigs`, the baud-rate fallback, `normalizeProcessNames`, and the sensitivity parsing.

## Regenerate after adding or changing a fixture

```bash
export PATH="/c/Program Files/Go/bin:$PATH"
tools/golden-gen/gen.sh
```

The first run needs network access to download the two dependency sets. `crates/deej-core/tests/golden_config.rs` fails if a fixture has no golden data.

A file named `NN_name.preferences.yaml` next to `NN_name.yaml` is loaded as that fixture's `logs/preferences.yaml`.
