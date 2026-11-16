//! Static file serving for the public site. Only an explicit allowlist of
//! paths is reachable; everything else (backend sources, data, dotfiles)
//! answers 404. Mirrors the routing of the production nginx config.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::Response;
use percent_encoding::percent_decode_str;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::api::{SharedApp, not_found as api_not_found, security_headers};
use crate::config::Config;

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; base-uri 'self'; form-action 'self'; \
frame-ancestors 'none'; object-src 'none'; script-src 'self' 'unsafe-inline'; \
style-src 'self' 'unsafe-inline'; img-src 'self' https: data:; media-src 'self' https:; \
connect-src 'self' https://discord.com https://cdn.discordapp.com; font-src 'self' data:";

const ASSET_CACHE: &str = "public, max-age=300";
/// Pages are always revalidated, so edits show up immediately while
/// unchanged pages still answer with a cheap `304 Not Modified`.
const PAGE_CACHE: &str = "no-cache";

#[derive(Debug, PartialEq)]
enum Target {
    /// A single file; never a directory.
    File(PathBuf),
    /// A path inside a tree where directories serve their `index.html`.
    Tree(PathBuf),
    Redirect(String),
    NotFound,
}

fn content_type(path: &Path) -> Option<&'static str> {
    Some(
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "html" => "text/html; charset=utf-8",
            "js" => "text/javascript; charset=utf-8",
            "json" => "application/json; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "txt" => "text/plain; charset=utf-8",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "ico" => "image/x-icon",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "ogg" => "video/ogg",
            _ => return None,
        },
    )
}

/// Maps a request path to what should be served for it.
fn resolve(cfg: &Config, raw_path: &str) -> Target {
    let Ok(path) = percent_decode_str(raw_path).decode_utf8() else {
        return Target::NotFound;
    };
    let Some(relative) = path.strip_prefix('/') else {
        return Target::NotFound;
    };

    // No dotfiles, no traversal, nothing a filesystem could reinterpret.
    let unsafe_segment = |segment: &str| segment.starts_with('.') || segment.contains(['\\', '\0']);
    let mut segments = relative.split('/').peekable();
    while let Some(segment) = segments.next() {
        let is_last = segments.peek().is_none();
        if unsafe_segment(segment) || (segment.is_empty() && !is_last) {
            return Target::NotFound;
        }
    }

    let root = &cfg.site_root;
    match relative {
        "" | "index.html" => Target::File(root.join("index.html")),
        "server" | "administracja" | "changelog" => {
            Target::File(root.join(format!("{relative}.html")))
        }
        "server.html" | "administracja.html" | "changelog.html" => {
            Target::Redirect(format!("/{}", relative.trim_end_matches(".html")))
        }
        "site-lang.js" => Target::File(root.join(relative)),
        "profile" => Target::Redirect("/profile/".to_owned()),
        _ => {
            if let Some(rest) = relative.strip_prefix("assets/") {
                Target::File(root.join("assets").join(rest))
            } else if let Some(rest) = relative.strip_prefix("profile/") {
                Target::Tree(cfg.profile_root.join(rest))
            } else if let Some(rest) = relative.strip_prefix("media/") {
                Target::Tree(cfg.media_root.join(rest))
            } else {
                Target::NotFound
            }
        }
    }
}

fn plain_not_found() -> Response {
    let mut response = Response::new(Body::from("Not found"));
    *response.status_mut() = StatusCode::NOT_FOUND;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    security_headers(headers);
    response
}

fn moved_permanently(location: &str, query: Option<&str>) -> Response {
    let location = match query {
        Some(query) => format!("{location}?{query}"),
        None => location.to_owned(),
    };
    let Ok(location) = HeaderValue::from_str(&location) else {
        return plain_not_found();
    };
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
    response.headers_mut().insert(LOCATION, location);
    security_headers(response.headers_mut());
    response
}

