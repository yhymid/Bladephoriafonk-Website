//! Bladephoriafonk website server.
//!
//! One binary replaces the former Node `dev-server.mjs` and
//! `oauth/discord-auth-server.mjs`: it serves the static site and the
//! `/auth/*` Discord OAuth API.

mod api;
mod check;
mod config;
mod diagnostics;
mod discord;
mod error;
mod static_files;
mod store;
mod util;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Extensions, HeaderMap, StatusCode, Version};
use axum::routing::{any, get, post};
use axum::serve::ListenerExt;
use serde_json::json;
use tower_http::compression::CompressionLayer;

use crate::api::App;
use crate::config::Config;

const USAGE: &str = "Usage: bladephoria-web [--env-file <path>] [--check-config]

Serves the Bladephoriafonk website and its Discord OAuth API.
Configuration is read from environment variables (see oauth/.env.example).

  --check-config   validate the configuration and data files, then exit
                   (nothing is started or written; exit code 1 on errors)
  --version        print the version";

/// Compress text responses only; images, archives and video are already
/// compressed and would just burn CPU.
fn is_compressible(_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|mime| {
            mime.starts_with("text/")
                || mime.starts_with("application/json")
                || mime.starts_with("image/svg")
        })
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health", any(api::health))
        .route("/auth/discord/login", get(api::discord_login))
        .route("/auth/discord/callback", get(api::discord_callback))
        .route("/auth/session", get(api::session))
        .route("/auth/public-profile", get(api::public_profile))
        .route("/auth/server-data", get(api::server_data))
        .route(
            "/auth/changelog",
            get(api::changelog_list).post(api::changelog_create),
        )
        .route("/auth/profile-mode", post(api::profile_mode))
        .route("/auth/profile-delete", post(api::profile_delete))
        .route("/auth/logout", post(api::logout))
        .route("/auth/media-delete", post(api::media_delete))
        .method_not_allowed_fallback(api::not_found)
        .fallback(static_files::serve)
        .layer(CompressionLayer::new().compress_when(is_compressible))
        // Outermost, so it sees the compressed bytes that go on the wire.
        .layer(axum::middleware::from_fn(diagnostics::track_requests))
        .with_state(app)
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).ok();
    let terminated = async {
        match terminate.as_mut() {
            Some(stream) => {
                stream.recv().await;
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminated => {}
    }
}

struct Args {
    env_file: Option<PathBuf>,
    check_config: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut env_file = None;
    let mut check_config = false;
    while let Some(arg) = args.next() {
        if arg == "--check-config" {
            check_config = true;
        } else if arg == "--env-file" {
            env_file = Some(PathBuf::from(args.next().ok_or("--env-file needs a path")?));
        } else if let Some(path) = arg.strip_prefix("--env-file=") {
            env_file = Some(PathBuf::from(path));
        } else {
            return Err(format!("unknown argument: {arg}\n\n{USAGE}"));
        }
    }
    Ok(Args {
        env_file,
        check_config,
    })
}

/// Prints every finding; fails when one of them is an error.
async fn check_config(cfg: &Config) -> Result<(), String> {
    let findings = check::check_config(cfg).await;
    let mut errors = 0;
    for found in &findings {
        let mark = match found.level {
            check::Level::Ok => "ok   ",
            check::Level::Warn => "warn ",
            check::Level::Error => {
                errors += 1;
                "ERROR"
            }
        };
        println!("{mark} {}", found.text);
    }
    if errors == 0 {
        println!("configuration ok");
        Ok(())
    } else {
        Err(format!("configuration has {errors} error(s)"))
    }
}

/// First 12 characters of the commit recorded by `deploy/deploy.sh`.
fn deployed_commit(site_root: &Path) -> Option<String> {
    let commit = std::fs::read_to_string(site_root.join(".deployed-commit")).ok()?;
    let commit = commit.trim();
    (!commit.is_empty()).then(|| commit.chars().take(12).collect())
}

/// Reports inconsistencies in the data files as diagnostics anomalies.
async fn audit_data(cfg: &Config) {
    if !diagnostics::enabled() {
        return;
    }
    let (profiles, posts, issues) = store::audit_data(cfg).await;
    for (kind, detail) in &issues {
        diagnostics::anomaly(kind, detail.clone());
    }
    diagnostics::lifecycle(
        "data_audit",
        json!({ "profiles": profiles, "posts": posts, "issues": issues.len() }),
    );
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let cfg = Config::load(args.env_file.as_deref())?;
    if args.check_config {
        return check_config(&cfg).await;
    }
    let started = Instant::now();

    // Diagnostics must never keep the site down: a failure only disables them.
    let diagnostics_on = diagnostics::start(&cfg.diagnostics).unwrap_or_else(|error| {
        eprintln!(
            "[diagnostics] disabled, cannot write to {}: {error}",
            cfg.diagnostics.dir.display()
        );
        false
    });
    diagnostics::lifecycle(
        "start",
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commit": deployed_commit(&cfg.site_root),
            "pid": std::process::id(),
            "port": cfg.port,
            "session_data_ttl_ms": cfg.session_data_ttl_ms,
            "invite_cache_ttl_ms": cfg.invite_cache_ttl_ms,
            "retention_days": cfg.diagnostics.retention_days,
            "max_mb_per_day": cfg.diagnostics.max_mb_per_day,
            "summary_minutes": cfg.diagnostics.summary_every.as_secs() / 60,
        }),
    );

    store::ensure_default_files(&cfg)
        .await
        .map_err(|error| format!("cannot prepare data files: {error}"))?;
    let address = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|error| format!("cannot listen on {address}: {error}"))?;
    // Files are streamed as separate header and body writes; without
    // TCP_NODELAY every such response waits ~40 ms on Nagle's algorithm.
    let listener = listener.tap_io(|stream| {
        let _ = stream.set_nodelay(true);
    });

    println!("Bladephoriafonk web server listening on http://{address}");
    println!("Site root: {}", cfg.site_root.display());
    println!("Profile template: {}", cfg.profile_template_file.display());
    if diagnostics_on {
        println!("Diagnostics: {}", cfg.diagnostics.dir.display());
    }

    let app =
        Arc::new(App::new(cfg).map_err(|error| format!("cannot start HTTP client: {error}"))?);

    match app.load_sessions().await {
        Ok(count) => println!("Restored sessions: {count}"),
        Err(error) => eprintln!("[sessions] cannot restore sessions: {error}"),
    }

    audit_data(&app.cfg).await;
    diagnostics::spawn_samplers({
        let app = app.clone();
        move || app.counts()
    });

    let janitor = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        let mut ticks = 0u32;
        loop {
            let scheduled = tick.tick().await;
            let late = tokio::time::Instant::now().saturating_duration_since(scheduled);
            let work = Instant::now();
            ticks = ticks.wrapping_add(1);
            if ticks.is_multiple_of(4) {
                janitor.cleanup();
            }
            janitor.save_sessions(false).await;
            // Hourly consistency check of the data files.
            if ticks.is_multiple_of(240) {
                audit_data(&janitor.cfg).await;
            }
            diagnostics::tick("janitor", late, work.elapsed());
        }
    });

    let served = axum::serve(listener, router(app.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("server error: {error}"));
    app.save_sessions(true).await;
    diagnostics::lifecycle(
        "stop",
        json!({ "uptime_s": started.elapsed().as_secs(), "error": served.as_ref().err() }),
    );
    diagnostics::shutdown();
    served
}

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("bladephoria-web {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            diagnostics::anomaly("startup_failed", error.clone());
            diagnostics::shutdown();
            eprintln!("bladephoria-web: {error}");
            ExitCode::FAILURE
        }
    }
}
