//! Opt-in diagnostics recorder, switched on with `BLADEPHORIA_DIAGNOSTICS=1`.
//!
//! The request middleware, the Discord client, file storage and background
//! tasks report measurements here. A dedicated thread receives them through
//! a bounded channel, so recording never blocks a request: when the channel
//! is full the event is counted as dropped instead. The thread appends every
//! event to `events-YYYY-MM-DD.jsonl` (UTC days) and rewrites
//! `summary-YYYY-MM-DD.md` every few minutes, at midnight and on shutdown.
//! Files older than the retention period are deleted; each day's event file
//! stops growing at a size limit.
//!
//! Nothing secret is written: no query strings, cookies, headers, tokens,
//! request bodies or client addresses - only route labels, timings, sizes,
//! status codes and counters.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE};
use axum::middleware::Next;
use axum::response::Response;
use http_body::{Body as HttpBody, Frame, SizeHint};
use serde_json::{Map, Value, json};
use tokio::time::MissedTickBehavior;

use crate::util::{iso_from_ms, now_ms, parse_http_date, parse_iso_ms};

const CHANNEL_CAPACITY: usize = 16_384;
/// Latency samples kept per metric and day (uniform reservoir beyond that).
const MAX_SAMPLES: usize = 20_000;
const FLUSH_EVERY: Duration = Duration::from_secs(2);
const SLOWEST_KEPT: usize = 10;
/// Lock waits shorter than this only count towards the request breakdown.
const LOCK_EVENT_MIN: Duration = Duration::from_millis(1);
/// Wall clock moving this much against the monotonic clock is a jump.
const CLOCK_JUMP_MS: i64 = 2_000;
/// Difference to the clock of a remote server that counts as skew.
const CLOCK_SKEW_MS: i64 = 3_000;
/// `USER_HZ` - the unit of CPU times in `/proc/self/stat` on Linux.
const CLOCK_TICKS: f64 = 100.0;

#[derive(Clone, Debug)]
pub struct DiagConfig {
    pub enabled: bool,
    pub dir: PathBuf,
    pub retention_days: u32,
    pub max_mb_per_day: u64,
    pub summary_every: Duration,
}

// ---------------------------------------------------------------- recorder

enum Msg {
    Event(Value),
    Shutdown,
}

