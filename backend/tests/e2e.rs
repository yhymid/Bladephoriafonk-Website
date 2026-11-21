//! End-to-end test of the real server binary with diagnostics enabled.
//!
//! An in-process mock of the Discord API stands in for discord.com. The test
//! logs in, reads the session, publishes a changelog post, opens the profile
//! page and the server list, stops the server with SIGTERM and then checks
//! the diagnostics files: valid JSON lines, every kind of measurement, a
//! summary with percentiles - and none of the secrets used on the way.

use std::collections::HashSet;
use std::fs;
use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

const CLIENT_SECRET: &str = "e2e-client-secret-4f1c9a";
const AUTH_CODE: &str = "e2e-auth-code-7b2d10";
const ACCESS_TOKEN: &str = "e2e-access-token-93ce55";
const REFRESH_TOKEN: &str = "e2e-refresh-token-0a8e7f";
/// Matches the default ADMIN_USER_IDS, so the user may post to the changelog.
const USER_ID: &str = "780552865445969920";

fn reply(status: StatusCode, body: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(&format!("Bearer {ACCESS_TOKEN}"))
}

async fn token(body: String) -> Response {
    let form: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let field = |name: &str| {
        form.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    if field("client_secret") != Some(CLIENT_SECRET) {
        return reply(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "invalid_client" }),
        );
    }
    let valid = match field("grant_type") {
        Some("authorization_code") => field("code") == Some(AUTH_CODE),
        Some("refresh_token") => field("refresh_token") == Some(REFRESH_TOKEN),
        _ => false,
    };
    if !valid {
        return reply(StatusCode::BAD_REQUEST, json!({ "error": "invalid_grant" }));
    }
    reply(
        StatusCode::OK,
        json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": REFRESH_TOKEN,
            "expires_in": 604800,
            "token_type": "Bearer",
        }),
    )
}

async fn user_resource(headers: HeaderMap, body: Value) -> Response {
    if authorized(&headers) {
        reply(StatusCode::OK, body)
    } else {
        reply(
            StatusCode::UNAUTHORIZED,
            json!({ "message": "401: Unauthorized" }),
        )
    }
}

async fn invite(UrlPath(code): UrlPath<String>) -> Response {
    match code.as_str() {
        // The main Bladephoriafonk invite has expired: the backup must be used.
        "Nh7XnSMhCp" => reply(
            StatusCode::NOT_FOUND,
            json!({ "message": "Invite is expired.", "code": 50270 }),
        ),
        // PMC is rate limited: no fallback, served as stale.
        "2upsTHV5Bv" => (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::RETRY_AFTER, "1.5"),
            ],
            json!({ "message": "You are being rate limited.", "retry_after": 1.5, "global": false })
                .to_string(),
        )
            .into_response(),
        _ => reply(
            StatusCode::OK,
            json!({
                "guild": { "id": "222", "icon": "abc", "description": "E2E" },
                "approximate_member_count": 42,
                "approximate_presence_count": 7,
            }),
        ),
    }
}

async fn start_mock_discord() -> String {
    let app = Router::new()
        .route("/api/oauth2/token", post(token))
        .route(
            "/api/v10/users/@me",
            get(|headers: HeaderMap| {
                user_resource(
                    headers,
                    json!({ "id": USER_ID, "username": "e2e.user", "global_name": "E2E", "avatar": null }),
                )
            }),
        )
        .route(
            "/api/v10/users/@me/connections",
            get(|headers: HeaderMap| user_resource(headers, json!([]))),
        )
        .route(
            "/api/v10/users/@me/guilds",
            get(|headers: HeaderMap| {
                user_resource(headers, json!([{ "id": "222", "name": "Bladephoriafonk" }]))
            }),
        )
        .route(
            "/api/v10/users/@me/guilds/{guild}/member",
            get(|headers: HeaderMap| {
                user_resource(headers, json!({ "nick": "E2E", "bio": "", "banner": null }))
            }),
        )
        .route("/api/v9/invites/{code}", get(invite));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    origin
}

