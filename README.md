# busones

Real-time bus tracking for the city of Rio de Janeiro.

`busones` ingests the city's live vehicle feeds, works out where each bus is
along which route and in which direction, and publishes small JSON snapshots
that a browser can poll through a CDN. It is the backend half of a progressive
web app whose goal is to answer one question well: *which bus is coming to my
stop, and how far away is it*.

Status: **phase 1** (ingestion, matching and publishing). Arrival predictions in
minutes are phase 3; see `Roadmap`.

## Why it works this way

An earlier attempt inferred direction in the browser from two consecutive polls
and kept no state anywhere. That cannot work here. In the published GTFS, 311 of
the 441 two-way routes run their outbound and inbound shapes within 30 m of each
other over more than half their length, 120 shapes double back along the same
street, and 41 routes are closed loops. Position alone is not enough, and a
client that forgets everything between polls has nothing else to go on.

So every decision that needs more than one vehicle, or more than one fix, is
made on the server:

- **A hypothesis is a pass, not a shape.** For each candidate shape, every
  distinct place along it near the vehicle competes as its own hypothesis, which
  is what lets heading and forward progress separate the two halves of an
  out-and-back street.
- **Direction is earned.** A vehicle publishes a direction only after a net
  100 m of progress along one hypothesis across four fixes with no backward step
  over 30 m. Until then it is drawn on the map as `pending`, with no direction.
- **Physics is per unit.** SPPO buses carry two independent tracking units that
  report ~30 s apart, unsynchronised, from positions up to 80 m apart. Jump
  filtering compares a fix only against the same unit's previous fix, and the
  motion model carries 25 m of slack.
- **Staleness is bounded on the server.** A fix older than 5 minutes never
  touches live state, a timestamp that does not advance is not a new fix (the
  BRT snapshot repeats the same rows every minute), and a vehicle that goes
  quiet for 3 minutes stops being predicted and is forgotten after 5.

## Data sources

| Source | Use |
|---|---|
| `its.mobilidade.rio/v1/geolocalizacao/veiculos` | primary: every vendor in one schema, one UTC minute per call |
| `dados.mobilidade.rio/gtfs/realtime/brt/vehicle-positions` | BRT, already matched to trips by SMTR |
| `dados.mobilidade.rio/gtfs/schedule` | routes, shapes, stops, stop distances |

The aggregator's open minute fills in steps, and a closed minute is only
complete about 15 s after it ends: asking earlier returns a first-quarter stub
with HTTP 200. `busones` reads the open minute four times (at +8, +23, +38 and
+53 s) and the closed minute once at +18 s, retrying when the response looks
like a stub.

Data is published by the Secretaria Municipal de Transportes under CC BY 4.0.

## Running it

```bash
# Everything at once: prepares the feed if needed, starts the server and a
# static file server, and prints the viewer URL.
./scripts/dev.sh
```

Then open <http://127.0.0.1:8000/web/>. The viewer is a development tool, not
the phase 2 client: one dependency-free HTML file that reads exactly what the
server publishes, so anything wrong with the snapshots shows up on screen.
Pick a line to draw its shapes and stops, or leave it on "all lines in view" to
load the tile snapshots the way the real client will. Clicking a bus prints its
raw record. Deep links work: `?line=483&z=14&at=-22.88,-43.26`. The basemap is
drawn by the client from small vector tiles that `busones base build` cuts out
of a Protomaps extract: water, green areas, beaches, streets and names only,
about 0.4 KB per tile at street level against 15 to 30 KB for an OSM raster
tile. `./scripts/fetch-basemap.sh` refreshes it.

The pieces separately:

```bash
# 1. Prepare the static feed (25 MB download, ~7 s of processing).
./scripts/fetch-gtfs.sh

# 2. Run. Snapshots land in ./run, health on 127.0.0.1:8081.
cargo run --release -- serve

# One-shot measurement of every upstream feed.
cargo run --release -- probe

# Static bundles for the browser client: one file per line, stops, stop index,
# headways, and the calendar (holidays run the Sunday service).
cargo run --release -- gtfs export data/gtfs.json.gz dist
```

On the server, `scripts/update-gtfs.sh` runs daily from
`deploy/busones-gtfs.timer` and does steps 1 and the export only when the feed
changed, then restarts the service.

Configuration is environment only, so the systemd unit is the single place
deployment differs from a laptop: `BUSONES_ITS_URL`, `BUSONES_BRT_URL`,
`BUSONES_GTFS`, `BUSONES_ALIASES`, `BUSONES_RUNTIME_DIR`, `BUSONES_STATE_DIR`,
`BUSONES_STATUS_ADDR`, `BUSONES_CELL_ZOOM`, `BUSONES_WARM_MINUTES`,
`BUSONES_PUBLISH_SECS`, `BUSONES_BRT_SECS`, `BUSONES_LOG`.

