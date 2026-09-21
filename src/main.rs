//! busones: real-time bus tracking backend for the city of Rio de Janeiro.
//!
//! Copyright (C) 2026 Brian Pravato. Licensed under the GNU GPL v3 or later.

mod basemap;
mod config;
mod csv;
mod engine;
mod geo;
mod geocode;
mod gtfs;
mod ingest;
mod matcher;
mod model;
mod pb;
mod polyline;
mod publish;
mod status;
mod timeutil;

use anyhow::{bail, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use config::Config;
use engine::Engine;
use publish::Publisher;

const USAGE: &str = "busones - real-time bus tracking for Rio de Janeiro

USAGE:
  busones serve                        run the ingester and publisher
  busones gtfs build [DIR] [OUT]       prepare the static feed (default data/gtfs -> data/gtfs.json.gz)
  busones gtfs export [GTFS] [OUT]     write the static client bundles (default dist/)
  busones base build [PMTILES] [OUT]   draw-ready basemap tiles (default data/rio.pmtiles -> dist/base)
  busones probe                        measure one closed minute of every feed

Configuration comes from the environment:
  BUSONES_ITS_URL, BUSONES_BRT_URL, BUSONES_GTFS, BUSONES_ALIASES,
  BUSONES_RUNTIME_DIR, BUSONES_STATE_DIR, BUSONES_STATUS_ADDR,
  BUSONES_CELL_ZOOM, BUSONES_WARM_MINUTES, BUSONES_PUBLISH_SECS, BUSONES_BRT_SECS";

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BUSONES_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("serve") {
        "serve" => serve().await,
        "gtfs" => gtfs_cmd(&args[1..]),
        "probe" => probe().await,
        "base" => base_cmd(&args[1..]),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("{USAGE}");
            bail!("unknown command: {other}");
        }
    }
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!(
            "busones/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/kbrianps/busones)"
        ))
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?)
}

fn gtfs_cmd(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("build") => {
            let dir: PathBuf = args.get(1).map_or_else(|| "data/gtfs".into(), PathBuf::from);
            let out: PathBuf = args
                .get(2)
                .map_or_else(|| "data/gtfs.json.gz".into(), PathBuf::from);
            let t = Instant::now();
            let g = gtfs::build(&dir)?;
            g.save(&out)?;
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            println!(
                "built {} in {:.1}s ({} KB)\n{}",
                out.display(),
                t.elapsed().as_secs_f32(),
                size / 1024,
                g.summary()
            );
            Ok(())
        }
        Some("export") => {
            let src: PathBuf = args
                .get(1)
                .map_or_else(|| "data/gtfs.json.gz".into(), PathBuf::from);
            let out: PathBuf = args.get(2).map_or_else(|| "dist".into(), PathBuf::from);
            let t = Instant::now();
            let g = gtfs::Gtfs::load(&src)?;
            let zoom: u8 = std::env::var("BUSONES_CELL_ZOOM")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(13);
            gtfs::export(&g, &out, zoom)?;
            println!(
                "exported {} line bundles to {} in {:.1}s",
                g.routes.len(),
                out.display(),
                t.elapsed().as_secs_f32()
            );
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            bail!("gtfs: expected `build` or `export`");
        }
    }
}

fn base_cmd(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("build") => {
            let src: PathBuf = args.get(1).map_or_else(|| "data/rio.pmtiles".into(), PathBuf::from);
            let out: PathBuf = args.get(2).map_or_else(|| "dist/base".into(), PathBuf::from);
            let t = Instant::now();
            let st = basemap::build(&src, &out)?;
            println!(
                "basemap in {:.1}s: z11 {} tiles, z13 {}, z15 {}; {} place names; {:.1} MB raw, {:.1} MB gzip",
                t.elapsed().as_secs_f32(),
                st.tiles[0], st.tiles[1], st.tiles[2], st.places,
                st.bytes as f64 / 1e6, st.gz_bytes as f64 / 1e6
            );
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            bail!("base: expected `build`");
        }
    }
}

