//! The `/auth/*` JSON API: Discord OAuth login, sessions, profiles,
//! changelog, server data and media deletion. Routes, status codes and JSON
//! shapes are identical to the previous Node backend.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::diagnostics;
use crate::discord::{self, Discord, PUBLIC_SERVERS, ServerConfig};
use crate::error::Fail;
use crate::store;
use crate::util::{
    constant_time_eq, encode_uri_component, infer_media_type, is_lower_hex, iso_from_ms, js_str,
    now_iso, now_ms, parse_iso_ms, random_hex, sanitize_http_url, sanitize_return_to,
    sanitize_slug, truthy,
};

const STATE_TTL_MS: i64 = 10 * 60 * 1000;
const SESSION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const MEDIA_DELETE_BODY_LIMIT: usize = 64 * 1024;

struct LoginState {
    created_at: i64,
    return_to: String,
}

struct SessionData {
    updated_at: i64,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    user: Option<Value>,
    connections: Value,
    guild_profile: Option<Value>,
    profile: Option<Value>,
}

struct Session {
    last_seen_at: AtomicI64,
    data: tokio::sync::Mutex<SessionData>,
}

struct InviteCacheEntry {
    fetched_at: i64,
    payload: Map<String, Value>,
}

pub struct App {
    pub cfg: Config,
    discord: Discord,
    states: Mutex<HashMap<String, LoginState>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Set when sessions changed since they were last written to disk.
    sessions_dirty: AtomicBool,
    invite_cache: Mutex<HashMap<&'static str, InviteCacheEntry>>,
    profile_lock: tokio::sync::Mutex<()>,
    changelog_lock: tokio::sync::Mutex<()>,
}

