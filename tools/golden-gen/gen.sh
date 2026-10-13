#!/usr/bin/env bash
# Regenerates testdata/golden/config-{upstream,fork}.json from testdata/configs/*.yaml.
# Needs Go on PATH and network access for the first run (or a warm module cache).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
for variant in upstream fork; do
  cp "$here/shared/golden.go" "$here/$variant/golden.go"
  (cd "$here/$variant" && go mod tidy >/dev/null && go run . "$root/testdata/configs") > "$root/testdata/golden/config-$variant.json"
  echo "wrote testdata/golden/config-$variant.json"
done
(cd "$here/protocol" && go run .) > "$root/testdata/golden/protocol.json"
echo "wrote testdata/golden/protocol.json"