struct Recorder {
    sender: SyncSender<Msg>,
    dropped: AtomicU64,
    in_flight: AtomicI64,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Last report time per throttled anomaly.
    throttle: Mutex<HashMap<String, Instant>>,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

pub fn enabled() -> bool {
    RECORDER.get().is_some()
}

/// Starts the writer thread. Returns `false` when diagnostics are disabled.
pub fn start(cfg: &DiagConfig) -> std::io::Result<bool> {
    if !cfg.enabled || enabled() {
        return Ok(false);
    }
    fs::create_dir_all(&cfg.dir)?;
    let (sender, receiver) = sync_channel(CHANNEL_CAPACITY);
    let writer = Writer::new(cfg.clone());
    let worker = std::thread::Builder::new()
        .name("diagnostics".to_owned())
        .spawn(move || writer.run(receiver))?;
    let _ = RECORDER.set(Recorder {
        sender,
        dropped: AtomicU64::new(0),
        in_flight: AtomicI64::new(0),
        worker: Mutex::new(Some(worker)),
        throttle: Mutex::default(),
    });
    Ok(true)
}

/// Writes the final summary and waits for the writer thread to finish.
pub fn shutdown() {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    let _ = recorder.sender.send(Msg::Shutdown);
    let worker = recorder
        .worker
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(worker) = worker {
        let _ = worker.join();
    }
}

fn dropped_events() -> u64 {
    RECORDER
        .get()
        .map_or(0, |recorder| recorder.dropped.load(Ordering::Relaxed))
}

/// Queues an event without ever waiting; a full queue drops it.
fn offer(sender: &SyncSender<Msg>, dropped: &AtomicU64, msg: Msg) {
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = sender.try_send(msg) {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn emit(kind: &'static str, fields: Value) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    let ts = now_ms();
    let mut event = Map::new();
    event.insert("ts".to_owned(), json!(iso_from_ms(ts)));
    event.insert("t_ms".to_owned(), json!(ts));
    event.insert("kind".to_owned(), json!(kind));
    if let Value::Object(fields) = fields {
        event.extend(fields);
    }
    offer(
        &recorder.sender,
        &recorder.dropped,
        Msg::Event(Value::Object(event)),
    );
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn round(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

// ---------------------------------------------------- per-request breakdown

/// Time a request spent in Discord calls, file I/O and lock waits.
#[derive(Default)]
pub struct Spans {
    discord_us: AtomicU64,
    discord_calls: AtomicU64,
    io_us: AtomicU64,
    lock_us: AtomicU64,
}

impl Spans {
    fn ms(counter: &AtomicU64) -> f64 {
        round(counter.load(Ordering::Relaxed) as f64 / 1000.0, 3)
    }
}

tokio::task_local! {
    static SPANS: Arc<Spans>;
}

fn add_span(update: impl FnOnce(&Spans)) {
    let _ = SPANS.try_with(|spans| update(spans));
}

/// The breakdown of the current request, to carry into spawned tasks.
pub fn current_spans() -> Option<Arc<Spans>> {
    SPANS.try_with(Arc::clone).ok()
}

/// Runs `future` so that its measurements count towards `spans`.
pub async fn with_spans<F: Future>(spans: Option<Arc<Spans>>, future: F) -> F::Output {
    match spans {
        Some(spans) => SPANS.scope(spans, future).await,
        None => future.await,
    }
}

// ------------------------------------------------------------ measurements

pub struct DiscordCall<'a> {
    pub endpoint: &'static str,
    /// HTTP status, 0 when no response arrived.
    pub status: u16,
    /// Until the response headers arrived.
    pub ttfb: Duration,
    /// Until the whole body was read.
    pub total: Duration,
    pub bytes: usize,
    pub error: Option<&'static str>,
    pub headers: Option<&'a reqwest::header::HeaderMap>,
}

pub fn discord(call: DiscordCall<'_>) {
    add_span(|spans| {
        spans
            .discord_us
            .fetch_add(micros(call.total), Ordering::Relaxed);
        spans.discord_calls.fetch_add(1, Ordering::Relaxed);
    });
    if !enabled() {
        return;
    }

    let header = |name: &str| {
        call.headers
            .and_then(|headers| headers.get(name))
            .and_then(|value| value.to_str().ok())
    };
    let number = |name: &str| header(name).and_then(|value| value.trim().parse::<f64>().ok());
    let retry_after = number("retry-after");

    emit(
        "discord",
        json!({
            "endpoint": call.endpoint,
            "status": call.status,
            "ttfb_ms": ms(call.ttfb),
            "total_ms": ms(call.total),
            "bytes": call.bytes,
            "error": call.error,
            "ratelimit_remaining": number("x-ratelimit-remaining"),
            "ratelimit_reset_after_s": number("x-ratelimit-reset-after"),
            "retry_after_s": retry_after,
            "ratelimit_scope": header("x-ratelimit-scope"),
            "ratelimit_global": header("x-ratelimit-global") == Some("true"),
        }),
    );

    if call.status == 429 {
        anomaly(
            "rate_limited",
            format!(
                "Discord {} answered 429, retry after {} s",
                call.endpoint,
                retry_after.map_or_else(|| "?".to_owned(), |value| value.to_string())
            ),
        );
    }
}

/// Compares the `Date` header of a remote server with the local clock.
pub fn remote_clock(source: &'static str, date_header: Option<&str>) {
    if !enabled() {
        return;
    }
    let Some(remote) = date_header.and_then(parse_http_date) else {
        return;
    };
    // The header has 1 s resolution; anything within the threshold is noise.
    let skew = now_ms() - remote;
    if skew.abs() > CLOCK_SKEW_MS {
        anomaly_throttled(
            "clock_skew",
            source,
            format!("local clock differs from {source} by {skew:+} ms"),
            Duration::from_secs(3600),
        );
    }
}

/// A read or write of a data file. Only the file name is recorded.
pub fn io(op: &'static str, path: &Path, elapsed: Duration, bytes: usize, ok: bool) {
    add_span(|spans| {
        spans.io_us.fetch_add(micros(elapsed), Ordering::Relaxed);
    });
    if !enabled() {
        return;
    }
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    emit(
        "io",
        json!({ "op": op, "file": file, "ms": ms(elapsed), "bytes": bytes, "ok": ok }),
    );
}

/// Locks `mutex`, recording how long the wait took.
pub async fn timed_lock<'a, T>(
    name: &'static str,
    mutex: &'a tokio::sync::Mutex<T>,
) -> tokio::sync::MutexGuard<'a, T> {
    if !enabled() {
        return mutex.lock().await;
    }
    let started = Instant::now();
    let guard = mutex.lock().await;
    let waited = started.elapsed();
    add_span(|spans| {
        spans.lock_us.fetch_add(micros(waited), Ordering::Relaxed);
    });
    if waited >= LOCK_EVENT_MIN {
        emit("lock", json!({ "name": name, "wait_ms": ms(waited) }));
    }
    guard
}

/// One run of a periodic task: how late it started and how long it worked.
pub fn tick(task: &'static str, late: Duration, work: Duration) {
    if enabled() {
        emit(
            "tick",
            json!({ "task": task, "late_ms": ms(late), "work_ms": ms(work) }),
        );
    }
}

pub fn anomaly(kind: &'static str, detail: impl Into<String>) {
    if enabled() {
        emit("anomaly", json!({ "type": kind, "detail": detail.into() }));
    }
}

/// Like [`anomaly`], but reported at most once per `every` for each `key`.
pub fn anomaly_throttled(
    kind: &'static str,
    key: &str,
    detail: impl Into<String>,
    every: Duration,
) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    {
        let mut throttle = recorder
            .throttle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let slot = format!("{kind}/{key}");
        if throttle
            .get(&slot)
            .is_some_and(|last| last.elapsed() < every)
        {
            return;
        }
        throttle.insert(slot, Instant::now());
    }
    anomaly(kind, detail);
}

pub fn lifecycle(phase: &'static str, detail: Value) {
    if enabled() {
        emit("lifecycle", json!({ "phase": phase, "detail": detail }));
    }
}

// --------------------------------------------------------- HTTP middleware

/// Decrements the in-flight counter however the request ends.
struct InFlight;

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(recorder) = RECORDER.get() {
            recorder.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

struct Pending {
    method: String,
    route: String,
    status: u16,
    encoding: &'static str,
    accepts_compression: bool,
    content: &'static str,
    started: Instant,
    ttfb: Duration,
    in_flight: i64,
    spans: Arc<Spans>,
    _in_flight: InFlight,
}

/// Groups request paths so that the reports stay readable: profile slugs
/// and media files are folded together and every 404 lands in one bucket
/// (scanners probe thousands of different paths).
fn route_label(path: &str, status: u16) -> String {
    match status {
        404 => return "(404)".to_owned(),
        405 => return "(405)".to_owned(),
        _ => {}
    }
    let label = if let Some(rest) = path.strip_prefix("/profile/") {
        if rest.is_empty() || rest == "index.html" {
            "/profile/"
        } else {
            "/profile/:slug"
        }
    } else if path.starts_with("/media/") {
        "/media/*"
    } else {
        path
    };
    label.chars().take(80).collect()
}

fn content_class(content_type: Option<&str>) -> &'static str {
    let Some(content_type) = content_type else {
        return "none";
    };
    let mime = content_type.split(';').next().unwrap_or("").trim();
    match mime {
        "text/html" => "html",
        "text/javascript" | "application/javascript" => "js",
        "text/css" => "css",
        "application/json" => "json",
        "image/svg+xml" => "svg",
        _ if mime.starts_with("text/") => "text",
        _ if mime.starts_with("image/") => "image",
        _ if mime.starts_with("video/") => "video",
        _ => "other",
    }
}

fn is_text(content: &str) -> bool {
    matches!(content, "html" | "js" | "css" | "json" | "svg" | "text")
}

/// Outermost layer: measures every request until its body is fully sent.
pub async fn track_requests(request: Request, next: Next) -> Response {
    let Some(recorder) = RECORDER.get() else {
        return next.run(request).await;
    };
    let started = Instant::now();
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let accepts_compression = request
        .headers()
        .get(ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("br") || value.contains("gzip"));
    let in_flight = recorder.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
    let guard = InFlight;

    let spans = Arc::new(Spans::default());
    let response = SPANS.scope(spans.clone(), next.run(request)).await;
    let ttfb = started.elapsed();

    let headers = response.headers();
    let encoding = match headers
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
    {
        None => "identity",
        Some("br") => "br",
        Some("gzip") => "gzip",
        Some(_) => "other",
    };
    let content = content_class(
        headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
    );
    let status = response.status().as_u16();

    let pending = Pending {
        method,
        route: route_label(&path, status),
        status,
        encoding,
        accepts_compression,
        content,
        started,
        ttfb,
        in_flight,
        spans,
        _in_flight: guard,
    };
    response.map(|body| {
        Body::new(MeteredBody {
            expected: body.size_hint().exact(),
            inner: body,
            bytes: 0,
            ended: false,
            pending: Some(pending),
        })
    })
}

/// Response body wrapper that counts the bytes sent and records the request
/// once the body is finished (or dropped, when the client went away).
struct MeteredBody {
    inner: Body,
    /// Exact body length when known up front (files, fixed responses).
    expected: Option<u64>,
    bytes: u64,
    ended: bool,
    pending: Option<Pending>,
}

impl MeteredBody {
    fn finish(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        // Hyper stops polling once a body of known length is complete, so
        // reaching that length counts as finished too.
        let complete = self.ended
            || self.inner.is_end_stream()
            || self.expected.is_some_and(|expected| self.bytes >= expected);
        let total = pending.started.elapsed();
        let spans = &pending.spans;
        emit(
            "http",
            json!({
                "method": pending.method,
                "route": pending.route,
                "status": pending.status,
                "ttfb_ms": ms(pending.ttfb),
                "total_ms": ms(total),
                "send_ms": ms(total.saturating_sub(pending.ttfb)),
                "bytes": self.bytes,
                "encoding": pending.encoding,
                "content": pending.content,
                "in_flight": pending.in_flight,
                "accepts_compression": pending.accepts_compression,
                "aborted": !complete,
                "discord_ms": Spans::ms(&spans.discord_us),
                "discord_calls": spans.discord_calls.load(Ordering::Relaxed),
                "io_ms": Spans::ms(&spans.io_us),
                "lock_ms": Spans::ms(&spans.lock_us),
            }),
        );
    }
}

impl HttpBody for MeteredBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.bytes += data.len() as u64;
                }
            }
            Poll::Ready(None) => {
                this.ended = true;
                this.finish();
            }
            Poll::Ready(Some(Err(_))) => this.finish(),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for MeteredBody {
    fn drop(&mut self) {
        self.finish();
    }
}

