#!/usr/bin/env python3
"""Evidence for config/service-aliases.json.

For each live service code, samples one minute per hour from the ITS API and
compares where those buses actually run with the routes of the published GTFS.

    scripts/alias-evidence.py                  codes from run/api/v1/index.json
    scripts/alias-evidence.py 685 SV774 54     these codes
    scripts/alias-evidence.py --hours 24 685   a shorter window

Standard library only. See config/README.md for how to read the output.
"""

import argparse
import collections
import concurrent.futures as cf
import csv
import datetime
import json
import math
import sys
import urllib.parse
import urllib.request
from pathlib import Path

ITS = "https://its.mobilidade.rio/v1/geolocalizacao/veiculos"
UA = "busones-alias-evidence (+https://github.com/kbrianps/busones)"
NEAR_M = 60.0
CELL_M = 100.0


def fetch(svc, minute):
    rows, cursor = [], None
    for _ in range(8):
        q = {"minuto_utc": minute.strftime("%Y-%m-%dT%H:%M:00Z"), "servico": svc, "limit": 1000}
        if cursor:
            q["cursor"] = cursor
        req = urllib.request.Request(ITS + "?" + urllib.parse.urlencode(q), headers={"User-Agent": UA})
        with urllib.request.urlopen(req, timeout=30) as r:
            d = json.load(r)
        rows += d.get("data", [])
        cursor = d.get("next_cursor")
        if not cursor:
            break
    return rows


def collect(codes, hours):
    now = datetime.datetime.now(datetime.timezone.utc).replace(second=0, microsecond=0)
    minutes = [now - datetime.timedelta(minutes=5 + 60 * k) for k in range(hours)]
    out = {c: [] for c in codes}
    jobs = [(c, m) for c in codes for m in minutes]
    errors = 0
    with cf.ThreadPoolExecutor(4) as ex:
        futs = {ex.submit(fetch, c, m): c for c, m in jobs}
        for f in cf.as_completed(futs):
            try:
                out[futs[f]] += f.result()
            except Exception:
                errors += 1
    if errors:
        print(f"warning: {errors} of {len(jobs)} requests failed", file=sys.stderr)
    return out


def xy(lat, lon):
    k = 111_320.0
    return lon * k * math.cos(math.radians(-22.9)), lat * k


def load_gtfs(gtfs):
    names = {r["route_id"]: r["route_short_name"]
             for r in csv.DictReader(open(gtfs / "routes.txt", encoding="utf-8-sig"))}
    shape_routes = collections.defaultdict(set)
    for t in csv.DictReader(open(gtfs / "trips.txt", encoding="utf-8-sig")):
        shape_routes[t["shape_id"]].add(names.get(t["route_id"], "?"))
    pts = collections.defaultdict(list)
    for r in csv.DictReader(open(gtfs / "shapes.txt", encoding="utf-8-sig")):
        pts[r["shape_id"]].append((int(r["shape_pt_sequence"]), float(r["shape_pt_lat"]), float(r["shape_pt_lon"])))
    grid = collections.defaultdict(list)
    for sid, p in pts.items():
        p.sort()
        q = [xy(a, b) for _, a, b in p]
        for (x1, y1), (x2, y2) in zip(q, q[1:]):
            n = max(1, int(math.hypot(x2 - x1, y2 - y1) // CELL_M) + 1)
            cells = {(int((x1 + (x2 - x1) * k / n) // CELL_M), int((y1 + (y2 - y1) * k / n) // CELL_M))
                     for k in range(n + 1)}
            for c in cells:
                grid[c].append((x1, y1, x2, y2, sid))
    return names, shape_routes, grid


def routes_near(grid, shape_routes, lat, lon):
    x, y = xy(lat, lon)
    cx, cy = int(x // CELL_M), int(y // CELL_M)
    shapes = set()
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            for x1, y1, x2, y2, sid in grid.get((cx + dx, cy + dy), ()):
                if sid in shapes:
                    continue
                vx, vy = x2 - x1, y2 - y1
                ll = vx * vx + vy * vy
                t = 0.0 if ll == 0 else max(0.0, min(1.0, ((x - x1) * vx + (y - y1) * vy) / ll))
                if math.hypot(x - (x1 + t * vx), y - (y1 + t * vy)) <= NEAR_M:
                    shapes.add(sid)
    out = set()
    for s in shapes:
        out |= shape_routes.get(s, set())
    return out


def verdict(rows, shape_share, cov):
    if not rows:
        return "no data"
    vendors = {r.get("fornecedor") for r in rows}
    dirs = sum(1 for r in rows if r.get("sentido") in ("ida", "volta"))
    if shape_share and cov:
        (route, share), (croute, c) = shape_share[0], cov[0]
        if route == croute and c >= 85:
            return f"alias -> {route}"
    if len(vendors) == 1 and dirs / len(rows) < 0.1:
        return "not a line (one vendor, no direction)"
    return "line without a route"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("codes", nargs="*")
    ap.add_argument("--hours", type=int, default=48)
    ap.add_argument("--gtfs", type=Path, default=Path("data/gtfs"))
    ap.add_argument("--index", type=Path, default=Path("run/api/v1/index.json"))
    a = ap.parse_args()
    codes = a.codes or [c for c, _ in json.load(open(a.index)).get("unknown_services", [])]
    if not codes:
        print("no codes to check")
        return
    names, shape_routes, grid = load_gtfs(a.gtfs)
    data = collect(codes, a.hours)
    print(f"| Live code | Buses | Vendor shape | Coverage | Verdict |\n|---|---|---|---|---|")
    for code in codes:
        rows = [r for r in data[code] if r.get("latitude") and r.get("longitude")]
        by_shape = collections.Counter()
        for r in rows:
            for s in shape_routes.get(r.get("shape_id") or "", ()):
                by_shape[s] += 1
        cov = collections.Counter()
        step = max(1, len(rows) // 600)
        sample = rows[::step]
        for r in sample:
            for s in routes_near(grid, shape_routes, r["latitude"], r["longitude"]):
                cov[s] += 1
        shape_share = [(s, round(100 * n / len(rows))) for s, n in by_shape.most_common(2)] if rows else []
        cover = [(s, round(100 * n / len(sample))) for s, n in cov.most_common(2)] if sample else []
        buses = len({r.get("id_veiculo") for r in rows})
        fmt = lambda xs: ", ".join(f"{s} {p}%" for s, p in xs) or "-"
        print(f"| {code} | {buses} | {fmt(shape_share)} | {fmt(cover)} | {verdict(rows, shape_share, cover)} |")


if __name__ == "__main__":
    main()
