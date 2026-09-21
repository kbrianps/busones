#!/usr/bin/env bash
# Cuts the Rio municipality out of the daily Protomaps build and turns it into
# the small vector tiles the client draws. Protomaps data is OpenStreetMap,
# ODbL: keep "© OpenStreetMap" in the page attribution.
set -euo pipefail
cd "$(dirname "$0")/.."
build="${PROTOMAPS_BUILD:-$(curl -fsS https://build-metadata.protomaps.dev/builds.json | python3 -c 'import sys,json; print(json.load(sys.stdin)[-1]["key"])')}"
pmtiles="${PMTILES_BIN:-pmtiles}"
if ! command -v "$pmtiles" >/dev/null 2>&1; then
  echo "needs the pmtiles CLI: https://github.com/protomaps/go-pmtiles/releases" >&2
  exit 1
fi
mkdir -p data
echo "extracting Rio from $build"
"$pmtiles" extract "https://build.protomaps.com/$build" data/rio.pmtiles \
  --bbox=-43.90,-23.15,-43.00,-22.65 --maxzoom=15
cargo run --release --quiet -- base build data/rio.pmtiles dist/base