// ------------------------------------------------------ background samplers

pub struct AppCounts {
    pub sessions: usize,
    pub login_states: usize,
    pub invite_cache: usize,
}

#[derive(Default)]
struct Lag {
    max_us: AtomicU64,
    sum_us: AtomicU64,
    samples: AtomicU64,
}

/// Starts the runtime-lag watcher (every second) and the process sampler
/// (every minute).
pub fn spawn_samplers<F>(counts: F)
where
    F: Fn() -> AppCounts + Send + Sync + 'static,
{
    if !enabled() {
        return;
    }
    let lag = Arc::new(Lag::default());
    tokio::spawn(watch_runtime(lag.clone()));
    tokio::spawn(sample_resources(lag, counts));
}

/// How late a 1 s timer fires shows whether something blocks the runtime
/// threads; comparing wall and monotonic time reveals clock jumps.
async fn watch_runtime(lag: Arc<Lag>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous: Option<(Instant, i64)> = None;
    loop {
        let scheduled = interval.tick().await;
        let late = micros(tokio::time::Instant::now().saturating_duration_since(scheduled));
        lag.max_us.fetch_max(late, Ordering::Relaxed);
        lag.sum_us.fetch_add(late, Ordering::Relaxed);
        lag.samples.fetch_add(1, Ordering::Relaxed);

        let (mono, wall) = (Instant::now(), now_ms());
        if let Some((previous_mono, previous_wall)) = previous {
            let elapsed =
                i64::try_from(mono.duration_since(previous_mono).as_millis()).unwrap_or(i64::MAX);
            let jump = (wall - previous_wall) - elapsed;
            if jump.abs() > CLOCK_JUMP_MS {
                anomaly(
                    "clock_jump",
                    format!("system clock moved {jump:+} ms against the monotonic clock"),
                );
            }
        }
        previous = Some((mono, wall));
    }
}

#[derive(Default)]
struct ProcSample {
    rss_kb: Option<u64>,
    peak_kb: Option<u64>,
    threads: Option<u64>,
    fds: Option<usize>,
    children: Option<usize>,
    cpu_ticks: Option<u64>,
}

fn read_proc() -> ProcSample {
    let mut sample = ProcSample::default();
    if let Ok(status) = fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            let value = |prefix: &str| {
                line.strip_prefix(prefix)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|value| value.parse().ok())
            };
            sample.rss_kb = sample.rss_kb.or_else(|| value("VmRSS:"));
            sample.peak_kb = sample.peak_kb.or_else(|| value("VmHWM:"));
            sample.threads = sample.threads.or_else(|| value("Threads:"));
        }
    }
    sample.fds = fs::read_dir("/proc/self/fd").ok().map(Iterator::count);
    sample.children = fs::read_dir("/proc/self/task").ok().map(|tasks| {
        tasks
            .flatten()
            .filter_map(|task| fs::read_to_string(task.path().join("children")).ok())
            .map(|children| children.split_whitespace().count())
            .sum()
    });
    // Fields after the parenthesised command name: utime is the 12th, stime the 13th.
    sample.cpu_ticks = fs::read_to_string("/proc/self/stat").ok().and_then(|stat| {
        let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
    });
    sample
}

async fn sample_resources<F: Fn() -> AppCounts>(lag: Arc<Lag>, counts: F) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous_cpu: Option<(u64, Instant)> = None;
    loop {
        interval.tick().await;
        let sample = read_proc();
        let now = Instant::now();
        let cpu_pct = match (previous_cpu, sample.cpu_ticks) {
            (Some((ticks_before, at)), Some(ticks)) => {
                let seconds = now.duration_since(at).as_secs_f64();
                (seconds > 0.0).then(|| {
                    round(
                        ticks.saturating_sub(ticks_before) as f64 / CLOCK_TICKS / seconds * 100.0,
                        2,
                    )
                })
            }
            _ => None,
        };
        previous_cpu = sample.cpu_ticks.map(|ticks| (ticks, now));

        let samples = lag.samples.swap(0, Ordering::Relaxed);
        let lag_sum = lag.sum_us.swap(0, Ordering::Relaxed);
        let lag_max = lag.max_us.swap(0, Ordering::Relaxed);
        let counts = counts();
        emit(
            "resources",
            json!({
                "rss_kb": sample.rss_kb,
                "rss_peak_kb": sample.peak_kb,
                "threads": sample.threads,
                "fds": sample.fds,
                "children": sample.children,
                "cpu_pct": cpu_pct,
                "cpu_total_s": sample.cpu_ticks.map(|ticks| ticks as f64 / CLOCK_TICKS),
                "runtime_lag_max_ms": round(lag_max as f64 / 1000.0, 3),
                "runtime_lag_avg_ms": if samples == 0 { 0.0 } else { round(lag_sum as f64 / samples as f64 / 1000.0, 3) },
                "sessions": counts.sessions,
                "login_states": counts.login_states,
                "invite_cache": counts.invite_cache,
                "in_flight": RECORDER.get().map_or(0, |recorder| recorder.in_flight.load(Ordering::Relaxed)),
                "dropped_events": dropped_events(),
            }),
        );
    }
}

// ------------------------------------------------------------------ writer

fn day_of(ts_ms: i64) -> String {
    iso_from_ms(ts_ms)[..10].to_owned()
}

struct Writer {
    cfg: DiagConfig,
    day: String,
    file: Option<BufWriter<File>>,
    bytes_today: u64,
    limit_noted: bool,
    suppressed: u64,
    events_today: u64,
    stats: Stats,
    window: Stats,
    window_from_ms: i64,
}

impl Writer {
    fn new(cfg: DiagConfig) -> Self {
        Self {
            cfg,
            day: String::new(),
            file: None,
            bytes_today: 0,
            limit_noted: false,
            suppressed: 0,
            events_today: 0,
            stats: Stats::default(),
            window: Stats::default(),
            window_from_ms: now_ms(),
        }
    }

    fn run(mut self, receiver: Receiver<Msg>) {
        self.open_day(&day_of(now_ms()));
        let mut next_summary = Instant::now() + self.cfg.summary_every;
        let mut next_flush = Instant::now() + FLUSH_EVERY;
        loop {
            let wake = next_summary.min(next_flush);
            match receiver.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(Msg::Event(event)) => self.handle(event),
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    while let Ok(Msg::Event(event)) = receiver.try_recv() {
                        self.handle(event);
                    }
                    self.write_summary(true);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if now >= next_flush {
                self.flush();
                next_flush = now + FLUSH_EVERY;
            }
            if now >= next_summary {
                self.write_summary(false);
                next_summary = now + self.cfg.summary_every;
            }
        }
    }

    fn events_path(&self, day: &str) -> PathBuf {
        self.cfg.dir.join(format!("events-{day}.jsonl"))
    }

