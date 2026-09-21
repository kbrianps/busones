#!/usr/bin/env bash
# Starts busones and a static server, then points you at the dev viewer.
set -euo pipefail
cd "$(dirname "$0")/.."

[ -f data/gtfs.json.gz ] || ./scripts/fetch-gtfs.sh
[ -d dist/lines ] || cargo run --release --quiet -- gtfs export data/gtfs.json.gz dist
if [ ! -f dist/base/places.json ]; then
  if [ -f data/rio.pmtiles ]; then cargo run --release --quiet -- base build data/rio.pmtiles dist/base
  else ./scripts/fetch-basemap.sh; fi
fi
cargo build --release --quiet

BUSONES_RUNTIME_DIR=run ./target/release/busones serve &
bus=$!
python3 scripts/static-server.py . 8000 >/dev/null 2>&1 &
web=$!
trap 'kill $bus $web 2>/dev/null || true' EXIT INT TERM

echo
echo "  viewer:  http://127.0.0.1:8000/web/"
echo "  health:  http://127.0.0.1:8081/api/v1/status.json"
echo "  ctrl-c to stop"
echo
wait $bus