async fn send_file(path: PathBuf, request: Request) -> Response {
    let Ok(response) = ServeFile::new(&path).oneshot(request).await;
    if !(response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED) {
        return plain_not_found();
    }

    let mut response = response.map(Body::new);
    let headers = response.headers_mut();
    let mime = content_type(&path);
    if let Some(mime) = mime {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    }
    let is_page = mime.is_some_and(|mime| mime.starts_with("text/html"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if is_page { PAGE_CACHE } else { ASSET_CACHE }),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    security_headers(headers);
    response
}

/// Router fallback: everything that is not an `/auth/*` API route.
pub async fn serve(State(app): State<SharedApp>, request: Request) -> Response {
    let path = request.uri().path();
    if path.starts_with("/auth/") || !app.cfg.serve_static {
        return api_not_found().await;
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return plain_not_found();
    }

    match resolve(&app.cfg, path) {
        Target::NotFound => plain_not_found(),
        Target::Redirect(location) => moved_permanently(&location, request.uri().query()),
        Target::File(file) => match tokio::fs::metadata(&file).await {
            Ok(meta) if meta.is_file() => send_file(file, request).await,
            _ => plain_not_found(),
        },
        Target::Tree(target) => {
            if path.ends_with('/') {
                return send_file(target.join("index.html"), request).await;
            }
            match tokio::fs::metadata(&target).await {
                Ok(meta) if meta.is_file() => send_file(target, request).await,
                Ok(meta) if meta.is_dir() => {
                    moved_permanently(&format!("{path}/"), request.uri().query())
                }
                _ => plain_not_found(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        // SAFETY: tests in this module only read these variables through
        // `Config::load`, and all of them set the same values.
        unsafe {
            std::env::set_var("SITE_ROOT", "/site");
            std::env::remove_var("PROFILE_ROOT");
            std::env::remove_var("MEDIA_ROOT");
        }
        Config::load(None).unwrap()
    }

    fn file(path: &str) -> Target {
        Target::File(PathBuf::from(path))
    }

    #[test]
    fn pages_and_assets() {
        let cfg = config();
        assert_eq!(resolve(&cfg, "/"), file("/site/index.html"));
        assert_eq!(resolve(&cfg, "/index.html"), file("/site/index.html"));
        assert_eq!(resolve(&cfg, "/server"), file("/site/server.html"));
        assert_eq!(resolve(&cfg, "/changelog"), file("/site/changelog.html"));
        assert_eq!(
            resolve(&cfg, "/administracja.html"),
            Target::Redirect("/administracja".into())
        );
        assert_eq!(resolve(&cfg, "/site-lang.js"), file("/site/site-lang.js"));
        assert_eq!(
            resolve(&cfg, "/assets/site.css"),
            file("/site/assets/site.css")
        );
        assert_eq!(
            resolve(&cfg, "/profile"),
            Target::Redirect("/profile/".into())
        );
        assert_eq!(
            resolve(&cfg, "/profile/"),
            Target::Tree(PathBuf::from("/site/profile/"))
        );
        assert_eq!(
            resolve(&cfg, "/profile/yhym/"),
            Target::Tree(PathBuf::from("/site/profile/yhym/"))
        );
        assert_eq!(
            resolve(&cfg, "/media/Build%201/a%20b.zip"),
            Target::Tree(PathBuf::from("/site/media/Build 1/a b.zip"))
        );
    }

    #[test]
    fn private_paths_are_not_reachable() {
        let cfg = config();
        for path in [
            "/oauth/.env",
            "/oauth/data/profiles.json",
            "/oauth/profile-template.html",
            "/src/profile/index.html",
            "/backend/Cargo.toml",
            "/deploy/DEPLOY.md",
            "/.git/config",
            "/.env",
            "/assets/../oauth/.env",
            "/assets/%2e%2e/oauth/.env",
            "/assets/..%2foauth/.env",
            "/profile/../oauth/.env",
            "/media/abc/.delete-token",
            "/media//etc/passwd",
            "/assets/a\\..\\b",
            "/assets/%00",
            "/%ff",
            "/unknown",
            "/server/",
        ] {
            assert_eq!(resolve(&cfg, path), Target::NotFound, "{path}");
        }
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("a/index.HTML")),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            content_type(Path::new("site-lang.js")),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(
            content_type(Path::new("favicon.svg")),
            Some("image/svg+xml")
        );
        assert_eq!(content_type(Path::new("build.zip")), None);
    }
}