pub type SharedApp = Arc<App>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl App {
    pub fn new(cfg: Config) -> Result<Self, Fail> {
        Ok(Self {
            discord: Discord::new(&cfg)?,
            cfg,
            states: Mutex::default(),
            sessions: Mutex::default(),
            sessions_dirty: AtomicBool::new(false),
            invite_cache: Mutex::default(),
            profile_lock: tokio::sync::Mutex::default(),
            changelog_lock: tokio::sync::Mutex::default(),
        })
    }

    /// Sizes of the in-memory state, for the diagnostics resource samples.
    pub fn counts(&self) -> diagnostics::AppCounts {
        diagnostics::AppCounts {
            sessions: lock(&self.sessions).len(),
            login_states: lock(&self.states).len(),
            invite_cache: lock(&self.invite_cache).len(),
        }
    }

    /// Drops expired login states and sessions idle for over a week.
    pub fn cleanup(&self) {
        let now = now_ms();
        lock(&self.states).retain(|_, state| now - state.created_at <= STATE_TTL_MS);

        let mut sessions = lock(&self.sessions);
        let before = sessions.len();
        sessions.retain(|_, session| {
            now - session.last_seen_at.load(Ordering::Relaxed) <= SESSION_TTL_MS
        });
        if sessions.len() != before {
            self.sessions_dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Restores the sessions saved by a previous run, so a restart does not
    /// log everybody out.
    pub async fn load_sessions(&self) -> Result<usize, Fail> {
        let Some(saved) = store::read_json_file(&self.cfg.session_db_file).await? else {
            return Ok(0);
        };
        let Some(entries) = saved.get("sessions").and_then(Value::as_object) else {
            return Ok(0);
        };

        let now = now_ms();
        let number = |entry: &Value, key: &str| entry.get(key).and_then(Value::as_i64).unwrap_or(0);
        let optional =
            |entry: &Value, key: &str| entry.get(key).filter(|value| !value.is_null()).cloned();

        let mut sessions = lock(&self.sessions);
        for (session_id, entry) in entries {
            let last_seen_at = number(entry, "last_seen_at");
            let access_token = js_str(entry.get("access_token"));
            if now - last_seen_at > SESSION_TTL_MS || access_token.is_empty() {
                continue;
            }
            let data = SessionData {
                updated_at: number(entry, "updated_at"),
                access_token,
                refresh_token: js_str(entry.get("refresh_token")),
                expires_at: number(entry, "expires_at"),
                user: optional(entry, "user"),
                connections: entry
                    .get("connections")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                guild_profile: optional(entry, "guild_profile"),
                profile: optional(entry, "profile"),
            };
            sessions.insert(
                session_id.clone(),
                Arc::new(Session {
                    last_seen_at: AtomicI64::new(last_seen_at),
                    data: tokio::sync::Mutex::new(data),
                }),
            );
        }
        Ok(sessions.len())
    }

    /// Writes the sessions to disk when they changed (or always with `force`,
    /// used on shutdown to keep the last-seen times).
    pub async fn save_sessions(&self, force: bool) {
        if !self.sessions_dirty.swap(false, Ordering::Relaxed) && !force {
            return;
        }

        let sessions: Vec<(String, Arc<Session>)> = lock(&self.sessions)
            .iter()
            .map(|(session_id, session)| (session_id.clone(), session.clone()))
            .collect();
        let mut entries = Map::new();
        for (session_id, session) in sessions {
            let data = diagnostics::timed_lock("session", &session.data).await;
            entries.insert(
                session_id,
                json!({
                    "last_seen_at": session.last_seen_at.load(Ordering::Relaxed),
                    "updated_at": data.updated_at,
                    "access_token": data.access_token,
                    "refresh_token": data.refresh_token,
                    "expires_at": data.expires_at,
                    "user": data.user,
                    "connections": data.connections,
                    "guild_profile": data.guild_profile,
                    "profile": data.profile,
                }),
            );
        }

        let payload = json!({ "version": 1, "sessions": entries });
        if let Err(error) = store::write_private_json(&self.cfg.session_db_file, &payload).await {
            self.sessions_dirty.store(true, Ordering::Relaxed);
            eprintln!("[sessions] cannot save sessions: {error}");
            diagnostics::anomaly("session_save_failed", error.to_string());
        }
    }

    fn is_admin(&self, user: Option<&Value>) -> bool {
        let Some(user) = user else {
            return false;
        };
        let id = js_str(user.get("id"));
        if !id.is_empty() && self.cfg.admin_user_ids.contains(&id) {
            return true;
        }
        ["username", "global_name"].iter().any(|key| {
            let name = crate::util::normalize_admin_identity(&js_str(user.get(key)));
            !name.is_empty() && self.cfg.admin_usernames.contains(&name)
        })
    }

    fn session_cookie(&self, value: &str, max_age: i64) -> HeaderValue {
        let mut cookie = format!(
            "{}={}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Lax",
            self.cfg.cookie_name,
            encode_uri_component(value)
        );
        if self.cfg.cookie_secure {
            cookie.push_str("; Secure");
        }
        HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static(""))
    }

    fn read_session(&self, headers: &HeaderMap) -> Option<(String, Arc<Session>)> {
        let session_id = cookie_value(headers, &self.cfg.cookie_name)?;
        let session = lock(&self.sessions).get(&session_id).cloned()?;
        session.last_seen_at.store(now_ms(), Ordering::Relaxed);
        Some((session_id, session))
    }

    /// Ends a session whose Discord data could not be refreshed.
    fn expire_session(&self, session_id: &str, status: StatusCode, body: Value) -> Response {
        if lock(&self.sessions).remove(session_id).is_some() {
            self.sessions_dirty.store(true, Ordering::Relaxed);
        }
        let mut response = json_response(status, &body);
        response
            .headers_mut()
            .insert(SET_COOKIE, self.session_cookie("", 0));
        response
    }
}

// ------------------------------------------------------------ HTTP helpers

pub enum ApiError {
    InvalidJson,
    PayloadTooLarge,
    Internal(Fail),
}

impl From<Fail> for ApiError {
    fn from(error: Fail) -> Self {
        Self::Internal(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidJson => {
                json_response(StatusCode::BAD_REQUEST, &json!({ "error": "invalid_json" }))
            }
            Self::PayloadTooLarge => json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &json!({ "error": "payload_too_large" }),
            ),
            Self::Internal(error) => {
                eprintln!("[oauth] request failed: {error}");
                diagnostics::anomaly("internal_error", error.to_string());
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &json!({ "error": "internal_error" }),
                )
            }
        }
    }
}

type ApiResult = Result<Response, ApiError>;

pub fn security_headers(headers: &mut HeaderMap) {
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
}

