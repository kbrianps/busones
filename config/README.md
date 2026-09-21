# Service table

`service-aliases.json` reconciles the line codes buses report in the live feed
with the published GTFS. Every entry is backed by evidence; nothing here is a
guess from the line number.

- **`aliases`**: live code to GTFS `route_short_name`. When the GTFS still uses
  the provisional `LECD` code and buses report the public number, the line is
  shown under the public number and the `LECD` code keeps resolving to it
  (685 is shown for `LECD140`). When it goes the other way, because the GTFS
  already has the final number, that number is kept (`LECD156` resolves to 856).
- **`not_service`**: live codes that name no line. Buses reporting them are
  treated as out of service.

Both `busones serve` and `busones gtfs export` apply the table, so live data
and the client bundles always agree on what a line is called. Re-export the
bundles after editing it.

## How an alias is proven

Run `scripts/alias-evidence.py` (standard library only). For each live code it
samples one minute per hour over the last 48 hours from the ITS API, which
keeps at least three days of history and filters by `servico`, and measures:

1. **Vendor shape**: the share of fixes whose `shape_id` (sent by Conecta and
   Maxtrack) belongs to one GTFS route.
2. **Coverage**: the share of fixes within 60 m of any shape of that route.

An alias needs both to point at the same route, with coverage of at least 85%.

## Evidence, 2026-09-21

Sample: one minute per hour, 2026-09-19 to 2026-09-21.

| Live code | GTFS route | Route name | Buses | Vendor shape | Coverage |
|---|---|---|---|---|---|
| 685 | LECD140 | Terminal Margaridas - Méier | 47 | 82% | 88% |
| SV774 | LECD142 | Madureira - Jardim América | 37 | 89% | 96% |
| 109 | LECD131 | São Conrado - Terminal Gentileza | 21 | 83% | 91% |
| SVB685 | LECD141 | Terminal Margaridas - Méier | 29 | 82% | 91% |
| LECD156 | 856 | Capoeira Grande - Terminal Campo Grande | 28 | 88% | 87% |
| 391 | SV391 | Padre Miguel - Praça da República | 9 | 72% | 95% |
| 456 | LECD132 | Norte Shopping - Copacabana | 26 | 91% | 92% |
| 950 | LECD144 | Metrô Vicente de Carvalho - Terminal Margaridas | 15 | 87% | 94% |
| SVA665 | LECD147 | Metrô Pavuna - Saens Peña | 26 | 83% | 97% |
| 157 | LECD127 | Gávea - Rodoviária | 7 | 92% | 94% |
| 951 | LECD145 | Metrô Vicente de Carvalho - Terminal Margaridas | 8 | 87% | 97% |
| 959 | LECD146 | Metrô Vicente de Carvalho - Terminal Margaridas | 6 | 91% | 97% |
| 944 | LECD143 | Metrô Pavuna - Terminal Margaridas | 11 | 82% | 88% |

### Not lines

| Live code | Why |
|---|---|
| 3 | 122 different buses in 48 h, Conecta only, no direction on 99% of fixes, spread across the whole city; best route coverage 23% |
| 441 | 15 buses with 2 fixes each, Conecta only, never a direction; best coverage 13% |
| 001 | 28 buses, Conecta only, always `circular`, spread across the metropolitan area beyond the city limits; best coverage 28% |
| TR | 3 buses in 24 h, one vendor, never a direction |

`000`, garage codes such as `1 GAR`, `TREINO` and `VISTORIA` are handled by the
built-in out-of-service rules instead.

### Real lines with no published route

These run all day with both directions and two vendors, but no GTFS route
matches them. They are published as lines without a route: searchable, with
their buses on the map, no direction and no arrival times.

| Live code | Buses | Best coverage |
|---|---|---|
| 54 (BRT) | 20 | 27% (53) |
| LECD153 | 9 | 65% (LECD136) |
| LECD154 | 8 | 41% (LECD134) |
| LECD155 | 7 | 68% (SN368) |
| LECD157 | 8 | 86% (826), no vendor shape: not enough for an alias |
| LECD158 | 6 | 53% (2804) |
| 2305 - A | 4 | 62% (2305) |
| 2305 - B | 3 | 53% (399) |
| 2802SV | 5 | 74% (2802) |

Check again whenever the GTFS ETag changes: a new feed may add these routes or
rename the ones above.
