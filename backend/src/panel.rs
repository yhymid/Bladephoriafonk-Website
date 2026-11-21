//! Read-only admin panel on a separate port, bound to 127.0.0.1 only and
//! reached through an SSH tunnel (`scripts/admin-tunnel.sh`). Off unless
//! `ADMIN_PANEL_PORT` is set.
//!
//! Server-rendered HTML without JavaScript or forms: an overview (version,
//! memory, sessions, which Discord invite works), the diagnostics reports,
//! the data files and the service log. Secrets are never shown; log lines
//! are masked before they are rendered.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{RawQuery, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HOST};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::Value;

use crate::api::{SharedApp, query_param};
use crate::check;
use crate::diagnostics;
use crate::discord::PUBLIC_SERVERS;
use crate::util::{iso_from_ms, now_ms};

const CSS: &str = include_str!("panel.css");
const CSP: &str = "default-src 'none'; style-src 'self'; img-src 'self' data:; \
                   form-action 'self'; frame-ancestors 'none'; base-uri 'none'";
const LOG_LINES: usize = 300;
const LOG_TIMEOUT: Duration = Duration::from_secs(10);

/// What the overview shows about the running process.
#[derive(Clone)]
pub struct ProcessInfo {
    pub commit: Option<String>,
    pub started: Instant,
    pub started_ms: i64,
}

#[derive(Clone)]
struct Panel {
    app: SharedApp,
    info: ProcessInfo,
}

/// Serves the panel until `stop` turns true. A failure to bind is reported
/// and leaves the website running.
pub async fn serve(
    app: SharedApp,
    info: ProcessInfo,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let port = app.cfg.admin_panel_port;
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("[panel] cannot listen on 127.0.0.1:{port}: {error}");
            diagnostics::anomaly("panel_failed", format!("127.0.0.1:{port}: {error}"));
            return;
        }
    };
    println!("Admin panel: http://127.0.0.1:{port} (SSH tunnel only)");
    let shutdown = async move {
        let _ = stop.wait_for(|stopped| *stopped).await;
    };
    if let Err(error) = axum::serve(listener, router(Panel { app, info }))
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("[panel] {error}");
    }
}

fn router(panel: Panel) -> Router {
    Router::new()
        .route("/", get(overview))
        .route("/diagnostics", get(diagnostics_page))
        .route("/data", get(data_page))
        .route("/logs", get(logs_page))
        .route("/style.css", get(stylesheet))
        .fallback(|| async {
            page_response(
                StatusCode::NOT_FOUND,
                "404",
                "",
                "<p>Nie ma takiej strony.</p>",
            )
        })
        .layer(axum::middleware::from_fn(local_only))
        .with_state(panel)
}

/// Accepts only requests addressed to the loopback name, whatever the
/// tunnel's local port. Blocks DNS rebinding: a page on another site that
/// resolves its name to 127.0.0.1 still sends its own Host header.
fn is_local_host(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split_once(']').map(|(name, _)| name)
    } else {
        Some(host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.chars().all(|ch| ch.is_ascii_digit()) {
                name
            } else {
                host
            }
        }))
    };
    matches!(name, Some("127.0.0.1" | "localhost" | "::1"))
}

async fn local_only(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !is_local_host(host) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert("content-security-policy", HeaderValue::from_static(CSP));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn stylesheet() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], CSS).into_response()
}