pub fn json_response(status: StatusCode, payload: &Value) -> Response {
    let mut response = Response::new(Body::from(payload.to_string()));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    security_headers(headers);
    response
}

fn redirect(location: &str) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    security_headers(headers);
    match HeaderValue::from_str(location) {
        Ok(value) => {
            headers.insert(LOCATION, value);
        }
        Err(_) => {
            headers.insert(LOCATION, HeaderValue::from_static("/"));
        }
    }
    response
}

pub async fn not_found() -> Response {
    json_response(StatusCode::NOT_FOUND, &json!({ "error": "not_found" }))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(COOKIE) {
        let Ok(header) = header.to_str() else {
            continue;
        };
        for part in header.split(';') {
            if let Some((key, value)) = part.trim().split_once('=')
                && key == name
            {
                found = Some(
                    percent_decode_str(value)
                        .decode_utf8()
                        .map(|decoded| decoded.into_owned())
                        .unwrap_or_else(|_| value.to_owned()),
                );
            }
        }
    }
    found.filter(|value| !value.is_empty())
}

fn query_param(query: Option<&str>, name: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

async fn read_json_body(body: Body, max_bytes: usize) -> Result<Value, ApiError> {
    let mut body = body;
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| ApiError::Internal(Fail(format!("body:{error}"))))?;
        if let Ok(chunk) = frame.into_data() {
            if bytes.len() + chunk.len() > max_bytes {
                return Err(ApiError::PayloadTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
    }

    let text = String::from_utf8_lossy(&bytes);
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|_| ApiError::InvalidJson)
}

// ---------------------------------------------------------------- sessions

fn expires_at(token: &Value) -> i64 {
    let seconds = match token.get("expires_in") {
        Some(Value::Number(number)) => number.as_f64().filter(|n| *n != 0.0),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .unwrap_or(3600.0);
    now_ms() + (seconds * 1000.0) as i64
}

/// Refreshes the Discord data of a session when it is stale (or `force`d),
/// renewing the access token first when it is about to expire.
async fn update_session_data(
    app: &App,
    session: &mut SessionData,
    force: bool,
) -> Result<(), Fail> {
    let cfg = &app.cfg;
    let token_needs_refresh = now_ms() >= session.expires_at - 30_000;
    let has_fresh_data = !force
        && session.user.is_some()
        && session.connections.is_array()
        && session.updated_at != 0
        && now_ms() - session.updated_at < cfg.session_data_ttl_ms
        && !token_needs_refresh;
    if has_fresh_data {
        return Ok(());
    }

    if token_needs_refresh {
        if session.refresh_token.is_empty() {
            return Err(Fail("token_expired".to_owned()));
        }
        let refreshed = app
            .discord
            .refresh_token(cfg, &session.refresh_token)
            .await?;
        session.access_token = js_str(refreshed.get("access_token"));
        let refresh_token = js_str(refreshed.get("refresh_token"));
        if !refresh_token.is_empty() {
            session.refresh_token = refresh_token;
        }
        session.expires_at = expires_at(&refreshed);
    }

    let (user, connections) = tokio::try_join!(
        app.discord.user(&session.access_token),
        app.discord.connections(&session.access_token)
    )?;
    let guild_profile = app
        .discord
        .guild_profile(cfg, &session.access_token, &user)
        .await;

    let profile = {
        let _guard = diagnostics::timed_lock("profiles", &app.profile_lock).await;
        let created = store::upsert_profile(cfg, &user).await?;
        let user_id = js_str(user.get("id"));
        let updated = store::update_profile_entry(cfg, &user_id, |entry| {
            let public_mode = entry.get("public_mode") == Some(&Value::Bool(true));
            entry.insert("last_login_at".to_owned(), json!(now_iso()));
            entry.insert("username".to_owned(), json!(js_str(user.get("username"))));
            entry.insert(
                "global_name".to_owned(),
                json!(js_str(user.get("global_name"))),
            );
            entry.insert("public_mode".to_owned(), json!(public_mode));
            entry.insert(
                "public_profile".to_owned(),
                if public_mode {
                    discord::public_profile_snapshot(&user, &connections, guild_profile.as_ref())
                } else {
                    Value::Null
                },
            );
            public_mode
        })
        .await?;
        updated.unwrap_or(created)
    };

    session.user = Some(user);
    session.connections = connections;
    session.guild_profile = guild_profile;
    session.updated_at = now_ms();
    session.profile = Some(profile);
    app.sessions_dirty.store(true, Ordering::Relaxed);
    Ok(())
}

/// Resolves the logged-in session of a request and refreshes its data.
/// `Err` carries the ready 401 response.
async fn authenticated(
    app: &App,
    headers: &HeaderMap,
) -> Result<(String, Arc<Session>), Box<Response>> {
    let Some((session_id, session)) = app.read_session(headers) else {
        return Err(Box::new(json_response(
            StatusCode::UNAUTHORIZED,
            &json!({ "authenticated": false, "error": "not_authenticated" }),
        )));
    };

    let refreshed = update_session_data(
        app,
        &mut *diagnostics::timed_lock("session", &session.data).await,
        false,
    )
    .await;
    if refreshed.is_err() {
        return Err(Box::new(app.expire_session(
            &session_id,
            StatusCode::UNAUTHORIZED,
            json!({ "authenticated": false, "error": "session_expired" }),
        )));
    }
    Ok((session_id, session))
}

// ---------------------------------------------------------------- handlers

pub async fn health() -> Response {
    json_response(
        StatusCode::OK,
        &json!({ "ok": true, "service": "discord-oauth" }),
    )
}

pub async fn discord_login(State(app): State<SharedApp>, RawQuery(query): RawQuery) -> Response {
    let cfg = &app.cfg;
    if cfg.discord_client_id.is_empty()
        || cfg.discord_client_secret.is_empty()
        || cfg.discord_redirect_uri.is_empty()
    {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &json!({
                "authenticated": false,
                "error": "missing_server_config",
                "required_env": ["DISCORD_CLIENT_ID", "DISCORD_CLIENT_SECRET", "DISCORD_REDIRECT_URI"],
            }),
        );
    }

    let return_to =
        sanitize_return_to(&query_param(query.as_deref(), "return_to").unwrap_or_default());
    let state = random_hex(20);
    lock(&app.states).insert(
        state.clone(),
        LoginState {
            created_at: now_ms(),
            return_to,
        },
    );

    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &cfg.discord_client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &cfg.discord_redirect_uri)
        .append_pair("scope", "identify connections guilds guilds.members.read")
        .append_pair("state", &state)
        .append_pair("prompt", "consent")
        .finish();
    redirect(&format!("{}?{params}", app.discord.authorize_url()))
}