    /// Switches to the files of `day`. Statistics are rebuilt from the
    /// day's event file, so a restart does not reset the daily report.
    fn open_day(&mut self, day: &str) {
        self.flush();
        self.day = day.to_owned();
        self.stats = Stats::default();
        self.events_today = 0;
        self.suppressed = 0;
        self.limit_noted = false;

        let path = self.events_path(day);
        if let Ok(file) = File::open(&path) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if event["kind"] == "limit_reached" {
                    self.limit_noted = true;
                }
                self.stats.record(&event);
                self.events_today += 1;
            }
        }
        self.bytes_today = fs::metadata(&path).map_or(0, |meta| meta.len());
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()
            .map(BufWriter::new);
        prune(&self.cfg.dir, self.cfg.retention_days, day);
    }

    fn handle(&mut self, event: Value) {
        let ts = event["t_ms"].as_i64().unwrap_or_else(now_ms);
        let day = day_of(ts);
        // Events from different threads may arrive slightly out of order
        // around midnight; only ever move forward.
        if day > self.day {
            self.write_summary(true);
            self.open_day(&day);
            self.window = Stats::default();
            self.window_from_ms = ts;
        }
        self.stats.record(&event);
        self.window.record(&event);
        self.events_today += 1;
        self.write_line(&event);
    }

    fn write_line(&mut self, event: &Value) {
        let mut line = event.to_string();
        line.push('\n');
        let limit = self.cfg.max_mb_per_day.saturating_mul(1024 * 1024);
        if limit > 0 && self.bytes_today + line.len() as u64 > limit {
            self.suppressed += 1;
            if !self.limit_noted {
                self.limit_noted = true;
                let ts = now_ms();
                let note = json!({
                    "ts": iso_from_ms(ts),
                    "t_ms": ts,
                    "kind": "limit_reached",
                    "max_mb_per_day": self.cfg.max_mb_per_day,
                });
                self.append(&format!("{note}\n"));
            }
            return;
        }
        self.append(&line);
    }

    fn append(&mut self, line: &str) {
        if let Some(file) = self.file.as_mut()
            && file.write_all(line.as_bytes()).is_ok()
        {
            self.bytes_today += line.len() as u64;
        }
    }

    fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }

    fn write_summary(&mut self, final_report: bool) {
        if self.day.is_empty() {
            return;
        }
        self.flush();
        let report = render_summary(&Report {
            day: &self.day,
            stats: &self.stats,
            window: &self.window,
            window_from_ms: self.window_from_ms,
            now_ms: now_ms(),
            events_today: self.events_today,
            bytes_today: self.bytes_today,
            dropped: dropped_events(),
            suppressed: self.suppressed,
            final_report,
            cfg: &self.cfg,
        });
        let path = self.cfg.dir.join(format!("summary-{}.md", self.day));
        let temp = self.cfg.dir.join(format!(".summary-{}.md.tmp", self.day));
        if fs::write(&temp, report).is_ok() {
            let _ = fs::rename(&temp, &path);
        }
        if !final_report {
            self.window = Stats::default();
            self.window_from_ms = now_ms();
        }
    }
}

/// Deletes event and summary files older than `retention_days` days.
/// Nothing else in the directory is ever touched.
fn prune(dir: &Path, retention_days: u32, today: &str) {
    if retention_days == 0 {
        return;
    }
    let (Some(today_ms), Ok(entries)) = (parse_iso_ms(today), fs::read_dir(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let date = name
            .strip_prefix("events-")
            .and_then(|rest| rest.strip_suffix(".jsonl"))
            .or_else(|| {
                name.strip_prefix("summary-")
                    .and_then(|rest| rest.strip_suffix(".md"))
            });
        let Some(file_ms) = date.filter(|date| date.len() == 10).and_then(parse_iso_ms) else {
            continue;
        };
        if (today_ms - file_ms) / 86_400_000 >= i64::from(retention_days) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// -------------------------------------------------------------- statistics

#[derive(Default)]
struct Samples {
    values: Vec<f32>,
    seen: u64,
    max: f64,
    sum: f64,
    seed: u64,
}

impl Samples {
    fn push(&mut self, value: f64) {
        self.seen += 1;
        self.sum += value;
        if value > self.max {
            self.max = value;
        }
        if self.values.len() < MAX_SAMPLES {
            self.values.push(value as f32);
            return;
        }
        // Reservoir sampling keeps a uniform sample of everything seen.
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let slot = (self.seed >> 33) % self.seen;
        if let Ok(slot) = usize::try_from(slot)
            && slot < MAX_SAMPLES
        {
            self.values[slot] = value as f32;
        }
    }

    /// Nearest-rank percentiles.
    fn percentiles<const N: usize>(&self, points: [f64; N]) -> [f64; N] {
        let mut result = [0.0; N];
        if self.values.is_empty() {
            return result;
        }
        let mut sorted = self.values.clone();
        sorted.sort_by(f32::total_cmp);
        for (slot, point) in result.iter_mut().zip(points) {
            let rank = ((point / 100.0) * sorted.len() as f64).ceil() as usize;
            *slot = f64::from(sorted[rank.clamp(1, sorted.len()) - 1]);
        }
        result
    }

    fn mean(&self) -> f64 {
        if self.seen == 0 {
            0.0
        } else {
            self.sum / self.seen as f64
        }
    }
}

#[derive(Default)]
struct RouteStats {
    latency: Samples,
    ttfb_sum: f64,
    bytes: u64,
    by_class: [u64; 5],
    not_modified: u64,
    aborted: u64,
    discord_sum: f64,
    io_sum: f64,
    lock_sum: f64,
    text_large: u64,
    text_compressed: u64,
}

struct Slow {
    total_ms: f64,
    ts: i64,
    route: String,
    status: u64,
    ttfb_ms: f64,
    discord_ms: f64,
}

#[derive(Default)]
struct DiscordStats {
    latency: Samples,
    statuses: BTreeMap<u64, u64>,
    rate_limited: u64,
    errors: u64,
    min_remaining: Option<f64>,
}

#[derive(Default)]
struct IoStats {
    latency: Samples,
    errors: u64,
    bytes: u64,
}

#[derive(Default)]
struct TickStats {
    count: u64,
    late_max: f64,
    late_over_1s: u64,
    work_max: f64,
}

#[derive(Default)]
struct ResourceStats {
    samples: u64,
    first_rss_kb: Option<f64>,
    last: Option<Value>,
    rss_max_kb: f64,
    fds_max: f64,
    threads_max: f64,
    children_max: f64,
    lag_max_ms: f64,
    cpu_sum: f64,
    cpu_samples: u64,
}

#[derive(Default)]
struct AnomalyStats {
    count: u64,
    last_ts: i64,
    last_detail: String,
}

#[derive(Default)]
struct Stats {
    http_total: u64,
    routes: BTreeMap<String, RouteStats>,
    slowest: Vec<Slow>,
    discord: BTreeMap<String, DiscordStats>,
    io: BTreeMap<String, IoStats>,
    locks: BTreeMap<String, Samples>,
    ticks: BTreeMap<String, TickStats>,
    resources: ResourceStats,
    anomalies: BTreeMap<String, AnomalyStats>,
}

impl Stats {
    fn record(&mut self, event: &Value) {
        let number = |key: &str| event.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let text = |key: &str| {
            event
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let ts = event.get("t_ms").and_then(Value::as_i64).unwrap_or(0);

        match event.get("kind").and_then(Value::as_str).unwrap_or("") {
            "http" => {
                let status = event.get("status").and_then(Value::as_u64).unwrap_or(0);
                let total = number("total_ms");
                let route_name = text("route");
                let route = self.routes.entry(route_name.clone()).or_default();
                route.latency.push(total);
                route.ttfb_sum += number("ttfb_ms");
                route.bytes += event.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                if let Some(slot) = usize::try_from(status / 100)
                    .ok()
                    .filter(|class| (1..=5).contains(class))
                {
                    route.by_class[slot - 1] += 1;
                }
                if status == 304 {
                    route.not_modified += 1;
                }
                if event["aborted"] == true {
                    route.aborted += 1;
                }
                route.discord_sum += number("discord_ms");
                route.io_sum += number("io_ms");
                route.lock_sum += number("lock_ms");
                if status == 200
                    && event["accepts_compression"] == true
                    && is_text(&text("content"))
                    && number("bytes") > 1024.0
                {
                    route.text_large += 1;
                    if matches!(text("encoding").as_str(), "br" | "gzip") {
                        route.text_compressed += 1;
                    }
                }
                self.http_total += 1;
                self.push_slow(Slow {
                    total_ms: total,
                    ts,
                    route: route_name,
                    status,
                    ttfb_ms: number("ttfb_ms"),
                    discord_ms: number("discord_ms"),
                });
            }
            "discord" => {
                let entry = self.discord.entry(text("endpoint")).or_default();
                entry.latency.push(number("total_ms"));
                let status = event.get("status").and_then(Value::as_u64).unwrap_or(0);
                *entry.statuses.entry(status).or_default() += 1;
                if status == 429 {
                    entry.rate_limited += 1;
                }
                if status == 0 || !event["error"].is_null() {
                    entry.errors += 1;
                }
                if let Some(remaining) = event.get("ratelimit_remaining").and_then(Value::as_f64) {
                    entry.min_remaining = Some(
                        entry
                            .min_remaining
                            .map_or(remaining, |min| min.min(remaining)),
                    );
                }
            }
            "io" => {
                let entry = self
                    .io
                    .entry(format!("{} {}", text("op"), text("file")))
                    .or_default();
                entry.latency.push(number("ms"));
                entry.bytes += event.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                if event["ok"] == false {
                    entry.errors += 1;
                }
            }
            "lock" => self
                .locks
                .entry(text("name"))
                .or_default()
                .push(number("wait_ms")),
            "tick" => {
                let entry = self.ticks.entry(text("task")).or_default();
                entry.count += 1;
                let late = number("late_ms");
                entry.late_max = entry.late_max.max(late);
                if late > 1000.0 {
                    entry.late_over_1s += 1;
                }
                entry.work_max = entry.work_max.max(number("work_ms"));
            }
            "resources" => {
                let resources = &mut self.resources;
                resources.samples += 1;
                let rss = number("rss_kb");
                if resources.first_rss_kb.is_none() && rss > 0.0 {
                    resources.first_rss_kb = Some(rss);
                }
                resources.rss_max_kb = resources.rss_max_kb.max(rss);
                resources.fds_max = resources.fds_max.max(number("fds"));
                resources.threads_max = resources.threads_max.max(number("threads"));
                resources.children_max = resources.children_max.max(number("children"));
                resources.lag_max_ms = resources.lag_max_ms.max(number("runtime_lag_max_ms"));
                if let Some(cpu) = event.get("cpu_pct").and_then(Value::as_f64) {
                    resources.cpu_sum += cpu;
                    resources.cpu_samples += 1;
                }
                resources.last = Some(event.clone());
            }
            "anomaly" => {
                let entry = self.anomalies.entry(text("type")).or_default();
                entry.count += 1;
                entry.last_ts = ts;
                entry.last_detail = text("detail");
            }
            _ => {}
        }
    }

    fn push_slow(&mut self, slow: Slow) {
        if self.slowest.len() == SLOWEST_KEPT
            && self
                .slowest
                .last()
                .is_some_and(|fastest| fastest.total_ms >= slow.total_ms)
        {
            return;
        }
        let index = self
            .slowest
            .partition_point(|kept| kept.total_ms >= slow.total_ms);
        self.slowest.insert(index, slow);
        self.slowest.truncate(SLOWEST_KEPT);
    }
}

// ----------------------------------------------------------------- summary

struct Report<'a> {
    day: &'a str,
    stats: &'a Stats,
    window: &'a Stats,
    window_from_ms: i64,
    now_ms: i64,
    events_today: u64,
    bytes_today: u64,
    dropped: u64,
    suppressed: u64,
    final_report: bool,
    cfg: &'a DiagConfig,
}

fn fmt_ms(value: f64) -> String {
    if value < 10.0 {
        format!("{value:.2}")
    } else if value < 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    }
}

fn fmt_bytes(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    }
}

fn time_of(ts: i64) -> String {
    iso_from_ms(ts)[11..19].to_owned()
}

/// Markdown table cell: no pipes or line breaks.
fn cell(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|ch| {
            if ch == '|' || ch.is_control() {
                ' '
            } else {
                ch
            }
        })
        .collect();
    flat.chars().take(160).collect()
}