// ------------------------------------------------------------------ helpers

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn fmt_bytes(bytes: u64) -> String {
    let value = bytes as f64;
    if value >= 1024.0 * 1024.0 {
        format!("{:.1} MB", value / 1024.0 / 1024.0)
    } else if value >= 1024.0 {
        format!("{:.1} KB", value / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min {} s", seconds % 60)
    }
}

/// Polish plural: 1 wpis, 2-4 wpisy, 5+ wpisów (and 12-14 wpisów).
fn plural(count: usize, one: &str, few: &str, many: &str) -> String {
    let word = if count == 1 {
        one
    } else if (2..=4).contains(&(count % 10)) && !(12..=14).contains(&(count % 100)) {
        few
    } else {
        many
    };
    format!("{count} {word}")
}

fn badge(class: &str, text: &str) -> String {
    format!("<span class=\"badge {class}\">{}</span>", esc(text))
}

fn page_response(status: StatusCode, title: &str, active: &str, body: &str) -> Response {
    let mut nav = String::new();
    for (href, name) in [
        ("/", "Przegląd"),
        ("/diagnostics", "Diagnostyka"),
        ("/data", "Dane"),
        ("/logs", "Logi"),
    ] {
        let class = if href == active {
            " class=\"active\""
        } else {
            ""
        };
        let _ = write!(nav, "<a href=\"{href}\"{class}>{name}</a>");
    }
    let html = format!(
        "<!DOCTYPE html><html lang=\"pl\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\">\
         <title>{} · Bladephoriafonk admin</title><link rel=\"stylesheet\" href=\"/style.css\"></head>\
         <body><header><span class=\"brand\">Bladephoriafonk</span><nav>{nav}</nav>\
         <span class=\"note\">tylko odczyt</span></header><main><h1>{}</h1>{body}</main></body></html>",
        esc(title),
        esc(title)
    );
    (status, [(CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

fn page(title: &str, active: &str, body: &str) -> Response {
    page_response(StatusCode::OK, title, active, body)
}

fn rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `summary-YYYY-MM-DD.md` files, newest first.
fn summary_days(dir: &Path) -> Vec<String> {
    let mut days: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let day = name
                        .strip_prefix("summary-")?
                        .strip_suffix(".md")?
                        .to_owned();
                    is_day(&day).then_some(day)
                })
                .collect()
        })
        .unwrap_or_default();
    days.sort_unstable_by(|a, b| b.cmp(a));
    days
}

fn is_day(text: &str) -> bool {
    text.len() == 10
        && text.char_indices().all(|(index, ch)| {
            if index == 4 || index == 7 {
                ch == '-'
            } else {
                ch.is_ascii_digit()
            }
        })
}

/// One `## Heading` section of a summary, without the heading.
fn summary_section<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
    let start = markdown.find(&format!("\n## {heading}"))? + 1;
    let body = &markdown[start..];
    let body = &body[body.find('\n')? + 1..];
    Some(body.find("\n## ").map_or(body, |end| &body[..end]))
}

// ------------------------------------------------------- markdown rendering

/// Inline Markdown of the generated reports: `code` and _emphasis_.
fn inline(text: &str) -> String {
    let mut out = String::new();
    for (index, part) in text.split('`').enumerate() {
        if index % 2 == 1 {
            let _ = write!(out, "<code>{}</code>", esc(part));
        } else {
            let escaped = esc(part);
            match escaped
                .strip_prefix('_')
                .and_then(|rest| rest.strip_suffix('_'))
            {
                Some(emphasis) if !emphasis.is_empty() => {
                    let _ = write!(out, "<em>{emphasis}</em>");
                }
                _ => out.push_str(&escaped),
            }
        }
    }
    out
}

fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