pub async fn discord_callback(
    State(app): State<SharedApp>,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let param = |name: &str| query_param(query.as_deref(), name).filter(|value| !value.is_empty());

    let login_state = param("state").and_then(|state| lock(&app.states).remove(&state));
    let Some(login_state) = login_state else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "authenticated": false, "error": "invalid_state" }),
        ));
    };
    let return_to = sanitize_return_to(&login_state.return_to);

    if let Some(error) = param("error") {
        return Ok(redirect(&format!(
            "{return_to}?oauth_error={}",
            encode_uri_component(&error)
        )));
    }
    let Some(code) = param("code") else {
        return Ok(redirect(&format!("{return_to}?oauth_error=missing_code")));
    };

    // A failed login sends the visitor back to the site with a readable
    // message instead of a raw error response.
    let login_failed = |error: Fail| {
        eprintln!("[oauth] login failed: {error}");
        diagnostics::anomaly("login_failed", error.to_string());
        redirect(&format!("{return_to}?oauth_error=login_failed"))
    };

    let token = match app.discord.exchange_code(&app.cfg, &code).await {
        Ok(token) => token,
        Err(error) => return Ok(login_failed(error)),
    };
    let mut data = SessionData {
        updated_at: 0,
        access_token: js_str(token.get("access_token")),
        refresh_token: js_str(token.get("refresh_token")),
        expires_at: expires_at(&token),
        user: None,
        connections: json!([]),
        guild_profile: None,
        profile: None,
    };
    if let Err(error) = update_session_data(&app, &mut data, true).await {
        return Ok(login_failed(error));
    }

    let destination = data
        .profile
        .as_ref()
        .and_then(|profile| profile.get("url"))
        .and_then(Value::as_str)
        .unwrap_or(&return_to)
        .trim_end_matches('/')
        .to_owned();

    let session_id = random_hex(24);
    app.sessions_dirty.store(true, Ordering::Relaxed);
    lock(&app.sessions).insert(
        session_id.clone(),
        Arc::new(Session {
            last_seen_at: AtomicI64::new(now_ms()),
            data: tokio::sync::Mutex::new(data),
        }),
    );

    let mut response = redirect(if destination.is_empty() {
        "/"
    } else {
        &destination
    });
    response.headers_mut().insert(
        SET_COOKIE,
        app.session_cookie(&session_id, SESSION_TTL_MS / 1000),
    );
    Ok(response)
}

