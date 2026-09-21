#!/usr/bin/env bash
# Downloads the published Rio GTFS feed and prepares the artifact busones loads.
#
# The SMTR endpoint answers 404 to HEAD and 200 to GET, so the ETag cannot be
# read cheaply up front. The feed is downloaded once with its headers, and the
# rebuild is skipped when the ETag matches the one from the last run.
set -euo pipefail
url="${GTFS_URL:-https://dados.mobilidade.rio/gtfs/schedule}"
# The installed binary on the server, cargo on a laptop.
bin="${BUSONES_BIN:-cargo run --release --quiet --}"
dir="${1:-data/gtfs}"
mkdir -p data
zip="data/gtfs.zip"
hdr="$(mktemp)"
trap 'rm -f "$hdr"' EXIT

echo "downloading $url"
curl -fsSL -D "$hdr" -o "$zip" "$url"
new_etag="$(awk 'tolower($1)=="etag:"{print $2}' "$hdr" | tail -1 | tr -d '\r"')"
prev_etag="$(cat data/gtfs.etag 2>/dev/null || true)"

# FORCE=1 rebuilds anyway, for a new busones that extracts more from the feed.
if [ -z "${FORCE:-}" ] && [ -n "$new_etag" ] && [ "$new_etag" = "$prev_etag" ] && [ -f data/gtfs.json.gz ] && [ -f "$dir/stop_times.txt" ]; then
  echo "feed unchanged (etag $new_etag); nothing to do"
  rm -f "$zip"
  exit 0
fi

rm -rf "$dir"
mkdir -p "$dir"
unzip -q -o "$zip" -d "$dir"
rm -f "$zip"
$bin gtfs build "$dir" data/gtfs.json.gz
[ -n "$new_etag" ] && printf '%s' "$new_etag" > data/gtfs.etag
echo "done"