/// Renders the Markdown that the diagnostics recorder writes: headings,
/// lists, tables (right-aligned columns from `---:`) and paragraphs.
/// Everything is escaped first, so a report can never inject HTML.
fn render_markdown(markdown: &str) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(text) = line.strip_prefix("# ") {
            let _ = write!(out, "<h2>{}</h2>", inline(text));
        } else if let Some(text) = line.strip_prefix("## ") {
            let _ = write!(out, "<h3>{}</h3>", inline(text));
        } else if line.starts_with("- ") {
            out.push_str("<ul>");
            while index < lines.len() && lines[index].starts_with("- ") {
                let _ = write!(out, "<li>{}</li>", inline(&lines[index][2..]));
                index += 1;
            }
            out.push_str("</ul>");
            continue;
        } else if line.starts_with('|') {
            let header = table_cells(line);
            let numeric: Vec<bool> = lines
                .get(index + 1)
                .filter(|next| next.starts_with("|-") || next.starts_with("|:"))
                .map(|next| {
                    table_cells(next)
                        .iter()
                        .map(|cell| cell.ends_with(':'))
                        .collect()
                })
                .unwrap_or_default();
            let class = |column: usize| {
                if numeric.get(column).copied().unwrap_or(false) {
                    " class=\"num\""
                } else {
                    ""
                }
            };
            out.push_str("<div class=\"scroll\"><table><thead><tr>");
            for (column, cell) in header.iter().enumerate() {
                let _ = write!(out, "<th{}>{}</th>", class(column), inline(cell));
            }
            out.push_str("</tr></thead><tbody>");
            index += if numeric.is_empty() { 1 } else { 2 };
            while index < lines.len() && lines[index].starts_with('|') {
                out.push_str("<tr>");
                for (column, cell) in table_cells(lines[index]).iter().enumerate() {
                    let _ = write!(out, "<td{}>{}</td>", class(column), inline(cell));
                }
                out.push_str("</tr>");
                index += 1;
            }
            out.push_str("</tbody></table></div>");
            continue;
        } else if !line.trim().is_empty() {
            let _ = write!(out, "<p>{}</p>", inline(line));
        }
        index += 1;
    }
    out
}

// -------------------------------------------------------------------- pages

async fn overview(State(panel): State<Panel>) -> Response {
    let app = &panel.app;
    let cfg = &app.cfg;
    let state = app.panel_state();
    let mut body = String::new();

    body.push_str("<div class=\"tiles\">");
    let mut tile = |label: &str, value: &str| {
        let _ = write!(
            body,
            "<div class=\"tile\"><span>{}</span><strong>{}</strong></div>",
            esc(label),
            value
        );
    };
    tile(
        "Wersja",
        &esc(&format!(
            "{} · {}",
            env!("CARGO_PKG_VERSION"),
            panel.info.commit.as_deref().unwrap_or("brak commita")
        )),
    );
    tile(
        "Działa od",
        &esc(&format!(
            "{} ({} UTC)",
            fmt_duration(panel.info.started.elapsed()),
            iso_from_ms(panel.info.started_ms)[..16].replace('T', " ")
        )),
    );
    tile("PID", &std::process::id().to_string());
    tile(
        "RAM (RSS)",
        &rss_kb().map_or_else(|| "?".to_owned(), |kb| fmt_bytes(kb * 1024)),
    );
    tile("Zalogowane sesje", &state.sessions.to_string());
    tile("Logowania w toku", &state.login_states.to_string());
    tile(
        "Diagnostyka",
        &if diagnostics::enabled() {
            badge("ok", "włączona")
        } else {
            badge("warn", "wyłączona")
        },
    );
    body.push_str("</div>");

    body.push_str("<h2>Serwery Discord</h2><div class=\"scroll\"><table><thead><tr><th>Serwer</th>\
                   <th>Działające zaproszenie</th><th>Kody na liście</th><th class=\"num\">Członkowie</th>\
                   <th class=\"num\">Online</th><th>Pobrane</th></tr></thead><tbody>");
    for server in PUBLIC_SERVERS {
        let cached = state
            .invites
            .iter()
            .find(|(key, _, _)| key == server.primary_invite());
        let (status, members, online, age) = match cached {
            Some((_, fetched_at, payload)) => {
                let used = payload
                    .get("invite_code_used")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let position = server.invite_codes.iter().position(|code| *code == used);
                let status = match position {
                    Some(0) => format!("{} {}", badge("ok", "główne"), esc(used)),
                    Some(index) => format!(
                        "{} {} <span class=\"muted\">(główne wygasło, użyte nr {})</span>",
                        badge("warn", "zapasowe"),
                        esc(used),
                        index + 1
                    ),
                    None => badge("bad", "nieznane"),
                };
                let number = |key: &str| {
                    payload
                        .get(key)
                        .and_then(Value::as_u64)
                        .map_or_else(|| "?".to_owned(), |value| value.to_string())
                };
                (
                    status,
                    number("member_count"),
                    number("online_count"),
                    format!(
                        "{} temu",
                        fmt_duration(Duration::from_millis(
                            u64::try_from(now_ms() - fetched_at).unwrap_or(0)
                        ))
                    ),
                )
            }
            None => (
                format!(
                    "{} <span class=\"muted\">nikt nie otworzył /server od startu</span>",
                    badge("warn", "brak danych")
                ),
                "—".to_owned(),
                "—".to_owned(),
                "—".to_owned(),
            ),
        };
        let _ = write!(
            body,
            "<tr><td>{}</td><td>{status}</td><td class=\"num\">{}</td><td class=\"num\">{members}</td>\
             <td class=\"num\">{online}</td><td>{}</td></tr>",
            esc(server.name),
            server.invite_codes.len(),
            esc(&age)
        );
    }
    body.push_str("</tbody></table></div>");

    let dir = &cfg.diagnostics.dir;
    if let Some(day) = summary_days(dir).first() {
        if let Ok(summary) = std::fs::read_to_string(dir.join(format!("summary-{day}.md"))) {
            for (heading, title) in [("Wskazówki", "Wskazówki"), ("Anomalie", "Anomalie")] {
                if let Some(section) = summary_section(&summary, heading) {
                    let _ = write!(
                        body,
                        "<h2>{title} <span class=\"muted\">({day})</span></h2>{}",
                        render_markdown(section)
                    );
                }
            }
        }
    } else {
        body.push_str("<p class=\"muted\">Brak raportów diagnostyki.</p>");
    }
    page("Przegląd", "/", &body)
}