pub async fn session(
    State(app): State<SharedApp>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let refresh = query_param(query.as_deref(), "refresh").as_deref() == Some("1");
    let Some((session_id, session)) = app.read_session(&headers) else {
        return json_response(StatusCode::OK, &json!({ "authenticated": false }));
    };

    let mut data = diagnostics::timed_lock("session", &session.data).await;
    if update_session_data(&app, &mut data, refresh).await.is_err() {
        drop(data);
        return app.expire_session(
            &session_id,
            StatusCode::OK,
            json!({ "authenticated": false, "expired": true }),
        );
    }

    let user = data.user.as_ref().unwrap_or(&Value::Null);
    json_response(
        StatusCode::OK,
        &json!({
            "authenticated": true,
            "is_admin": app.is_admin(data.user.as_ref()),
            "user": discord::to_public_user(user, data.guild_profile.as_ref()),
            "connections": discord::to_public_connections(&data.connections),
            "profile": data.profile,
            "updated_at": iso_from_ms(if data.updated_at != 0 { data.updated_at } else { now_ms() }),
        }),
    )
}

pub async fn public_profile(State(app): State<SharedApp>, RawQuery(query): RawQuery) -> ApiResult {
    let slug = sanitize_slug(&query_param(query.as_deref(), "slug").unwrap_or_default());
    if slug.is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "invalid_slug" }),
        ));
    }

    let users = store::read_profile_db(&app.cfg).await?;
    let Some(entry) = store::find_profile_by_slug(&users, &slug).filter(|entry| truthy(entry))
    else {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "profile_not_found" }),
        ));
    };
    if entry.get("public_mode") != Some(&Value::Bool(true)) {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "profile_private" }),
        ));
    }
    let Some(profile) = entry
        .get("public_profile")
        .filter(|profile| truthy(profile))
    else {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "public_profile_not_ready" }),
        ));
    };

    Ok(json_response(
        StatusCode::OK,
        &json!({ "public_mode": true, "slug": entry.get("slug"), "profile": profile }),
    ))
}

async fn read_invite_profile(app: SharedApp, server: &'static ServerConfig, force: bool) -> Value {
    let cached = {
        let cache = lock(&app.invite_cache);
        cache
            .get(server.primary_invite())
            .map(|entry| (entry.fetched_at, entry.payload.clone()))
    };

    if let Some((fetched_at, mut payload)) = cached.clone()
        && !force
        && now_ms() - fetched_at < app.cfg.invite_cache_ttl_ms
    {
        payload.insert("stale".to_owned(), json!(false));
        payload.insert("cached".to_owned(), json!(true));
        return Value::Object(payload);
    }

    match app.discord.invite_snapshot(server).await {
        Ok(mut payload) => {
            lock(&app.invite_cache).insert(
                server.primary_invite(),
                InviteCacheEntry {
                    fetched_at: now_ms(),
                    payload: payload.clone(),
                },
            );
            payload.insert("cached".to_owned(), json!(false));
            Value::Object(payload)
        }
        Err(error) => {
            if let Some((_, mut payload)) = cached {
                payload.insert("stale".to_owned(), json!(true));
                payload.insert("cached".to_owned(), json!(true));
                payload.insert("error".to_owned(), json!("refresh_failed"));
                return Value::Object(payload);
            }

            let mut payload = discord::server_identity(server, 0);
            let empty = json!({
                "guild_id": "",
                "description": "",
                "member_count": 0,
                "online_count": 0,
                "verification_level": null,
                "premium_tier": 0,
                "premium_subscription_count": 0,
                "icon_url": "",
                "source_url": app.discord.invite_url(server.primary_invite()),
                "updated_at": "",
                "stale": true,
                "cached": false,
                "error": error.to_string(),
            });
            if let Value::Object(empty) = empty {
                payload.extend(empty);
            }
            Value::Object(payload)
        }
    }
}

