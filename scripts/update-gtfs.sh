#!/usr/bin/env bash
# Daily GTFS refresh, run by deploy/busones-gtfs.timer.
#
# Downloads the feed and, only when it changed, rebuilds the artifact,
# re-exports the client bundles with the service table, publishes them, and
# restarts the service so live data and the bundles agree on line names. Then
# lists the live codes the new feed still does not know, for config/README.md.
set -euo pipefail
cd "$(dirname "$0")/.."

bin="${BUSONES_BIN:-./target/release/busones}"
dist="${BUSONES_DIST:-dist}"
gtfs_dest="${BUSONES_GTFS:-/var/lib/busones/gtfs.json.gz}"
index="${BUSONES_INDEX:-/run/busones/api/v1/index.json}"
export BUSONES_ALIASES="${BUSONES_ALIASES:-config/service-aliases.json}"
restart="${BUSONES_RESTART:-systemctl restart busones}"

stamp() { stat -c %Y data/gtfs.json.gz 2>/dev/null || echo 0; }
before="$(stamp)"
BUSONES_BIN="$bin" ./scripts/fetch-gtfs.sh
if [ "$(stamp)" = "$before" ] && [ -z "${FORCE:-}" ]; then
  exit 0
fi

"$bin" gtfs export data/gtfs.json.gz "$dist"
install -m 0644 data/gtfs.json.gz "$gtfs_dest"
# Where the static bundles are served from is decided at deploy time; this
# hook uploads them (empty until then).
if [ -n "${BUSONES_PUBLISH_STATIC:-}" ]; then
  sh -c "$BUSONES_PUBLISH_STATIC"
fi
$restart
echo "gtfs updated (etag $(cat data/gtfs.etag 2>/dev/null || echo none)), service restarted"

# The live codes the new feed does not know, with the evidence for each.
if [ -f "$index" ]; then
  python3 scripts/alias-evidence.py --hours 24 --index "$index" || true
fi