async fn diagnostics_page(State(panel): State<Panel>, RawQuery(query): RawQuery) -> Response {
    let dir = &panel.app.cfg.diagnostics.dir;
    let days = summary_days(dir);
    let wanted = query_param(query.as_deref(), "day").filter(|day| is_day(day));
    let Some(day) = wanted.or_else(|| days.first().cloned()) else {
        return page(
            "Diagnostyka",
            "/diagnostics",
            "<p class=\"muted\">Brak raportów. Włącz <code>BLADEPHORIA_DIAGNOSTICS=1</code>.</p>",
        );
    };
    let mut body = String::from("<p class=\"days\">");
    for other in &days {
        let class = if *other == day {
            " class=\"active\""
        } else {
            ""
        };
        let _ = write!(
            body,
            "<a href=\"/diagnostics?day={other}\"{class}>{other}</a>"
        );
    }
    body.push_str("</p>");
    match std::fs::read_to_string(dir.join(format!("summary-{day}.md"))) {
        Ok(summary) => body.push_str(&render_markdown(&summary)),
        Err(_) => {
            let _ = write!(body, "<p class=\"muted\">Brak raportu z {day}.</p>");
        }
    }
    page("Diagnostyka", "/diagnostics", &body)
}

fn file_row(body: &mut String, path: &Path, what: &str, strict: bool) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|since| i64::try_from(since.as_millis()).ok())
                .map(|ms| iso_from_ms(ms)[..19].replace('T', " "))
                .unwrap_or_default();
            let state = if what.is_empty() {
                String::new()
            } else {
                match std::fs::read_to_string(path)
                    .ok()
                    .map(|text| check::data_problem(what, &text, strict))
                {
                    Some(None) => badge("ok", "poprawny"),
                    Some(Some(problem)) => badge("bad", &problem),
                    None => badge("bad", "nie da się odczytać"),
                }
            };
            let _ = write!(
                body,
                "<tr><td><code>{}</code></td><td class=\"num\">{}</td><td>{modified}</td><td>{state}</td></tr>",
                esc(&name),
                fmt_bytes(meta.len())
            );
        }
        Err(_) => {
            let _ = write!(
                body,
                "<tr><td><code>{}</code></td><td class=\"num\">—</td><td>—</td><td>{}</td></tr>",
                esc(&name),
                badge("warn", "nie istnieje")
            );
        }
    }
}