/// Routes ordered by request count, busiest first.
fn by_count(routes: &BTreeMap<String, RouteStats>) -> Vec<(&String, &RouteStats)> {
    let mut list: Vec<(&String, &RouteStats)> = routes.iter().collect();
    list.sort_by(|a, b| b.1.latency.seen.cmp(&a.1.latency.seen).then(a.0.cmp(b.0)));
    list
}

fn render_summary(report: &Report<'_>) -> String {
    let stats = report.stats;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Diagnostyka bladephoria-web — {} (UTC)\n",
        report.day
    );
    let _ = writeln!(
        out,
        "- Wygenerowano: {}{}",
        iso_from_ms(report.now_ms),
        if report.final_report {
            " (raport końcowy: zamknięcie serwera albo koniec dnia)"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "- Raport odświeża się co {} min; wszystkie zdarzenia: `events-{}.jsonl`",
        report.cfg.summary_every.as_secs() / 60,
        report.day
    );
    let _ = writeln!(
        out,
        "- Zdarzeń dziś: {} ({} z limitu {} MB), żądań HTTP: {}",
        report.events_today,
        fmt_bytes(report.bytes_today as f64),
        report.cfg.max_mb_per_day,
        stats.http_total
    );
    let _ = writeln!(
        out,
        "- Pominięte zdarzenia: {} (pełna kolejka), {} (limit rozmiaru)\n",
        report.dropped, report.suppressed
    );

    let _ = writeln!(
        out,
        "## Ostatnie {} min (od {} UTC)\n",
        report.cfg.summary_every.as_secs() / 60,
        time_of(report.window_from_ms)
    );
    if report.window.routes.is_empty() {
        let _ = writeln!(out, "_Brak żądań._\n");
    } else {
        let _ = writeln!(
            out,
            "| Trasa | Żądań | p50 ms | p99 ms | max ms | 4xx | 5xx |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
        for (name, route) in by_count(&report.window.routes) {
            let [p50, p99] = route.latency.percentiles([50.0, 99.0]);
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} | {} |",
                cell(name),
                route.latency.seen,
                fmt_ms(p50),
                fmt_ms(p99),
                fmt_ms(route.latency.max),
                route.by_class[3],
                route.by_class[4]
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Czas odpowiedzi — cały dzień\n");
    if stats.routes.is_empty() {
        let _ = writeln!(out, "_Brak żądań._\n");
    } else {
        let _ = writeln!(
            out,
            "| Trasa | Żądań | p50 ms | p90 ms | p99 ms | max ms | TTFB śr. ms | Śr. rozmiar | 2xx | 3xx (304) | 4xx | 5xx | Kompresja | Przerwane |"
        );
        let _ = writeln!(
            out,
            "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
        );
        for (name, route) in by_count(&stats.routes) {
            let count = route.latency.seen.max(1) as f64;
            let [p50, p90, p99] = route.latency.percentiles([50.0, 90.0, 99.0]);
            let compression = if route.text_large == 0 {
                "—".to_owned()
            } else {
                format!(
                    "{:.0}%",
                    route.text_compressed as f64 / route.text_large as f64 * 100.0
                )
            };
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} ({}) | {} | {} | {} | {} |",
                cell(name),
                route.latency.seen,
                fmt_ms(p50),
                fmt_ms(p90),
                fmt_ms(p99),
                fmt_ms(route.latency.max),
                fmt_ms(route.ttfb_sum / count),
                fmt_bytes(route.bytes as f64 / count),
                route.by_class[1],
                route.by_class[2],
                route.not_modified,
                route.by_class[3],
                route.by_class[4],
                compression,
                route.aborted
            );
        }
        out.push('\n');

        let _ = writeln!(out, "## Gdzie idzie czas — średnio na żądanie (ms)\n");
        let _ = writeln!(
            out,
            "| Trasa | Całość | Discord | Pliki | Blokady | Reszta (serwer + wysyłka) |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
        for (name, route) in by_count(&stats.routes) {
            let count = route.latency.seen.max(1) as f64;
            let total = route.latency.mean();
            let (discord, io, lock) = (
                route.discord_sum / count,
                route.io_sum / count,
                route.lock_sum / count,
            );
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} |",
                cell(name),
                fmt_ms(total),
                fmt_ms(discord),
                fmt_ms(io),
                fmt_ms(lock),
                fmt_ms((total - discord - io - lock).max(0.0))
            );
        }
        let _ = writeln!(
            out,
            "\nDiscord to suma wywołań — przy równoległych zapytaniach może przekroczyć całość.\n"
        );

        let _ = writeln!(out, "## Najwolniejsze żądania\n");
        let _ = writeln!(
            out,
            "| Czas (UTC) | Trasa | Status | Całość ms | TTFB ms | Discord ms |"
        );
        let _ = writeln!(out, "|---|---|---:|---:|---:|---:|");
        for slow in &stats.slowest {
            let _ = writeln!(
                out,
                "| {} | `{}` | {} | {} | {} | {} |",
                time_of(slow.ts),
                cell(&slow.route),
                slow.status,
                fmt_ms(slow.total_ms),
                fmt_ms(slow.ttfb_ms),
                fmt_ms(slow.discord_ms)
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Discord API\n");
    if stats.discord.is_empty() {
        let _ = writeln!(out, "_Brak wywołań._\n");
    } else {
        let _ = writeln!(
            out,
            "| Endpoint | Wywołań | p50 ms | p99 ms | max ms | 429 | Błędy | Statusy | Min. remaining |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---|---:|");
        for (name, entry) in &stats.discord {
            let [p50, p99] = entry.latency.percentiles([50.0, 99.0]);
            let statuses: Vec<String> = entry
                .statuses
                .iter()
                .map(|(status, count)| {
                    if *status == 0 {
                        format!("brak odpowiedzi×{count}")
                    } else {
                        format!("{status}×{count}")
                    }
                })
                .collect();
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} |",
                cell(name),
                entry.latency.seen,
                fmt_ms(p50),
                fmt_ms(p99),
                fmt_ms(entry.latency.max),
                entry.rate_limited,
                entry.errors,
                statuses.join(", "),
                entry
                    .min_remaining
                    .map_or_else(|| "—".to_owned(), |value| format!("{value:.0}"))
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Pliki danych\n");
    if stats.io.is_empty() {
        let _ = writeln!(out, "_Brak operacji._\n");
    } else {
        let _ = writeln!(
            out,
            "| Operacja | Liczba | p50 ms | p99 ms | max ms | Błędy | Śr. rozmiar |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
        for (name, entry) in &stats.io {
            let [p50, p99] = entry.latency.percentiles([50.0, 99.0]);
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} | {} |",
                cell(name),
                entry.latency.seen,
                fmt_ms(p50),
                fmt_ms(p99),
                fmt_ms(entry.latency.max),
                entry.errors,
                fmt_bytes(entry.bytes as f64 / entry.latency.seen.max(1) as f64)
            );
        }
        out.push('\n');
    }

    if !stats.locks.is_empty() {
        let _ = writeln!(out, "## Czekanie na blokady (≥ 1 ms)\n");
        let _ = writeln!(out, "| Blokada | Liczba | p50 ms | p99 ms | max ms |");
        let _ = writeln!(out, "|---|---:|---:|---:|---:|");
        for (name, samples) in &stats.locks {
            let [p50, p99] = samples.percentiles([50.0, 99.0]);
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} |",
                cell(name),
                samples.seen,
                fmt_ms(p50),
                fmt_ms(p99),
                fmt_ms(samples.max)
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Zadania okresowe\n");
    if stats.ticks.is_empty() {
        let _ = writeln!(out, "_Brak danych._\n");
    } else {
        let _ = writeln!(
            out,
            "| Zadanie | Uruchomień | Maks. spóźnienie ms | Spóźnień > 1 s | Maks. czas pracy ms |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|");
        for (name, entry) in &stats.ticks {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} |",
                cell(name),
                entry.count,
                fmt_ms(entry.late_max),
                entry.late_over_1s,
                fmt_ms(entry.work_max)
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Zasoby procesu\n");
    let resources = &stats.resources;
    match &resources.last {
        None => {
            let _ = writeln!(out, "_Brak próbek._\n");
        }
        Some(last) => {
            let value = |key: &str| last.get(key).and_then(Value::as_f64).unwrap_or(0.0);
            let _ = writeln!(
                out,
                "- RAM (RSS): teraz {}, maks. dziś {}, pierwsza próbka dnia {}",
                fmt_bytes(value("rss_kb") * 1024.0),
                fmt_bytes(resources.rss_max_kb * 1024.0),
                fmt_bytes(resources.first_rss_kb.unwrap_or(0.0) * 1024.0)
            );
            let _ = writeln!(
                out,
                "- CPU: średnio {:.2}% jednego rdzenia, łącznie {:.1} s od startu procesu",
                if resources.cpu_samples == 0 {
                    0.0
                } else {
                    resources.cpu_sum / resources.cpu_samples as f64
                },
                value("cpu_total_s")
            );
            let _ = writeln!(
                out,
                "- Wątki: {} (maks. {}), deskryptory: {} (maks. {}), procesy potomne: {} (maks. {})",
                value("threads"),
                resources.threads_max,
                value("fds"),
                resources.fds_max,
                value("children"),
                resources.children_max
            );
            let _ = writeln!(
                out,
                "- Opóźnienie pętli zdarzeń: maks. dziś {} ms",
                fmt_ms(resources.lag_max_ms)
            );
            let _ = writeln!(
                out,
                "- Sesje: {}, logowania w toku: {}, serwery w cache: {}, próbek: {}\n",
                value("sessions"),
                value("login_states"),
                value("invite_cache"),
                resources.samples
            );
        }
    }

    let _ = writeln!(out, "## Anomalie\n");
    if stats.anomalies.is_empty() {
        let _ = writeln!(out, "_Brak._\n");
    } else {
        let _ = writeln!(out, "| Rodzaj | Liczba | Ostatnio (UTC) | Ostatni opis |");
        let _ = writeln!(out, "|---|---:|---|---|");
        for (name, entry) in &stats.anomalies {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} |",
                cell(name),
                entry.count,
                time_of(entry.last_ts),
                cell(&entry.last_detail)
            );
        }
        out.push('\n');
    }

    let _ = writeln!(out, "## Wskazówki\n");
    let hints = hints(report);
    if hints.is_empty() {
        let _ = writeln!(out, "- Brak uwag — wszystkie wskaźniki w normie.");
    } else {
        for hint in hints {
            let _ = writeln!(out, "- {hint}");
        }
    }
    out
}

/// Automatic advice derived from the day's numbers.
fn hints(report: &Report<'_>) -> Vec<String> {
    let stats = report.stats;
    let mut hints = Vec::new();

    for (name, route) in &stats.routes {
        let count = route.latency.seen;
        let [p99] = route.latency.percentiles([99.0]);
        if count >= 20 && p99 > 1000.0 {
            let per = count as f64;
            let total = route.latency.mean();
            let parts = [
                ("Discord", route.discord_sum / per),
                ("pliki danych", route.io_sum / per),
                ("blokady", route.lock_sum / per),
            ];
            let other = (total - parts.iter().map(|(_, value)| value).sum::<f64>()).max(0.0);
            let (component, _) = parts
                .iter()
                .copied()
                .chain(std::iter::once(("serwer i wysyłka", other)))
                .fold(
                    ("", f64::MIN),
                    |best, item| if item.1 > best.1 { item } else { best },
                );
            hints.push(format!(
                "`{name}`: p99 = {} ms przy {count} żądaniach; najwięcej czasu zajmuje: {component}.",
                fmt_ms(p99)
            ));
        }
        if route.discord_sum > 0.0 && route.discord_sum >= route.latency.sum * 0.5 && count >= 10 {
            hints.push(format!(
                "`{name}` czeka głównie na Discord ({:.0}% czasu). Dłuższe SESSION_DATA_TTL_MS / INVITE_CACHE_TTL_MS zmniejszy liczbę takich zapytań.",
                route.discord_sum / route.latency.sum * 100.0
            ));
        }
        if route.text_large >= 10 && route.text_compressed * 5 < route.text_large * 4 {
            hints.push(format!(
                "`{name}`: tylko {}/{} dużych odpowiedzi tekstowych do klientów obsługujących kompresję poszło skompresowanych.",
                route.text_compressed, route.text_large
            ));
        }
        if route.by_class[4] > 0 {
            hints.push(format!(
                "`{name}`: {} odpowiedzi 5xx — szczegóły w sekcji Anomalie i w `journalctl -u bladephoria-oauth`.",
                route.by_class[4]
            ));
        }
    }

    for (name, entry) in &stats.discord {
        if entry.rate_limited > 0 {
            hints.push(format!(
                "Discord `{name}` zwrócił 429 {} razy — ogranicz odświeżanie (refresh=1) albo wydłuż cache.",
                entry.rate_limited
            ));
        }
        if entry.errors > entry.rate_limited {
            hints.push(format!(
                "Discord `{name}`: {} wywołań bez poprawnej odpowiedzi (sieć, timeout albo błędna treść).",
                entry.errors - entry.rate_limited
            ));
        }
        let [p99] = entry.latency.percentiles([99.0]);
        if entry.latency.seen >= 10 && p99 > 2000.0 {
            hints.push(format!(
                "Discord `{name}`: p99 = {} ms — wolne łącze VPS ↔ Discord albo przeciążenie API.",
                fmt_ms(p99)
            ));
        }
    }

    for (name, entry) in &stats.io {
        let [p99] = entry.latency.percentiles([99.0]);
        if entry.latency.seen >= 5 && p99 > 50.0 {
            hints.push(format!(
                "Plik `{name}`: p99 = {} ms — wolny dysk albo za duży plik.",
                fmt_ms(p99)
            ));
        }
        if entry.errors > 0 {
            hints.push(format!(
                "Plik `{name}`: {} nieudanych operacji.",
                entry.errors
            ));
        }
    }

    for (name, samples) in &stats.locks {
        if samples.max > 100.0 {
            hints.push(format!(
                "Blokada `{name}`: czekanie do {} ms — równoległe zapisy się kolejkują.",
                fmt_ms(samples.max)
            ));
        }
    }

    for (name, entry) in &stats.ticks {
        if entry.late_over_1s > 0 {
            hints.push(format!(
                "Zadanie `{name}` spóźniło się ponad 1 s {} razy — przeciążony serwer albo zablokowana pętla zdarzeń.",
                entry.late_over_1s
            ));
        }
    }

    let resources = &stats.resources;
    if resources.lag_max_ms > 100.0 {
        hints.push(format!(
            "Pętla zdarzeń była zablokowana do {} ms — coś wykonuje się synchronicznie w wątkach tokio.",
            fmt_ms(resources.lag_max_ms)
        ));
    }
    if let (Some(first), Some(last)) = (
        resources.first_rss_kb,
        resources
            .last
            .as_ref()
            .and_then(|last| last.get("rss_kb"))
            .and_then(Value::as_f64),
    ) && last > first * 1.5
        && last - first > 20.0 * 1024.0
    {
        hints.push(format!(
            "Pamięć rośnie: {} → {} w ciągu dnia — możliwy wyciek.",
            fmt_bytes(first * 1024.0),
            fmt_bytes(last * 1024.0)
        ));
    }

    for (name, entry) in &stats.anomalies {
        let hint = match name.as_str() {
            "invite_expired" => format!(
                "Główne zaproszenie Discord wygasło ({}) — przestaw kolejność kodów w `backend/src/discord.rs`.",
                entry.last_detail
            ),
            "clock_skew" | "clock_jump" => format!(
                "Zegar systemowy: {} — sprawdź `timedatectl` / systemd-timesyncd.",
                entry.last_detail
            ),
            "login_failed" => format!(
                "Nieudane logowania: {} (ostatnio: {}).",
                entry.count, entry.last_detail
            ),
            "rate_limited" => continue,
            _ => format!(
                "Anomalia `{name}`: {}× (ostatnio: {}).",
                entry.count, entry.last_detail
            ),
        };
        hints.push(hint);
    }

    if report.dropped > 0 {
        hints.push(format!(
            "{} zdarzeń nie zmieściło się w kolejce zapisu — dysk nie nadąża albo ruch jest bardzo duży.",
            report.dropped
        ));
    }
    if report.suppressed > 0 {
        hints.push(format!(
            "Limit {} MB/dzień osiągnięty, {} zdarzeń pominięto (raport nadal je liczy do dnia od restartu) — zwiększ BLADEPHORIA_DIAGNOSTICS_MAX_MB.",
            report.cfg.max_mb_per_day, report.suppressed
        ));
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bladephoria-diag-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(dir: &Path) -> DiagConfig {
        DiagConfig {
            enabled: true,
            dir: dir.to_path_buf(),
            retention_days: 7,
            max_mb_per_day: 50,
            summary_every: Duration::from_secs(900),
        }
    }

    fn event(ts: i64, kind: &str, fields: Value) -> Value {
        let mut event = json!({ "ts": iso_from_ms(ts), "t_ms": ts, "kind": kind });
        if let (Value::Object(target), Value::Object(fields)) = (&mut event, fields) {
            target.extend(fields);
        }
        event
    }

    fn http(ts: i64, route: &str, status: u64, total: f64) -> Value {
        event(
            ts,
            "http",
            json!({
                "method": "GET", "route": route, "status": status, "ttfb_ms": total / 2.0,
                "total_ms": total, "bytes": 2048, "encoding": "br", "content": "html", "accepts_compression": true,
                "aborted": false, "discord_ms": 0.0, "io_ms": 0.0, "lock_ms": 0.0,
            }),
        )
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut samples = Samples::default();
        for value in 1..=100 {
            samples.push(f64::from(value));
        }
        assert_eq!(
            samples.percentiles([50.0, 90.0, 99.0, 100.0]),
            [50.0, 90.0, 99.0, 100.0]
        );
        assert_eq!(samples.max, 100.0);
        assert_eq!(Samples::default().percentiles([50.0]), [0.0]);
    }

    #[test]
    fn reservoir_stays_bounded() {
        let mut samples = Samples::default();
        for value in 0..(MAX_SAMPLES * 3) {
            samples.push(value as f64);
        }
        assert_eq!(samples.values.len(), MAX_SAMPLES);
        assert_eq!(samples.seen, (MAX_SAMPLES * 3) as u64);
        // A uniform sample of 0..60000 has its median far above the first third.
        let [p50] = samples.percentiles([50.0]);
        assert!(p50 > MAX_SAMPLES as f64, "median {p50}");
    }

    #[test]
    fn routes_are_grouped_without_query_or_slugs() {
        assert_eq!(route_label("/", 200), "/");
        assert_eq!(route_label("/auth/session", 200), "/auth/session");
        assert_eq!(route_label("/profile/", 200), "/profile/");
        assert_eq!(route_label("/profile/yhym/", 200), "/profile/:slug");
        assert_eq!(route_label("/profile/yhym", 301), "/profile/:slug");
        assert_eq!(route_label("/media/Build/a b.zip", 206), "/media/*");
        assert_eq!(route_label("/wp-login.php", 404), "(404)");
        assert_eq!(route_label("/auth/session", 405), "(405)");
        assert_eq!(route_label(&format!("/{}", "a".repeat(200)), 200).len(), 80);
    }

    #[test]
    fn content_classes() {
        assert_eq!(content_class(Some("text/html; charset=utf-8")), "html");
        assert_eq!(content_class(Some("text/javascript; charset=utf-8")), "js");
        assert_eq!(
            content_class(Some("application/json; charset=utf-8")),
            "json"
        );
        assert_eq!(content_class(Some("image/png")), "image");
        assert_eq!(content_class(Some("application/zip")), "other");
        assert_eq!(content_class(None), "none");
    }

    #[test]
    fn full_queue_drops_instead_of_blocking() {
        let (sender, _receiver) = sync_channel(1);
        let dropped = AtomicU64::new(0);
        offer(&sender, &dropped, Msg::Shutdown);
        offer(&sender, &dropped, Msg::Shutdown);
        offer(&sender, &dropped, Msg::Shutdown);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn writer_writes_events_and_summary() {
        let dir = temp_dir("writer");
        let mut writer = Writer::new(config(&dir));
        let now = now_ms();
        writer.open_day(&day_of(now));
        for index in 0..30 {
            writer.handle(http(now, "/", 200, f64::from(index)));
        }
        writer.handle(http(now, "/auth/session", 500, 2500.0));
        writer.handle(event(
            now,
            "discord",
            json!({ "endpoint": "invites", "status": 429, "total_ms": 40.0, "error": null, "ratelimit_remaining": 0 }),
        ));
        writer.handle(event(
            now,
            "anomaly",
            json!({ "type": "invite_expired", "detail": "Bladephoriafonk: Nh7XnSMhCp dead" }),
        ));
        writer.write_summary(false);

        let day = day_of(now);
        let events = fs::read_to_string(dir.join(format!("events-{day}.jsonl"))).unwrap();
        assert_eq!(events.lines().count(), 33);
        for line in events.lines() {
            serde_json::from_str::<Value>(line).unwrap();
        }

        let summary = fs::read_to_string(dir.join(format!("summary-{day}.md"))).unwrap();
        for expected in [
            "## Czas odpowiedzi — cały dzień",
            "p99 ms",
            "| `/` | 30 |",
            "## Gdzie idzie czas",
            "## Najwolniejsze żądania",
            "| `invites` | 1 |",
            "`invite_expired`",
            "## Wskazówki",
            "odpowiedzi 5xx",
            "zwrócił 429",
            "przestaw kolejność",
        ] {
            assert!(
                summary.contains(expected),
                "missing {expected:?} in\n{summary}"
            );
        }
        // The window starts over after each periodic summary.
        assert!(writer.window.routes.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restart_rebuilds_the_daily_report() {
        let dir = temp_dir("replay");
        let now = now_ms();
        let day = day_of(now);
        {
            let mut writer = Writer::new(config(&dir));
            writer.open_day(&day);
            for _ in 0..5 {
                writer.handle(http(now, "/server", 200, 3.0));
            }
            writer.flush();
        }
        let mut writer = Writer::new(config(&dir));
        writer.open_day(&day);
        writer.handle(http(now, "/server", 200, 3.0));
        assert_eq!(writer.stats.routes["/server"].latency.seen, 6);
        assert_eq!(writer.events_today, 6);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_limit_stops_the_event_file() {
        let dir = temp_dir("limit");
        let mut cfg = config(&dir);
        cfg.max_mb_per_day = 1;
        let mut writer = Writer::new(cfg);
        let now = now_ms();
        writer.open_day(&day_of(now));
        let filler = "x".repeat(4000);
        for _ in 0..400 {
            writer.handle(event(
                now,
                "anomaly",
                json!({ "type": "filler", "detail": filler }),
            ));
        }
        writer.flush();
        let size = fs::metadata(dir.join(format!("events-{}.jsonl", day_of(now))))
            .unwrap()
            .len();
        assert!(size <= 1024 * 1024 + 200, "size {size}");
        assert!(writer.suppressed > 0);
        assert_eq!(writer.stats.anomalies["filler"].count, 400);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_removes_only_old_report_files() {
        let dir = temp_dir("prune");
        for name in [
            "events-2026-11-01.jsonl",
            "summary-2026-11-01.md",
            "events-2026-11-10.jsonl",
            "summary-2026-11-16.md",
            "notes-2026-11-01.txt",
            "events-old.jsonl",
        ] {
            fs::write(dir.join(name), "x").unwrap();
        }
        prune(&dir, 7, "2026-11-17");
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "events-old.jsonl",
                "notes-2026-11-01.txt",
                "summary-2026-11-16.md"
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn day_change_finishes_the_old_report() {
        let dir = temp_dir("rotate");
        let mut writer = Writer::new(config(&dir));
        let day_one = parse_iso_ms("2026-11-16T23:59:59Z").unwrap();
        let day_two = parse_iso_ms("2026-11-17T00:00:01Z").unwrap();
        writer.open_day("2026-11-16");
        writer.handle(http(day_one, "/", 200, 1.0));
        writer.handle(http(day_two, "/", 200, 1.0));
        // A late event of the previous day stays in the current file.
        writer.handle(http(day_one, "/", 200, 1.0));
        writer.flush();
        assert!(dir.join("summary-2026-11-16.md").exists());
        let second = fs::read_to_string(dir.join("events-2026-11-17.jsonl")).unwrap();
        assert_eq!(second.lines().count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quiet_day_has_no_hints() {
        let dir = temp_dir("quiet");
        let cfg = config(&dir);
        let mut stats = Stats::default();
        let now = now_ms();
        for _ in 0..50 {
            stats.record(&http(now, "/", 200, 2.0));
        }
        let report = Report {
            day: "2026-11-17",
            stats: &stats,
            window: &Stats::default(),
            window_from_ms: now,
            now_ms: now,
            events_today: 50,
            bytes_today: 1000,
            dropped: 0,
            suppressed: 0,
            final_report: true,
            cfg: &cfg,
        };
        assert!(hints(&report).is_empty());
        assert!(render_summary(&report).contains("Brak uwag"));
        let _ = fs::remove_dir_all(&dir);
    }
}