`config/service-aliases.json` reconciles live service codes with the GTFS:
aliases (buses report 685 for what the GTFS calls `LECD140`) and codes that name
no line. Every entry is backed by evidence from `scripts/alias-evidence.py`; see
`config/README.md`. Zero padding is handled without it (`7` finds `007`). Codes
with no route are still tracked and published as lines without a route; they
simply get no direction and no arrival times.

## What it publishes

Under `$BUSONES_RUNTIME_DIR/api/v1`, rewritten every 10 s, with a `.gz` beside
each file:

| Path | Contents |
|---|---|
| `lines/{line}.json` | every tracked vehicle of that line, both directions |
| `cells/{z}/{x}/{y}.json` | the same vehicles grouped by map tile, for "near me" |
| `fleet.json` | every vehicle in one file, for a city-wide overview only |
| `live-lines.json` | `[line, buses, has_route]` for every line on the road now, for the search |
| `index.json` | counts, per-line totals, feed health, generation time |
| `status.json` | served by the process, never a file (see below) |

`fleet.json` exists for an operations or debugging view of the whole
municipality, where the alternative is dozens of tile requests per refresh. The
rider-facing client must not use it: someone watching one line should download
that line, not all two thousand buses in Rio.

Every known line and every tile is written on every cycle, empty array included,
so a missing file always means "the server has not published yet" and never
"this line has no buses". A file is only rewritten when its bytes change, which
keeps `ETag` and `Last-Modified` stable and lets the edge and the browser answer
304. That is also why a vehicle record carries the fix timestamp `t` rather than
an age in seconds: age is `now - t`, computed by the reader, so an unchanged
snapshot stays byte-identical.

Vehicle fields:

| Key | Meaning |
|---|---|
| `id` | vehicle number as the operator registers it |
| `t` | unix seconds of the GPS fix, from the device clock |
| `lat`, `lon` | position, 5 decimal places |
| `brg` | bearing in degrees, or null |
| `ph` | `live`, `layover`, `pending`, `parked`, `stale` or `offroute` |
| `dir` | `ida`, `volta` or `circular`; absent until direction is confirmed |
| `shp` | GTFS shape id of the confirmed pass |
| `alo` | metres travelled along that shape |
| `ns` | GTFS id of the next stop |
| `spd` | km/h, derived from movement, never the feed's own speed field |
| `cnf` | 0 to 1, how much to trust `dir` and `alo` |
| `ven` | which vendor supplied the last fix |
| `line` | route short name (tile files only) |

`status.json` is answered by the process itself rather than served from disk: a
snapshot on disk would keep reporting `ok` long after the publisher died, which
is exactly the failure an external monitor exists to catch. It reports
`ok | stale | down`, the age of the newest fix, per-source health and filesystem
usage, and is the endpoint an uptime monitor should watch for the literal
strings `"status":"ok"` and `"disk":"ok"`.

## Deployment

`deploy/` holds a hardened systemd unit, a Caddy site file and notes for
publishing through a Cloudflare tunnel with no inbound ports. The edge
configuration matters as much as the code: `/api/*` must be cacheable with the
origin's own TTL, the cache key must ignore query strings, and Bot Fight Mode
must stay off. See `PLANO.md` in the sibling planning repository for the
reasoning and the numbers.

## Layout

```
src/
  geo.rs        local planar projection for Rio, segment geometry, map tiles
  timeutil.rs   UTC parsing and formatting with no calendar dependency
  csv.rs        a tolerant RFC 4180 reader for the GTFS text files
  gtfs.rs       feed preparation, the in-memory model, client bundle export
  matcher.rs    pass hypotheses, the beam, direction commitment
  engine.rs     hygiene, deduplication, the phase machine, snapshots
  ingest.rs     polling schedules
  ingest/its.rs the aggregator client
  ingest/brt.rs a minimal GTFS-Realtime reader (no protoc, no code generation)
  publish.rs    snapshot rendering and change-detecting writes
  status.rs     the health endpoint
web/index.html  development viewer: no build step, no dependencies
```

`cargo test` covers the parts that are easy to get quietly wrong: timestamp
parsing, the projection's scale, pass separation on a doubled-back shape, the
refusal to commit a direction without progress, the two-unit jump filter, the
age gates, and the promise that unchanged snapshots are not rewritten.

## Roadmap

Phase 1 is here. Still to come:

- **Phase 2**: the browser client, self-rendered map tiles, the "near me" screen
  expressed in stops and distance.
- **Phase 3**: stop events, per-segment travel-time history by day type, arrival
  predictions published as calibrated ranges rather than a single minute.
- Garage polygons. Until they exist, a vehicle that has not moved 100 m in ten
  minutes and never confirmed a direction is classified `parked` and left out of
  the direction-rate denominator.
- Per-vendor fallbacks for when the aggregator is unavailable. It is in beta and
  undocumented, so the ingestion contract is deliberately isolated.

## License

GPL-3.0-or-later. See `LICENSE`.