async fn data_page(State(panel): State<Panel>) -> Response {
    let cfg = &panel.app.cfg;
    let strict = cfg.strict_data_files;
    let mut body = String::from(
        "<h2>Pliki danych</h2><div class=\"scroll\"><table><thead><tr><th>Plik</th>\
         <th class=\"num\">Rozmiar</th><th>Zmieniony (UTC)</th><th>Stan</th></tr></thead><tbody>",
    );
    file_row(&mut body, &cfg.profile_db_file, "profiles", strict);
    file_row(&mut body, &cfg.changelog_db_file, "changelog", strict);
    file_row(&mut body, &cfg.session_db_file, "sessions", false);

    // Backups and quarantine copies next to the data files.
    let mut extra: Vec<PathBuf> = Vec::new();
    for file in [
        &cfg.profile_db_file,
        &cfg.changelog_db_file,
        &cfg.session_db_file,
    ] {
        let (Some(dir), Some(base)) = (file.parent(), file.file_name()) else {
            continue;
        };
        let base = base.to_string_lossy().into_owned();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != base && name.starts_with(&base) && !extra.contains(&entry.path()) {
                    extra.push(entry.path());
                }
            }
        }
    }
    extra.sort();
    for path in &extra {
        file_row(&mut body, path, "", false);
    }
    body.push_str("</tbody></table></div>");
    if extra
        .iter()
        .any(|path| path.to_string_lossy().contains(".corrupt-"))
    {
        body.push_str(
            "<p>Są kopie uszkodzonych plików (<code>.corrupt-*</code>). Co z nimi zrobić, pokaże \
             <code>bladephoria-web --check-config</code>.</p>",
        );
    }

    match crate::store::read_changelog_posts(cfg).await {
        Ok(posts) => {
            let _ = write!(
                body,
                "<h2>Changelog <span class=\"muted\">({})</span></h2><ul>",
                plural(posts.len(), "wpis", "wpisy", "wpisów")
            );
            for post in posts.iter().take(8) {
                let text = |key: &str| post.get(key).and_then(Value::as_str).unwrap_or("");
                let _ = write!(
                    body,
                    "<li><span class=\"muted\">{}</span> {}</li>",
                    esc(&text("created_at").chars().take(10).collect::<String>()),
                    esc(text("title"))
                );
            }
            body.push_str("</ul>");
        }
        Err(error) => {
            let _ = write!(
                body,
                "<h2>Changelog</h2><p>{}</p>",
                badge("bad", &error.to_string())
            );
        }
    }
    match crate::store::read_profile_db(cfg).await {
        Ok(users) => {
            let _ = write!(
                body,
                "<h2>Profile <span class=\"muted\">({})</span></h2><ul>",
                users.len()
            );
            for entry in users.values() {
                let slug = entry.get("slug").and_then(Value::as_str).unwrap_or("?");
                let public = entry.get("public_mode").and_then(Value::as_bool) == Some(true);
                let _ = write!(
                    body,
                    "<li><code>/profile/{}/</code> {}</li>",
                    esc(slug),
                    if public {
                        badge("ok", "publiczny")
                    } else {
                        badge("muted-badge", "prywatny")
                    }
                );
            }
            body.push_str("</ul>");
        }
        Err(error) => {
            let _ = write!(
                body,
                "<h2>Profile</h2><p>{}</p>",
                badge("bad", &error.to_string())
            );
        }
    }
    page("Dane", "/data", &body)
}

// --------------------------------------------------------------------- logs

struct LogLine {
    time_ms: i64,
    priority: u8,
    message: String,
}

/// Masks the Discord client secret and anything that looks like a token
/// (32+ characters of letters, digits, `-`, `_` or `.`), e.g. session ids.
fn mask_secrets(line: &str, secret: &str) -> String {
    let line = if secret.len() >= 8 {
        line.replace(secret, "•••")
    } else {
        line.to_owned()
    };
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= 32 {
            out.push_str("•••");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            out.push(ch);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn parse_journal(output: &str, secret: &str) -> Vec<LogLine> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|entry| {
            // MESSAGE is an array of bytes when it is not valid UTF-8.
            let message = match entry.get("MESSAGE") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Array(bytes)) => String::from_utf8_lossy(
                    &bytes
                        .iter()
                        .filter_map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                        .collect::<Vec<u8>>(),
                )
                .into_owned(),
                _ => String::new(),
            };
            let number = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|text| text.parse::<i64>().ok())
            };
            LogLine {
                time_ms: number("__REALTIME_TIMESTAMP").map_or(0, |us| us / 1000),
                priority: number("PRIORITY")
                    .and_then(|value| u8::try_from(value).ok())
                    .unwrap_or(6),
                message: mask_secrets(&message, secret),
            }
        })
        .collect()
}

