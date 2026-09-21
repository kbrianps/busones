# Deploying busones

Target: the Oracle Ampere A1 instance (2 OCPU, 12 GB, sa-saopaulo-1, aarch64),
reached from the internet only through a Cloudflare tunnel. No inbound port is
opened; `cloudflared` makes an outbound connection on 7844.

## 1. Build for aarch64

On the VM: `cargo build --release`, then `install -m755 target/release/busones
/usr/local/bin/busones`. Cross-compiling from x86 works with
`cargo build --release --target aarch64-unknown-linux-gnu` and a linker.

## 2. Static feed

```bash
sudo install -d -o root -g root -m 0750 /var/lib/busones
./scripts/fetch-gtfs.sh
sudo install -m 0644 data/gtfs.json.gz /var/lib/busones/gtfs.json.gz
```

The feed moves every few months, and holidays live in it too
(`calendar_dates.txt`: the SMTR runs the Sunday service on them). A daily timer
keeps both current without manual steps: it downloads the feed, and only when
the ETag changed rebuilds, re-exports the client bundles, publishes them and
restarts the service, then logs the live codes the new feed does not know.

```bash
sudo git clone https://github.com/kbrianps/busones /opt/busones
sudo cp deploy/busones-gtfs.service deploy/busones-gtfs.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now busones-gtfs.timer
sudo systemctl start busones-gtfs.service   # first run builds everything
journalctl -u busones-gtfs.service -n 30
```

Set `BUSONES_PUBLISH_STATIC` in the unit to the command that uploads `dist/`
once the static hosting is decided.

After upgrading the binary, `busones serve` refuses an artifact prepared by an
older version (it would silently lack new fields such as holidays or line
colours). Rebuild it once with `FORCE=1 ./scripts/update-gtfs.sh`.

The service table ships with the repository. Install it next to the binary and
re-export the client bundles whenever it or the feed changes, so both agree on
line names (see `config/README.md`):

```bash
sudo install -D -m 0644 config/service-aliases.json /usr/local/share/busones/service-aliases.json
sudo install -D -m 0644 config/garages.json /usr/local/share/busones/garages.json
BUSONES_ALIASES=config/service-aliases.json busones gtfs export data/gtfs.json.gz dist
```

The garage areas (`config/garages.json`) come from where buses sleep at 03:00
and 04:00; regenerate them with `scripts/garage-evidence.py` when operators
move garages, then reinstall and restart.

## 3. Services

```bash
sudo cp deploy/busones.service /etc/systemd/system/
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile
sudo systemctl daemon-reload
sudo systemctl enable --now busones caddy
curl -s localhost:8081/api/v1/status.json | head -c 400
curl -sI localhost:8080/api/v1/lines/474.json
```

`caddy.service` must start after `busones.service` so it never serves an empty
tmpfs. Add a drop-in with `After=busones.service`.

## 4. Tunnel

```bash
cloudflared tunnel create busones
cloudflared tunnel route dns busones <hostname>
# ingress: <hostname> -> http://127.0.0.1:8080
sudo cloudflared service install
```

Run it as its own user rather than root: the installer's unit has no `User=`.

## 5. Cloudflare zone

Three settings decide whether this costs nothing and whether the origin load
stays bounded by the number of URLs instead of the number of users:

1. **Cache Rule** on `/api/*`: eligible for cache, Edge TTL and Browser TTL both
   "respect origin", serve stale while revalidating on, **cache key ignoring the
   query string**, and TTL "no store" for status 404 and 410. Without the query
   string rule, one client appending `?t=` reaches the origin on every request.
2. **Transform Rule** stripping the query string from `/api/*`, as a second line
   of defence for the same problem.
3. **Bot Fight Mode off.** On the Free plan it covers the whole zone, cannot be
   scoped, and Cloudflare's own documentation warns it may challenge API and app
   traffic. It would break the PWA's `fetch`, any uptime monitor, and the cache
   verification itself. Bots hitting a cached file cost nothing.

Also enable Smart Tiered Cache, and set the zone's Browser Cache TTL to "respect
existing headers".

Verify before trusting it:

```bash
for i in $(seq 1 12); do
  curl -sS -o /dev/null -D - https://<host>/api/v1/lines/474.json |
    grep -iE 'cf-cache-status|^age|cache-control'
  sleep 2
done
```

Expect `MISS`, then `HIT` with a rising `Age`, then `UPDATING` just after 10 s,
and `cache-control` unchanged at `max-age=10`. Confirm `?x=1` and `?x=2` return
the same `Age`.

## 6. Monitoring

Nothing inside the VM can report that the VM is gone. Two free monitors close
that gap:

- UptimeRobot on the public `status.json`, one monitor keyed on `"status":"ok"`
  and one on `"disk":"ok"`, every 5 minutes.
- Cloudflare Zero Trust "Tunnel Health Alert" for down and degraded.
- On OCI: a Notifications topic with a confirmed email, an alarm on
  `CpuUtilization[1m].absent(10m)`, and an alarm on
  `MemoryUtilization[1d].mean() < 20` as an early warning for the idle profile.
