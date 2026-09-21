//! Polling schedules for the upstream feeds.

pub mod brt;
pub mod its;

use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

use crate::model::Fix;
use crate::timeutil;

pub struct Batch {
    pub source: &'static str,
    pub fixes: Vec<Fix>,
    pub rows: usize,
    pub bytes: usize,
    pub pages: usize,
    pub providers: BTreeMap<String, String>,
    pub error: Option<String>,
    pub warming: bool,
    pub fetch_ms: u64,
}

impl Batch {
    fn failed(source: &'static str, e: anyhow::Error, ms: u64) -> Batch {
        Batch {
            source,
            fixes: Vec::new(),
            rows: 0,
            bytes: 0,
            pages: 0,
            providers: BTreeMap::new(),
            error: Some(format!("{e:#}")),
            warming: false,
            fetch_ms: ms,
        }
    }
}

/// Wall-clock offsets inside the minute, in milliseconds.
///
/// The aggregator's open minute grows in steps of 10 to 15 s, so four reads
/// pick up every step without wasting requests. The read at +18 s is the one
/// that asks for the minute that just closed: before +15 s the endpoint answers
/// with a first-quarter stub and an HTTP 200.
const OFFSETS_MS: [i64; 5] = [8_000, 18_000, 23_000, 38_000, 53_000];
const CLOSED_MINUTE_OFFSET_MS: i64 = 18_000;
const STUB_RETRIES: u32 = 3;
const STUB_RETRY_DELAY: Duration = Duration::from_secs(5);
/// A consolidated minute carries rows right up to its last second.
const CONSOLIDATED_MIN_SECOND: i64 = 45;

pub async fn run_its(client: its::Client, tx: Sender<Batch>, warm_minutes: i64) {
    let mut typical_rows = 4000.0f64;

    if warm_minutes > 0 {
        let now = timeutil::now();
        let current = now - now.rem_euclid(60);
        for k in (1..=warm_minutes).rev() {
            let minute = timeutil::format_minute_utc(current - k * 60);
            let started = std::time::Instant::now();
            match client.fetch(Some(&minute)).await {
                Ok(f) => {
                    tracing::info!(minute = %minute, rows = f.rows, "warm start");
                    if tx.send(batch_from("its", f, started, true)).await.is_err() {
                        return;
                    }
                }
                Err(e) => tracing::warn!(minute = %minute, error = %e, "warm start failed"),
            }
        }
    }

    loop {
        let offset = sleep_to_next_offset().await;
        let started = std::time::Instant::now();
        if offset == CLOSED_MINUTE_OFFSET_MS {
            let now = timeutil::now();
            let minute = timeutil::format_minute_utc(now - 60);
            let mut attempt = 0;
            loop {
                match client.fetch(Some(&minute)).await {
                    Ok(f) => {
                        let complete = f.rows as f64 >= typical_rows * 0.6
                            && f.max_server_second.unwrap_or(0) >= CONSOLIDATED_MIN_SECOND;
                        if complete || attempt >= STUB_RETRIES {
                            if complete {
                                typical_rows = 0.8 * typical_rows + 0.2 * f.rows as f64;
                            } else {
                                tracing::warn!(
                                    minute = %minute,
                                    rows = f.rows,
                                    "giving up on an incomplete closed minute"
                                );
                            }
                            if tx.send(batch_from("its", f, started, false)).await.is_err() {
                                return;
                            }
                            break;
                        }
                        tracing::debug!(
                            minute = %minute,
                            rows = f.rows,
                            attempt,
                            "closed minute still a stub, retrying"
                        );
                        attempt += 1;
                        tokio::time::sleep(STUB_RETRY_DELAY).await;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Batch::failed("its", e, started.elapsed().as_millis() as u64))
                            .await;
                        break;
                    }
                }
            }
        } else {
            match client.fetch(None).await {
                Ok(f) => {
                    if tx.send(batch_from("its", f, started, false)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    if tx
                        .send(Batch::failed("its", e, started.elapsed().as_millis() as u64))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    }
}

fn batch_from(
    source: &'static str,
    f: its::Fetch,
    started: std::time::Instant,
    warming: bool,
) -> Batch {
    Batch {
        source,
        rows: f.rows,
        bytes: f.bytes,
        pages: f.pages,
        providers: f.providers,
        fixes: f.fixes,
        error: None,
        warming,
        fetch_ms: started.elapsed().as_millis() as u64,
    }
}

/// Sleeps until the next scheduled read and returns which slot it is.
async fn sleep_to_next_offset() -> i64 {
    let in_minute = timeutil::now_millis().rem_euclid(60_000);
    let (wait, offset) = match OFFSETS_MS.iter().find(|&&o| o > in_minute + 20) {
        Some(&o) => (o - in_minute, o),
        None => (60_000 - in_minute + OFFSETS_MS[0], OFFSETS_MS[0]),
    };
    tokio::time::sleep(Duration::from_millis(wait.max(0) as u64)).await;
    offset
}

pub async fn run_brt(http: reqwest::Client, url: String, tx: Sender<Batch>, every: Duration) {
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let started = std::time::Instant::now();
        let batch = match fetch_brt(&http, &url).await {
            Ok((fixes, bytes, entities)) => Batch {
                source: "brt",
                fixes,
                rows: entities,
                bytes,
                pages: 1,
                providers: BTreeMap::new(),
                error: None,
                warming: false,
                fetch_ms: started.elapsed().as_millis() as u64,
            },
            Err(e) => Batch::failed("brt", e, started.elapsed().as_millis() as u64),
        };
        if tx.send(batch).await.is_err() {
            return;
        }
    }
}

async fn fetch_brt(
    http: &reqwest::Client,
    url: &str,
) -> anyhow::Result<(Vec<Fix>, usize, usize)> {
    let resp = http.get(url).send().await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if !status.is_success() {
        anyhow::bail!("BRT feed returned {status}");
    }
    let entities = brt::decode_feed(&body);
    let n = entities.len();
    Ok((brt::to_fixes(entities, timeutil::now()), body.len(), n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schedule_covers_the_minute_and_includes_the_closed_read() {
        assert!(OFFSETS_MS.contains(&CLOSED_MINUTE_OFFSET_MS));
        assert!(OFFSETS_MS.windows(2).all(|w| w[0] < w[1]), "must be sorted");
        // Four reads of the open minute plus one of the closed minute.
        assert_eq!(OFFSETS_MS.len(), 5);
        // No two reads closer than the aggregator's own refresh step.
        assert!(OFFSETS_MS.windows(2).all(|w| w[1] - w[0] >= 5_000));
    }
}
