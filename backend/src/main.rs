//! Bladephoriafonk website server.
//!
//! One binary replaces the former Node `dev-server.mjs` and
//! `oauth/discord-auth-server.mjs`: it serves the static site and the
//! `/auth/*` Discord OAuth API.

mod api;
mod config;
mod discord;
mod error;
mod static_files;
mod store;
mod util;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Extensions, HeaderMap, StatusCode, Version};
use axum::routing::{any, get, post};
use axum::serve::ListenerExt;
use tower_http::compression::CompressionLayer;

use crate::api::App;
use crate::config::Config;

const USAGE: &str = "Usage: bladephoria-web [--env-file <path>]

Serves the Bladephoriafonk website and its Discord OAuth API.
Configuration is read from environment variables (see oauth/.env.example).";

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

fn env_file_argument() -> Result<Option<PathBuf>, String> {
    let mut args = std::env::args().skip(1);
    let mut env_file = None;
    while let Some(arg) = args.next() {
        if arg == "--env-file" {
            env_file = Some(PathBuf::from(args.next().ok_or("--env-file needs a path")?));
        } else if let Some(path) = arg.strip_prefix("--env-file=") {
            env_file = Some(PathBuf::from(path));
        } else {
            return Err(format!("unknown argument: {arg}\n\n{USAGE}"));
        }
    }
    Ok(env_file)
}

async fn run() -> Result<(), String> {
    let env_file = env_file_argument()?;
    let cfg = Config::load(env_file.as_deref())?;

    store::ensure_default_files(&cfg)
        .await
        .map_err(|error| format!("cannot prepare data files: {error}"))?;
    store::sync_all_generated_profiles(&cfg)
        .await
        .map_err(|error| format!("cannot generate profile pages: {error}"))?;

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
    println!(
        "Static files: {}",
        if cfg.serve_static {
            "served by this process"
        } else {
            "disabled (API only)"
        }
    );
    println!("Profile template: {}", cfg.profile_template_file.display());
    println!("Profile output: {}", cfg.profile_root.display());

    let app =
        Arc::new(App::new(cfg).map_err(|error| format!("cannot start HTTP client: {error}"))?);

    let janitor = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            janitor.cleanup();
        }
    });

    axum::serve(listener, router(app))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| format!("server error: {error}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bladephoria-web: {error}");
            ExitCode::FAILURE
        }
    }
}