/// One-shot measurement of the upstream feeds, for the pre-build checks.
async fn probe() -> Result<()> {
    let cfg = Config::from_env()?;
    let http = http_client()?;
    let client = ingest::its::Client::new(cfg.its_url.clone(), http.clone());
    let minute = timeutil::format_minute_utc(timeutil::now() - 90);

    let t = Instant::now();
    let f = client.fetch(Some(&minute)).await?;
    let elapsed = t.elapsed();
    let now = timeutil::now();

    let mut ages: Vec<i64> = f.fixes.iter().map(|x| now - x.t).collect();
    ages.sort_unstable();
    let pct = |p: f64| -> i64 {
        if ages.is_empty() {
            0
        } else {
            ages[((ages.len() as f64 * p) as usize).min(ages.len() - 1)]
        }
    };
    let mut by_vendor: std::collections::BTreeMap<&str, usize> = Default::default();
    for x in &f.fixes {
        *by_vendor.entry(x.vendor.as_str()).or_default() += 1;
    }
    let vehicles: std::collections::HashSet<&str> =
        f.fixes.iter().map(|x| x.vehicle.as_str()).collect();

    println!("ITS  {minute}");
    println!(
        "  {} rows, {} usable fixes, {} vehicles, {} pages, {:.1} MB in {:.1}s",
        f.rows,
        f.fixes.len(),
        vehicles.len(),
        f.pages,
        f.bytes as f64 / 1e6,
        elapsed.as_secs_f32()
    );
    println!("  by vendor: {by_vendor:?}");
    println!("  providers: {:?}", f.providers);
    println!(
        "  age p10/p50/p90/p99: {}/{}/{}/{} s, newest server second {:?}",
        pct(0.1),
        pct(0.5),
        pct(0.9),
        pct(0.99),
        f.max_server_second
    );

    let t = Instant::now();
    let resp = http.get(&cfg.brt_url).send().await?;
    let body = resp.bytes().await?;
    let ents = ingest::brt::decode_feed(&body);
    let fixes = ingest::brt::to_fixes(ents, now);
    let fresh = fixes.iter().filter(|x| now - x.t <= 300).count();
    println!(
        "BRT  {} entities, {} with a position, {} fresher than 5 min, {} KB in {:.1}s",
        fixes.len(),
        fixes.len(),
        fresh,
        body.len() / 1024,
        t.elapsed().as_secs_f32()
    );
    Ok(())
}