pub async fn server_data(State(app): State<SharedApp>, RawQuery(query): RawQuery) -> Response {
    let refresh = query_param(query.as_deref(), "refresh").as_deref() == Some("1");
    let tasks: Vec<_> = PUBLIC_SERVERS
        .iter()
        .map(|server| {
            tokio::spawn(diagnostics::with_spans(
                diagnostics::current_spans(),
                read_invite_profile(app.clone(), server, refresh),
            ))
        })
        .collect();

    let mut servers = Vec::with_capacity(tasks.len());
    for task in tasks {
        servers.push(task.await.unwrap_or(Value::Null));
    }
    json_response(
        StatusCode::OK,
        &json!({ "updated_at": now_iso(), "servers": servers }),
    )
}

pub async fn changelog_list(State(app): State<SharedApp>) -> ApiResult {
    let mut posts = store::read_changelog_posts(&app.cfg).await?;
    let created = |post: &Value| {
        post.get("created_at")
            .and_then(Value::as_str)
            .and_then(parse_iso_ms)
            .unwrap_or(0)
    };
    posts.sort_by_key(|post| std::cmp::Reverse(created(post)));
    Ok(json_response(StatusCode::OK, &json!({ "posts": posts })))
}

pub async fn changelog_create(
    State(app): State<SharedApp>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let (_, session) = match authenticated(&app, &headers).await {
        Ok(found) => found,
        Err(response) => return Ok(*response),
    };
    let author = {
        let data = diagnostics::timed_lock("session", &session.data).await;
        if !app.is_admin(data.user.as_ref()) {
            return Ok(json_response(
                StatusCode::FORBIDDEN,
                &json!({ "error": "admin_required" }),
            ));
        }
        let user = data.user.as_ref().unwrap_or(&Value::Null);
        json!({
            "id": js_str(user.get("id")),
            "username": js_str(user.get("username")),
            "global_name": js_str(user.get("global_name")),
        })
    };

    let body = read_json_body(body, app.cfg.changelog_json_body_limit).await?;
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "content_required" }),
        ));
    }
    let media_url = sanitize_http_url(body.get("media_url"));

    let post = store::normalize_changelog_post(&json!({
        "id": format!("post_{}_{}", now_ms(), random_hex(6)),
        "title": body.get("title").and_then(Value::as_str).unwrap_or_default(),
        "content": content,
        "media_type": infer_media_type(&media_url, body.get("media_type")),
        "media_url": media_url,
        "created_at": now_iso(),
        "created_by": author,
    }))
    .unwrap_or(Value::Null);

    {
        let _guard = diagnostics::timed_lock("changelog", &app.changelog_lock).await;
        let mut posts = store::read_changelog_posts(&app.cfg).await?;
        posts.insert(0, post.clone());
        store::write_changelog_posts(&app.cfg, posts).await?;
    }

    Ok(json_response(
        StatusCode::CREATED,
        &json!({ "ok": true, "post": post }),
    ))
}

pub async fn profile_mode(
    State(app): State<SharedApp>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    let (_, session) = match authenticated(&app, &headers).await {
        Ok(found) => found,
        Err(response) => return Ok(*response),
    };

    let body = read_json_body(body, app.cfg.json_body_limit).await?;
    let Some(public_mode) = body.get("public_mode").and_then(Value::as_bool) else {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "invalid_public_mode" }),
        ));
    };

    let mut data = diagnostics::timed_lock("session", &session.data).await;
    let user = data.user.clone().unwrap_or(Value::Null);
    let user_id = js_str(user.get("id"));
    if user_id.is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "missing_user_id" }),
        ));
    }

    let updated = {
        let _guard = diagnostics::timed_lock("profiles", &app.profile_lock).await;
        let snapshot = public_mode.then(|| {
            discord::public_profile_snapshot(&user, &data.connections, data.guild_profile.as_ref())
        });
        store::update_profile_entry(&app.cfg, &user_id, |entry| {
            entry.insert("public_mode".to_owned(), json!(public_mode));
            entry.insert("public_profile".to_owned(), snapshot.unwrap_or(Value::Null));
            public_mode
        })
        .await?
    };
    let Some(profile) = updated else {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "profile_not_found" }),
        ));
    };

    data.profile = Some(profile.clone());
    app.sessions_dirty.store(true, Ordering::Relaxed);
    Ok(json_response(
        StatusCode::OK,
        &json!({ "ok": true, "profile": profile }),
    ))
}