/// Kills the server if the test fails half way.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn find_file(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.starts_with(prefix) && name.ends_with(suffix)
        })
        .unwrap_or_else(|| panic!("no {prefix}*{suffix} in {}", dir.display()))
}

#[tokio::test]
async fn diagnostics_record_everything_and_no_secrets() {
    let temp = std::env::temp_dir().join(format!("bladephoria-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp).unwrap();
    let diagnostics_dir = temp.join("diagnostics");
    let site_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();

    let discord = start_mock_discord().await;
    let port = free_port();
    let panel_port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_bladephoria-web"))
            .current_dir(&temp)
            .env_clear()
            .env("SITE_ROOT", site_root)
            .env("HOST", "127.0.0.1")
            .env("PORT", port.to_string())
            .env("BASE_URL", &base)
            .env("DISCORD_ORIGIN", &discord)
            .env("DISCORD_CLIENT_ID", "123456789012345678")
            .env("DISCORD_CLIENT_SECRET", CLIENT_SECRET)
            .env(
                "DISCORD_REDIRECT_URI",
                format!("{base}/auth/discord/callback"),
            )
            .env("PROFILE_DB_FILE", temp.join("data/profiles.json"))
            .env("CHANGELOG_DB_FILE", temp.join("data/changelog.json"))
            .env("SESSION_DB_FILE", temp.join("data/sessions.json"))
            .env("MEDIA_ROOT", temp.join("media"))
            .env("BLADEPHORIA_DIAGNOSTICS", "1")
            .env("BLADEPHORIA_DIAGNOSTICS_DIR", &diagnostics_dir)
            .env("ADMIN_PANEL_PORT", panel_port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("server binary starts"),
    );

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.get(format!("{base}/health")).send().await.is_err() {
        assert!(Instant::now() < deadline, "server did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let status = |url: String| {
        let client = client.clone();
        async move {
            let response = client.get(url).send().await.unwrap();
            let status = response.status().as_u16();
            // Read the body to the end, like a browser would.
            response.bytes().await.unwrap();
            status
        }
    };
    assert_eq!(status(format!("{base}/")).await, 200);
    assert_eq!(status(format!("{base}/nope")).await, 404);
    // The diagnostics directory is never served.
    assert_eq!(status(format!("{base}/diagnostics/")).await, 404);

    let servers: Value = client
        .get(format!("{base}/auth/server-data"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .map(|body| serde_json::from_slice(&body).unwrap())
        .unwrap();
    assert_eq!(
        servers["servers"][0]["main_url"],
        "https://discord.gg/2xTr8pmk36"
    );
    assert_eq!(servers["servers"][1]["stale"], true);

    // Login: /auth/discord/login -> Discord -> callback with code and state.
    let login = client
        .get(format!("{base}/auth/discord/login?return_to=/"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 302);
    let location = url::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    let state = location
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    let callback = client
        .get(format!(
            "{base}/auth/discord/callback?state={state}&code={AUTH_CODE}"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 302);
    let cookie = callback.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let session_id = cookie.split_once('=').unwrap().1.to_owned();
    assert!(session_id.len() >= 32);

    let session: Value = client
        .get(format!("{base}/auth/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .map(|body| serde_json::from_slice(&body).unwrap())
        .unwrap();
    assert_eq!(session["authenticated"], true);
    assert_eq!(session["is_admin"], true);
    let slug = session["profile"]["slug"].as_str().unwrap().to_owned();

    let created = client
        .post(format!("{base}/auth/changelog"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body(json!({ "title": "E2E", "content": "End-to-end post" }).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    created.bytes().await.unwrap();
    assert_eq!(status(format!("{base}/profile/{slug}/")).await, 200);

    // The admin panel: loopback port only, every page renders, a foreign
    // Host header is refused, the public port does not serve it.
    let panel = format!("http://127.0.0.1:{panel_port}");
    let mut panel_html = String::new();
    for (path, expected) in [
        ("/", "Bladephoriafonk"),
        ("/diagnostics", "Diagnostyka"),
        ("/data", "changelog.json"),
        ("/logs", "Logi"),
        ("/logs?errors=1&q=failed", "Logi"),
        ("/style.css", "--bg: #000"),
    ] {
        let response = client.get(format!("{panel}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
        if path != "/style.css" {
            assert!(
                response.headers()["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .starts_with("default-src 'none'")
            );
        }
        let text = response.text().await.unwrap();
        assert!(text.contains(expected), "{path} lacks {expected:?}");
        panel_html.push_str(&text);
    }
    assert!(panel_html.contains("Zalogowane sesje</span><strong>1</strong>"));
    // Bladephoriafonk's main invite is dead in the mock: the panel says so.
    assert!(panel_html.contains("zapasowe"));
    let foreign = client
        .get(format!("{panel}/"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 403);
    foreign.bytes().await.unwrap();
    assert_eq!(status(format!("{base}/logs")).await, 404);
    assert_eq!(status(format!("{base}/data")).await, 404);

    // Graceful stop writes the final summary.
    let pid = server.0.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "server did not stop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let events_path = find_file(&diagnostics_dir, "events-", ".jsonl");
    let events = fs::read_to_string(&events_path).unwrap();
    let mut kinds = HashSet::new();
    let mut anomalies = HashSet::new();
    let mut http_events = 0;
    for line in events.lines() {
        let event: Value = serde_json::from_str(line).expect("every line is JSON");
        let kind = event["kind"].as_str().unwrap().to_owned();
        assert!(event["t_ms"].is_i64() && event["ts"].is_string());
        if kind == "http" {
            http_events += 1;
            let route = event["route"].as_str().unwrap();
            assert!(!route.contains('?'), "query string recorded: {route}");
            // Every response here is read to the end by the client.
            assert_eq!(
                event["aborted"], false,
                "complete response marked aborted: {line}"
            );
            assert!(event["total_ms"].as_f64().unwrap() >= event["ttfb_ms"].as_f64().unwrap());
        }
        if kind == "anomaly" {
            anomalies.insert(event["type"].as_str().unwrap().to_owned());
        }
        kinds.insert(kind);
    }
    for kind in [
        "lifecycle",
        "http",
        "discord",
        "io",
        "tick",
        "resources",
        "anomaly",
    ] {
        assert!(kinds.contains(kind), "no {kind} event in {events}");
    }
    for anomaly in ["invite_expired", "rate_limited"] {
        assert!(anomalies.contains(anomaly), "no {anomaly} anomaly");
    }
    assert!(http_events >= 9, "only {http_events} http events");
    assert!(events.contains("\"phase\":\"stop\""));

    let summary = fs::read_to_string(find_file(&diagnostics_dir, "summary-", ".md")).unwrap();
    for expected in [
        "p99 ms",
        "## Gdzie idzie czas",
        "| `/auth/session` |",
        "| `oauth2/token` |",
        "## Wskazówki",
        "raport końcowy",
    ] {
        assert!(
            summary.contains(expected),
            "summary lacks {expected:?}:\n{summary}"
        );
    }

    // Not one secret in any diagnostics file.
    let secrets = [
        CLIENT_SECRET,
        AUTH_CODE,
        ACCESS_TOKEN,
        REFRESH_TOKEN,
        state.as_str(),
        session_id.as_str(),
    ];
    for secret in secrets {
        assert!(
            !panel_html.contains(secret),
            "the admin panel shows a secret"
        );
    }
    for entry in fs::read_dir(&diagnostics_dir).unwrap().flatten() {
        let content = fs::read_to_string(entry.path()).unwrap();
        for secret in secrets {
            assert!(
                !content.contains(secret),
                "{} contains a secret",
                entry.path().display()
            );
        }
        assert!(
            !content.contains("code="),
            "query string in {}",
            entry.path().display()
        );
    }

    let _ = fs::remove_dir_all(&temp);
}