/// Lines that are warnings or errors: by syslog priority, or by wording
/// (the server writes its errors to stderr, which journald logs as info).
fn is_problem(line: &LogLine) -> bool {
    if line.priority <= 4 {
        return true;
    }
    let lower = line.message.to_lowercase();
    ["error", "failed", "cannot", "panic", "!!", "warn"]
        .iter()
        .any(|word| lower.contains(word))
}

fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 100
        && unit
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '_' | '-'))
}

/// Runs journalctl without a shell, with a time limit.
fn read_journal(unit: &str) -> Result<String, String> {
    let mut child = std::process::Command::new("journalctl")
        .args(["-u", unit, "-n", &LOG_LINES.to_string(), "-o", "json"])
        .args(["--output-fields=MESSAGE,PRIORITY", "--no-pager", "-q"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("journalctl: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > LOG_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("journalctl did not answer within 10 s".to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(error.to_string()),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

async fn logs_page(State(panel): State<Panel>, RawQuery(query): RawQuery) -> Response {
    let cfg = &panel.app.cfg;
    let only_problems = query_param(query.as_deref(), "errors").as_deref() == Some("1");
    let search = query_param(query.as_deref(), "q")
        .unwrap_or_default()
        .chars()
        .take(100)
        .collect::<String>();
    let unit = cfg.admin_panel_unit.clone();

    let mut body = String::new();
    let filter_link = |errors: bool| {
        let mut href = String::from("/logs");
        if errors {
            href.push_str("?errors=1");
        }
        href
    };
    let _ = write!(
        body,
        "<p class=\"days\"><a href=\"{}\"{}>Wszystko</a><a href=\"{}\"{}>Tylko ostrzeżenia i błędy</a></p>\
         <form method=\"get\" action=\"/logs\" class=\"search\"><input type=\"search\" name=\"q\" value=\"{}\" \
         placeholder=\"Szukaj w logach\">{}<button type=\"submit\">Szukaj</button></form>",
        filter_link(false),
        if only_problems {
            ""
        } else {
            " class=\"active\""
        },
        filter_link(true),
        if only_problems {
            " class=\"active\""
        } else {
            ""
        },
        esc(&search),
        if only_problems {
            "<input type=\"hidden\" name=\"errors\" value=\"1\">"
        } else {
            ""
        }
    );

    if !valid_unit(&unit) {
        let _ = write!(
            body,
            "<p>{}</p>",
            badge("bad", "ADMIN_PANEL_UNIT has invalid characters")
        );
        return page("Logi", "/logs", &body);
    }
    let secret = cfg.discord_client_secret.clone();
    let journal = tokio::task::spawn_blocking(move || read_journal(&unit))
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
    match journal {
        Err(error) => {
            let _ = write!(
                body,
                "<p>{}</p>",
                badge("bad", &mask_secrets(&error, &secret))
            );
        }
        Ok(output) => {
            let needle = search.to_lowercase();
            let mut lines: Vec<LogLine> = parse_journal(&output, &secret)
                .into_iter()
                .filter(|line| !only_problems || is_problem(line))
                .filter(|line| needle.is_empty() || line.message.to_lowercase().contains(&needle))
                .collect();
            lines.reverse();
            let _ = write!(
                body,
                "<p class=\"muted\">{} z ostatnich {LOG_LINES} linii <code>{}</code>, od najnowszych.</p>\
                 <div class=\"scroll\"><table class=\"log\"><thead><tr><th>Czas (UTC)</th><th>Treść</th></tr></thead><tbody>",
                lines.len(),
                esc(&cfg.admin_panel_unit)
            );
            for line in &lines {
                let class = if line.priority <= 3 {
                    " class=\"bad\""
                } else if is_problem(line) {
                    " class=\"warn\""
                } else {
                    ""
                };
                let _ = write!(
                    body,
                    "<tr{class}><td>{}</td><td><code>{}</code></td></tr>",
                    if line.time_ms > 0 {
                        iso_from_ms(line.time_ms)[..19].replace('T', " ")
                    } else {
                        String::new()
                    },
                    esc(&line.message)
                );
            }
            body.push_str("</tbody></table></div>");
        }
    }
    page("Logi", "/logs", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_are_accepted() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:3002",
            "localhost:9000",
            "[::1]:3002",
            "[::1]",
            "localhost",
        ] {
            assert!(is_local_host(host), "{host}");
        }
        for host in [
            "",
            "bladephoriafonk.xyz",
            "evil.example:3002",
            "127.0.0.1.evil.example",
            "localhost.evil",
            "[::2]:1",
        ] {
            assert!(!is_local_host(host), "{host}");
        }
    }

    #[test]
    fn markdown_is_escaped_and_rendered() {
        let html = render_markdown(
            "# Title\n- one `a<b>`\n- two\n\n| Col | n |\n|---|---:|\n| `x` | 5 |\n_Brak._\n<script>",
        );
        assert!(html.contains("<h2>Title</h2>"));
        assert!(html.contains("<li>one <code>a&lt;b&gt;</code></li><li>two</li>"));
        assert!(html.contains("<th class=\"num\">n</th>"));
        assert!(html.contains("<td><code>x</code></td><td class=\"num\">5</td>"));
        assert!(html.contains("<em>Brak.</em>"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn sections_of_a_summary() {
        let summary = "# D\n\n## Anomalie\n\n_Brak._\n\n## Wskazówki\n\n- Brak uwag\n";
        assert_eq!(
            summary_section(summary, "Anomalie").unwrap().trim(),
            "_Brak._"
        );
        assert_eq!(
            summary_section(summary, "Wskazówki").unwrap().trim(),
            "- Brak uwag"
        );
        assert!(summary_section(summary, "Nope").is_none());
    }

    #[test]
    fn secrets_are_masked_in_logs() {
        let line = "token=e2e-access-token-93ce55aaaaaaaaaaaaaaaaaaaaaa secret s3cr3t-value path /home/ubuntu/Main Files";
        let masked = mask_secrets(line, "s3cr3t-value");
        assert_eq!(masked, "token=••• secret ••• path /home/ubuntu/Main Files");
    }

    #[test]
    fn journal_lines_are_parsed() {
        let output = concat!(
            "{\"MESSAGE\":\"[oauth] login failed: x\",\"PRIORITY\":\"6\",\"__REALTIME_TIMESTAMP\":\"1795272800111549\"}\n",
            "{\"MESSAGE\":[104,105],\"PRIORITY\":\"3\"}\n",
            "not json\n"
        );
        let lines = parse_journal(output, "");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, 1_795_272_800_111);
        assert!(is_problem(&lines[0]));
        assert_eq!(lines[1].message, "hi");
        assert!(is_problem(&lines[1]));
        assert!(valid_unit("bladephoria-oauth"));
        assert!(!valid_unit("x; rm -rf /"));
    }

    #[test]
    fn polish_plurals() {
        assert_eq!(plural(1, "wpis", "wpisy", "wpisów"), "1 wpis");
        assert_eq!(plural(3, "wpis", "wpisy", "wpisów"), "3 wpisy");
        assert_eq!(plural(7, "wpis", "wpisy", "wpisów"), "7 wpisów");
        assert_eq!(plural(12, "wpis", "wpisy", "wpisów"), "12 wpisów");
        assert_eq!(plural(22, "wpis", "wpisy", "wpisów"), "22 wpisy");
    }

    #[test]
    fn days_are_validated() {
        assert!(is_day("2026-11-21"));
        assert!(!is_day("../../etc/x"));
        assert!(!is_day("2026-11-2a"));
    }
}
