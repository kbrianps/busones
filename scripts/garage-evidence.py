#!/usr/bin/env python3
"""Garage areas for config/garages.json, from where buses sleep.

Samples every bus position at 03:00 and 04:00 in Rio over the last nights
from the ITS API (it keeps about three days), keeps the vehicles standing
still, and groups them on a 60 m grid. A group of many different buses far
from the end of any GTFS route is a garage; small groups at route ends are
terminals, where buses rest or wait for the night service, and are left out.

    scripts/garage-evidence.py              writes config/garages.json
    scripts/garage-evidence.py --dry-run    only prints the table

Standard library only.
"""

import argparse
import collections
import csv
import datetime
import json
import math
import urllib.parse
import urllib.request
from pathlib import Path

ITS = "https://its.mobilidade.rio/v1/geolocalizacao/veiculos"
UA = {"User-Agent": "busones-garage-evidence (+https://github.com/kbrianps/busones)"}
CELL_M = 60
MARGIN_M = 30
K = 111_320.0
COS = math.cos(math.radians(-22.9))


def xy(lat, lon):
    return lon * K * COS, lat * K


def fetch_minute(minute):
    rows, cursor = [], None
    for _ in range(10):
        q = {"minuto_utc": minute, "limit": 5000}
        if cursor:
            q["cursor"] = cursor
        req = urllib.request.Request(ITS + "?" + urllib.parse.urlencode(q), headers=UA)
        with urllib.request.urlopen(req, timeout=60) as r:
            d = json.load(r)
        rows += d.get("data", [])
        cursor = d.get("next_cursor")
        if not cursor:
            break
    return rows


def route_ends(gtfs):
    pts = collections.defaultdict(list)
    with open(gtfs / "shapes.txt", encoding="utf-8-sig") as f:
        for r in csv.DictReader(f):
            pts[r["shape_id"]].append((int(r["shape_pt_sequence"]), float(r["shape_pt_lat"]), float(r["shape_pt_lon"])))
    ends = []
    for v in pts.values():
        v.sort()
        ends += [xy(v[0][1], v[0][2]), xy(v[-1][1], v[-1][2])]
    return ends


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--nights", type=int, default=3)
    ap.add_argument("--gtfs", type=Path, default=Path("data/gtfs"))
    ap.add_argument("--out", type=Path, default=Path("config/garages.json"))
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()

    today = datetime.datetime.now(datetime.timezone.utc).date()
    cells = collections.defaultdict(set)
    for n in range(a.nights):
        day = today - datetime.timedelta(days=n)
        for hm in ("06:00", "07:00"):  # 03:00 and 04:00 in Rio
            minute = f"{day.isoformat()}T{hm}:00Z"
            if datetime.datetime.fromisoformat(minute.replace("Z", "+00:00")) > datetime.datetime.now(datetime.timezone.utc):
                continue
            rows = fetch_minute(minute)
            print(f"{minute}: {len(rows)} positions")
            for r in rows:
                if not r.get("latitude") or (r.get("velocidade") or 0) > 3:
                    continue
                x, y = xy(r["latitude"], r["longitude"])
                cells[(int(x // CELL_M), int(y // CELL_M))].add(r["id_veiculo"])

    dense = {c: v for c, v in cells.items() if len(v) >= 6}
    seen, groups = set(), []
    for c in dense:
        if c in seen:
            continue
        stack, comp = [c], []
        seen.add(c)
        while stack:
            p = stack.pop()
            comp.append(p)
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    q = (p[0] + dx, p[1] + dy)
                    if q in dense and q not in seen:
                        seen.add(q)
                        stack.append(q)
        buses = set().union(*(dense[k] for k in comp))
        xs = [k[0] for k in comp]
        ys = [k[1] for k in comp]
        groups.append((len(buses), min(xs) * CELL_M, min(ys) * CELL_M, (max(xs) + 1) * CELL_M, (max(ys) + 1) * CELL_M))

    ends = route_ends(a.gtfs)
    garages = []
    print(f"\n| Buses | Centre | Nearest route end | Verdict |\n|---|---|---|---|")
    for buses, x0, y0, x1, y1 in sorted(groups, reverse=True):
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        end = min(math.hypot(cx - ex, cy - ey) for ex, ey in ends)
        garage = buses >= 40 or (buses >= 20 and end >= 150)
        verdict = "garage" if garage else "terminal" if end < 150 else "too few buses to tell"
        print(f"| {buses} | {cy / K:.5f}, {cx / (K * COS):.5f} | {end:.0f} m | {verdict} |")
        if garage:
            garages.append({
                "buses": buses,
                "lat0": round((y0 - MARGIN_M) / K, 6), "lon0": round((x0 - MARGIN_M) / (K * COS), 6),
                "lat1": round((y1 + MARGIN_M) / K, 6), "lon1": round((x1 + MARGIN_M) / (K * COS), 6),
            })
    print(f"\n{len(garages)} garages")
    if not a.dry_run:
        body = {"generated": today.isoformat(), "nights": a.nights, "garages": garages}
        a.out.write_text(json.dumps(body, indent=1) + "\n")
        print(f"wrote {a.out}")


if __name__ == "__main__":
    main()