pub async fn profile_delete(State(app): State<SharedApp>, headers: HeaderMap) -> ApiResult {
    let (session_id, session) = match authenticated(&app, &headers).await {
        Ok(found) => found,
        Err(response) => return Ok(*response),
    };

    let user_id = js_str(
        session
            .data
            .lock()
            .await
            .user
            .as_ref()
            .and_then(|user| user.get("id")),
    );
    if user_id.is_empty() {
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "missing_user_id" }),
        ));
    }

    let deleted = {
        let _guard = diagnostics::timed_lock("profiles", &app.profile_lock).await;
        store::delete_profile(&app.cfg, &user_id).await?
    };
    let Some(slug) = deleted else {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "profile_not_found" }),
        ));
    };

    Ok(app.expire_session(
        &session_id,
        StatusCode::OK,
        json!({ "ok": true, "deleted": true, "slug": slug }),
    ))
}

pub async fn logout(State(app): State<SharedApp>, headers: HeaderMap) -> Response {
    let session_id = cookie_value(&headers, &app.cfg.cookie_name).unwrap_or_default();
    app.expire_session(
        &session_id,
        StatusCode::OK,
        json!({ "ok": true, "authenticated": false }),
    )
}

/// Removes an uploaded media directory when the caller knows its delete token.
async fn delete_published_media(cfg: &Config, id: &str, token: &str) -> bool {
    if !is_lower_hex(id, 32) || !is_lower_hex(token, 64) {
        return false;
    }

    let directory = cfg.media_root.join(id);
    let Ok(stored) = tokio::fs::read_to_string(directory.join(".delete-token")).await else {
        return false;
    };
    if !constant_time_eq(stored.trim().as_bytes(), token.as_bytes()) {
        return false;
    }
    tokio::fs::remove_dir_all(&directory).await.is_ok()
}

pub async fn media_delete(State(app): State<SharedApp>, body: Body) -> ApiResult {
    let body = read_json_body(body, MEDIA_DELETE_BODY_LIMIT).await?;
    let field = |key: &str| body.get(key).and_then(Value::as_str).unwrap_or_default();

    if !delete_published_media(&app.cfg, field("id"), field("token")).await {
        return Ok(json_response(
            StatusCode::NOT_FOUND,
            &json!({ "error": "media_not_found_or_invalid_token" }),
        ));
    }
    Ok(json_response(
        StatusCode::OK,
        &json!({ "ok": true, "deleted": true }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_str(cookie).unwrap());
        headers
    }

    #[test]
    fn cookie_parsing() {
        let parsed = headers("a=1; bpf_session=abc%20def; other=x=y");
        assert_eq!(
            cookie_value(&parsed, "bpf_session").as_deref(),
            Some("abc def")
        );
        assert_eq!(cookie_value(&parsed, "other").as_deref(), Some("x=y"));
        assert_eq!(cookie_value(&parsed, "missing"), None);
        assert_eq!(
            cookie_value(&headers("s=1; s=2"), "s").as_deref(),
            Some("2")
        );
        assert_eq!(cookie_value(&headers("s=%zz"), "s").as_deref(), Some("%zz"));
        assert_eq!(cookie_value(&headers("s="), "s"), None);
    }

    #[test]
    fn query_parsing() {
        assert_eq!(
            query_param(Some("a=1&return_to=%2Fprofile%2F&a=2"), "return_to").as_deref(),
            Some("/profile/")
        );
        assert_eq!(query_param(Some("a=1&a=2"), "a").as_deref(), Some("1"));
        assert_eq!(query_param(Some("a=1"), "b"), None);
        assert_eq!(query_param(None, "a"), None);
    }
}