async fn serve() -> Result<()> {
    let cfg = Arc::new(Config::from_env()?);
    let started_at = timeutil::now();

    let t = Instant::now();
    let mut g = gtfs::Gtfs::load(&cfg.gtfs_path)?;
    match std::fs::read(&cfg.aliases_path) {
        Ok(bytes) => {
            g.aliases = serde_json::from_slice(&bytes).unwrap_or_default();
            tracing::info!(count = g.aliases.len(), "service aliases loaded");
        }
        Err(_) => tracing::info!("no service alias table; using zero-padding rules only"),
    }
    tracing::info!("gtfs loaded in {:.1}s: {}", t.elapsed().as_secs_f32(), g.summary());
    let gtfs = Arc::new(g);

    let shared: status::Shared = Arc::new(Mutex::new(status::Status {
        status: "starting".into(),
        started_at,
        warming: true,
        feed_version: gtfs.feed_version.clone(),
        ..Default::default()
    }));
    {
        let addr = cfg.status_addr;
        let state = status::AppState {
            status: shared.clone(),
            geo: Arc::new(geocode::Geocoder::new(http_client()?)),
        };
        tokio::spawn(async move {
            if let Err(e) = status::serve(addr, state).await {
                tracing::error!(error = %e, "status endpoint stopped");
            }
        });
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ingest::Batch>(64);
    let http = http_client()?;
    tokio::spawn(ingest::run_its(
        ingest::its::Client::new(cfg.its_url.clone(), http.clone()),
        tx.clone(),
        cfg.warm_minutes,
    ));
    tokio::spawn(ingest::run_brt(
        http,
        cfg.brt_url.clone(),
        tx.clone(),
        cfg.brt_interval,
    ));
    drop(tx);

    let mut engine = Engine::new(gtfs.clone());
    let mut publisher = Publisher::new(cfg.runtime_dir.clone(), cfg.cell_zoom, &gtfs)?;
    let mut ticker = tokio::time::interval(cfg.publish_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut warming = cfg.warm_minutes > 0;

    tracing::info!(
        runtime = %cfg.runtime_dir.display(),
        cells = publisher.cell_count(),
        "publishing"
    );

    loop {
        tokio::select! {
            batch = rx.recv() => {
                let Some(b) = batch else { break };
                let now = timeutil::now();
                {
                    let mut s = shared.lock().unwrap();
                    let entry = s.sources.entry(b.source.to_string()).or_default();
                    entry.rows = b.rows;
                    entry.bytes = b.bytes;
                    entry.fetch_ms = b.fetch_ms;
                    entry.pages = b.pages;
                    if !b.providers.is_empty() {
                        entry.providers = b.providers.clone();
                    }
                    match &b.error {
                        Some(e) => {
                            entry.state = "error".into();
                            entry.last_error = Some(e.clone());
                            tracing::warn!(source = b.source, error = %e, "fetch failed");
                        }
                        None => {
                            entry.state = "ok".into();
                            entry.last_ok = Some(now);
                            entry.last_error = None;
                        }
                    }
                }
                if b.source == "its" && !b.warming {
                    warming = false;
                }
                if b.error.is_none() {
                    let n = b.fixes.len();
                    engine.apply_batch(b.fixes, now);
                    tracing::debug!(source = b.source, rows = b.rows, fixes = n, "batch applied");
                }
            }
            _ = ticker.tick() => {
                let now = timeutil::now();
                engine.tick(now);
                let outs = engine.snapshot();
                let arrivals = engine.arrivals();

                let mut per_line: std::collections::BTreeMap<&str, usize> = Default::default();
                for o in &outs {
                    *per_line.entry(o.line.as_str()).or_default() += 1;
                }
                let mut lines: Vec<(&str, usize)> = per_line.into_iter().collect();
                lines.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

                let counts = engine.phase_counts();
                let diagnostics = engine.diagnostics();
                let mut unknown: Vec<(&str, usize)> = engine
                    .unknown_services
                    .iter()
                    .map(|(k, n)| (k.as_str(), *n))
                    .collect();
                unknown.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                unknown.truncate(20);
                let newest = engine.newest_fix();
                let sources_json = {
                    let s = shared.lock().unwrap();
                    serde_json::to_value(&s.sources).unwrap_or_default()
                };
                let index = serde_json::json!({
                    "generated_at": now,
                    "warming": warming,
                    "feed_version": gtfs.feed_version,
                    "counts": counts,
                    "pending_reasons": diagnostics,
                    "unknown_services": unknown,
                    "cells": {"z": cfg.cell_zoom, "count": publisher.cell_count()},
                    "arrivals": arrivals.len(),
                    "lines": lines,
                    "sources": sources_json,
                });

                if std::env::var_os("BUSONES_DEBUG_PENDING").is_some() {
                    for line in engine.debug_pending(6) {
                        tracing::info!(target: "pending", "{line}");
                    }
                }

                let t = Instant::now();
                match publisher.publish(&outs, &arrivals, &gtfs, index) {
                    Ok(st) => {
                        let ms = t.elapsed().as_millis() as u64;
                        let disk = status::disk_used_pct(&cfg.runtime_dir);
                        let mut s = shared.lock().unwrap();
                        s.generated_at = now;
                        s.uptime_s = now - started_at;
                        s.warming = warming;
                        s.newest_fix_age_s = if newest == 0 { -1 } else { now - newest };
                        s.fleet_in_service = outs.len();
                        s.vehicles = counts.clone();
                        s.published_files = st.files_written;
                        s.publish_ms = ms;
                        s.disk_used_pct = disk;
                        s.disk = if disk >= 80 { "high".into() } else { "ok".into() };
                        s.status = if newest == 0 {
                            "down".into()
                        } else if now - newest > 120 {
                            "stale".into()
                        } else {
                            "ok".into()
                        };
                        tracing::info!(
                            vehicles = outs.len(),
                            matched = counts.get("matched").copied().unwrap_or(0),
                            live = counts.get("live").copied().unwrap_or(0),
                            pending = counts.get("pending").copied().unwrap_or(0),
                            age = s.newest_fix_age_s,
                            written = st.files_written,
                            of = st.files_total,
                            ms,
                            "published"
                        );
                    }
                    Err(e) => tracing::error!(error = %e, "publish failed"),
                }
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM, shutting down");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("interrupted, shutting down");
                break;
            }
        }
    }
    Ok(())
}
