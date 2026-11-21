//! JSON file storage for profiles and the changelog, plus the generated
//! per-user profile pages. File formats are unchanged from the Node backend.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::Path;
use std::time::Instant;

use serde_json::{Map, Value, json};
use tokio::fs;

use crate::config::Config;
use crate::diagnostics;
use crate::error::Fail;
use crate::util::{
    infer_media_type, js_str, now_iso, now_ms, parse_iso_ms, random_hex, safe_slug,
    sanitize_http_url, sanitize_slug,
};

pub async fn path_exists(path: &Path) -> bool {
    fs::metadata(path).await.is_ok()
}

/// Reads a JSON file; a missing or blank file yields `None`.
pub async fn read_json_file(path: &Path) -> Result<Option<Value>, Fail> {
    let started = Instant::now();
    let text = match fs::read_to_string(path).await {
        Ok(text) => text,
        Err(error) => {
            let missing = error.kind() == ErrorKind::NotFound;
            diagnostics::io("read", path, started.elapsed(), 0, missing);
            return if missing { Ok(None) } else { Err(error.into()) };
        }
    };
    let parsed = if text.trim().is_empty() {
        Ok(None)
    } else {
        serde_json::from_str(&text).map(Some).map_err(|_| ())
    };
    diagnostics::io("read", path, started.elapsed(), text.len(), parsed.is_ok());
    match parsed {
        Ok(value) => Ok(value),
        Err(()) => Err(damaged(path, "invalid_json_file").await),
    }
}

/// FNV-1a: a stable content hash for naming quarantine copies.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Keeps a copy of a damaged data file next to it as
/// `<name>.corrupt-<hash>`, reports it and returns the error for callers.
/// The original is left untouched (and is not overwritten, because every
/// write first reads it successfully); the same content is copied once.
async fn damaged(path: &Path, reason: &str) -> Fail {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut copy_note = String::new();
    if let Ok(content) = fs::read(path).await {
        let mut copy = path.as_os_str().to_owned();
        copy.push(format!(".corrupt-{:016x}", fnv1a(&content)));
        let copy = std::path::PathBuf::from(copy);
        if !path_exists(&copy).await && fs::copy(path, &copy).await.is_ok() {
            eprintln!(
                "[data] {name} is damaged ({reason}), copy kept as {}",
                copy.display()
            );
        }
        copy_note = format!(
            ", copy {}",
            copy.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    diagnostics::anomaly_throttled(
        "damaged_data_file",
        &name,
        format!("{name}: {reason}{copy_note}"),
        std::time::Duration::from_secs(3600),
    );
    Fail(format!("{reason}:{name}"))
}

/// What is wrong with the structure of a data file, if anything.
/// `kind` is "profiles" or "changelog".
pub fn shape_problem(kind: &str, data: &Value) -> Option<&'static str> {
    let field = match kind {
        "profiles" => data.get("users").map(Value::is_object),
        "changelog" => data.get("posts").map(Value::is_array),
        _ => return None,
    };
    match (data.is_object(), field) {
        (false, _) => Some("not_a_json_object"),
        (true, None) => Some("missing_main_field"),
        (true, Some(false)) => Some("wrong_main_field_type"),
        (true, Some(true)) => None,
    }
}

/// Reads a profiles or changelog file. In strict mode a file that exists
/// but is blank or has the wrong structure is an error: treating it as
/// empty would overwrite everything on the next save.
async fn read_data_file(cfg: &Config, kind: &str, path: &Path) -> Result<Option<Value>, Fail> {
    let data = read_json_file(path).await?;
    if !cfg.strict_data_files {
        return Ok(data);
    }
    match &data {
        None if fs::metadata(path).await.is_ok() => Err(damaged(path, "blank_file").await),
        Some(value) => match shape_problem(kind, value) {
            Some(problem) => Err(damaged(path, problem).await),
            None => Ok(data),
        },
        None => Ok(None),
    }
}

/// Writes pretty JSON through a temp file + rename, keeping a `.bak` copy of
/// the previous version.
async fn write_json_atomic(path: &Path, payload: &Value) -> Result<(), Fail> {
    let started = Instant::now();
    let written = write_json_atomic_inner(path, payload).await;
    diagnostics::io(
        "write",
        path,
        started.elapsed(),
        *written.as_ref().unwrap_or(&0),
        written.is_ok(),
    );
    written.map(|_| ())
}

async fn write_json_atomic_inner(path: &Path, payload: &Value) -> Result<usize, Fail> {
    let body = serde_json::to_string_pretty(payload)?;
    let size = body.len();
    let with_suffix = |suffix: String| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        std::path::PathBuf::from(name)
    };

    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).await?;
    }
    if path_exists(path).await {
        let _ = fs::copy(path, with_suffix(".bak".to_owned())).await;
    }

    let temp = with_suffix(format!(".{}.{}.tmp", std::process::id(), now_ms()));
    fs::write(&temp, body).await?;
    fs::rename(&temp, path).await?;
    Ok(size)
}

/// Writes compact JSON readable only by the service user (mode 0600), for
/// files holding secrets such as session tokens.
pub async fn write_private_json(path: &Path, payload: &Value) -> Result<(), Fail> {
    let started = Instant::now();
    let written = write_private_json_inner(path, payload).await;
    diagnostics::io(
        "write",
        path,
        started.elapsed(),
        *written.as_ref().unwrap_or(&0),
        written.is_ok(),
    );
    written.map(|_| ())
}

async fn write_private_json_inner(path: &Path, payload: &Value) -> Result<usize, Fail> {
    use tokio::io::AsyncWriteExt;

    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).await?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .await?;
    let body = payload.to_string();
    file.write_all(body.as_bytes()).await?;
    file.flush().await?;
    drop(file);
    fs::rename(&temp, path).await?;
    Ok(body.len())
}

// ---------------------------------------------------------------- profiles

pub type Users = Map<String, Value>;

pub async fn read_profile_db(cfg: &Config) -> Result<Users, Fail> {
    Ok(
        match read_data_file(cfg, "profiles", &cfg.profile_db_file).await? {
            Some(Value::Object(mut data)) => match data.remove("users") {
                Some(Value::Object(users)) => users,
                _ => Users::new(),
            },
            _ => Users::new(),
        },
    )
}

pub async fn write_profile_db(cfg: &Config, users: Users) -> Result<(), Fail> {
    write_json_atomic(
        &cfg.profile_db_file,
        &json!({ "version": 1, "users": users }),
    )
    .await
}

fn entry_slug(entry: &Value) -> String {
    entry
        .get("slug")
        .and_then(Value::as_str)
        .map(sanitize_slug)
        .unwrap_or_default()
}

pub fn find_profile_by_slug<'a>(users: &'a Users, slug: &str) -> Option<&'a Value> {
    users
        .values()
        .find(|entry| entry.get("slug").and_then(Value::as_str) == Some(slug))
}

/// Whether `slug` belongs to a registered profile.
pub async fn profile_exists(cfg: &Config, slug: &str) -> bool {
    match read_profile_db(cfg).await {
        Ok(users) => find_profile_by_slug(&users, slug).is_some(),
        Err(error) => {
            eprintln!("[profiles] cannot read profile database: {error}");
            false
        }
    }
}

fn create_unique_slug(users: &Users, base_value: &str) -> String {
    let used: HashSet<String> = users
        .values()
        .map(entry_slug)
        .filter(|slug| !slug.is_empty())
        .collect();
    let base = safe_slug(base_value);
    let mut slug = base.clone();
    let mut suffix = 1;

    while used.contains(&slug) {
        suffix += 1;
        slug = format!("{base}-{suffix}");
    }
    slug
}

fn profile_ref(slug: &str, public_mode: bool) -> Value {
    json!({ "slug": slug, "url": format!("/profile/{slug}/"), "public_mode": public_mode })
}

/// Creates or refreshes the profile entry of a Discord user on login.
/// The caller must hold the profile lock.
pub async fn upsert_profile(cfg: &Config, user: &Value) -> Result<Value, Fail> {
    let user_id = js_str(user.get("id"));
    if user_id.is_empty() {
        return Err(Fail("missing_user_id".to_owned()));
    }

    let mut users = read_profile_db(cfg).await?;
    let current = users.get(&user_id).cloned().unwrap_or(Value::Null);
    let field = |key: &str| {
        current
            .get(key)
            .filter(|value| crate::util::truthy(value))
            .cloned()
    };

    let mut slug = entry_slug(&current);
    if slug.is_empty() {
        let preferred = [user.get("username"), user.get("global_name")]
            .into_iter()
            .map(js_str)
            .find(|name| !name.is_empty())
            .unwrap_or_else(|| "user".to_owned());
        slug = create_unique_slug(&users, &preferred);
    }

    let public_mode = current
        .get("public_mode")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let public_profile = current
        .get("public_profile")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(Value::Null);

    users.insert(
        user_id,
        json!({
            "slug": slug,
            "slug_source": field("slug_source").unwrap_or_else(|| json!("username")),
            "created_at": field("created_at").unwrap_or_else(|| json!(now_iso())),
            "last_login_at": now_iso(),
            "username": js_str(user.get("username")),
            "global_name": js_str(user.get("global_name")),
            "public_mode": public_mode,
            "public_profile": public_profile,
        }),
    );
    write_profile_db(cfg, users).await?;

    Ok(profile_ref(&slug, public_mode))
}

/// Applies `update` to the stored entry of `user_id` and saves the database.
/// Returns `None` when the user has no profile. The caller must hold the
/// profile lock.
pub async fn update_profile_entry(
    cfg: &Config,
    user_id: &str,
    update: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<Option<Value>, Fail> {
    let mut users = read_profile_db(cfg).await?;
    let Some(Value::Object(entry)) = users.get_mut(user_id) else {
        return Ok(None);
    };
    let slug = match entry.get("slug") {
        Some(Value::String(slug)) if !slug.is_empty() => slug.clone(),
        _ => return Ok(None),
    };

    let public_mode = update(entry);
    write_profile_db(cfg, users).await?;
    Ok(Some(profile_ref(&slug, public_mode)))
}

/// Deletes the profile of `user_id`; returns its slug when one existed.
/// The caller must hold the profile lock.
pub async fn delete_profile(cfg: &Config, user_id: &str) -> Result<Option<String>, Fail> {
    let mut users = read_profile_db(cfg).await?;
    let slug = match users.get(user_id).and_then(|entry| entry.get("slug")) {
        Some(Value::String(slug)) if !slug.is_empty() => slug.clone(),
        _ => return Ok(None),
    };

    users.remove(user_id);
    write_profile_db(cfg, users).await?;
    Ok(Some(slug))
}

// --------------------------------------------------------------- changelog

fn default_changelog_post() -> Value {
    json!({
        "id": "website-1-0",
        "title": "Website 1.0: Init Project",
        "content": "First version of website Bladephoriafonk",
        "media_url": "",
        "media_type": "",
        "created_at": "2026-01-01T00:00:00.000Z",
        "created_by": { "id": "system", "username": "system", "global_name": "System" },
    })
}

pub fn normalize_changelog_post(input: &Value) -> Option<Value> {
    let input = input.as_object()?;
    let media_url = sanitize_http_url(input.get("media_url"));
    let media_type = infer_media_type(&media_url, input.get("media_type"));
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    let created_by = input.get("created_by").filter(|value| value.is_object());
    let author = |key: &str| js_str(created_by.and_then(|by| by.get(key)));

    let id = Some(js_str(input.get("id")))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| random_hex(10));

    let mut post = json!({
        "id": id,
        "title": text("title").unwrap_or_default().trim(),
        "content": text("content").unwrap_or_default(),
        "media_url": media_url,
        "media_type": media_type,
        "created_at": text("created_at").map(str::to_owned).unwrap_or_else(now_iso),
        "created_by": {
            "id": author("id"),
            "username": author("username"),
            "global_name": author("global_name"),
        },
    });

    // Optional Polish version of the post, shown when the site is in Polish.
    for key in ["title_pl", "content_pl"] {
        if let Some(translated) = text(key).filter(|value| !value.trim().is_empty()) {
            post[key] = json!(translated);
        }
    }
    Some(post)
}

pub async fn read_changelog_posts(cfg: &Config) -> Result<Vec<Value>, Fail> {
    let posts: Vec<Value> = match read_data_file(cfg, "changelog", &cfg.changelog_db_file).await? {
        Some(data) => data
            .get("posts")
            .and_then(Value::as_array)
            .map(|posts| posts.iter().filter_map(normalize_changelog_post).collect())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(if posts.is_empty() {
        vec![default_changelog_post()]
    } else {
        posts
    })
}

pub async fn write_changelog_posts(cfg: &Config, posts: Vec<Value>) -> Result<(), Fail> {
    write_json_atomic(
        &cfg.changelog_db_file,
        &json!({ "version": 1, "posts": posts }),
    )
    .await
}

/// Consistency checks of the data files for the diagnostics report.
/// Returns the number of profiles and posts and every issue found as
/// (anomaly type, detail).
pub async fn audit_data(cfg: &Config) -> (usize, usize, Vec<(&'static str, String)>) {
    let mut issues = Vec::new();
    let mut profiles = 0;
    let mut posts = 0;

    match read_profile_db(cfg).await {
        Ok(users) => {
            profiles = users.len();
            let mut seen = HashSet::new();
            for (user_id, entry) in &users {
                let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
                    issues.push(("profile_without_slug", format!("user {user_id}")));
                    continue;
                };
                if sanitize_slug(slug) != slug {
                    issues.push(("invalid_slug", slug.to_owned()));
                }
                if !seen.insert(slug.to_owned()) {
                    issues.push(("duplicate_slug", slug.to_owned()));
                }
            }
        }
        Err(error) => issues.push(("unreadable_data_file", error.to_string())),
    }

    match read_data_file(cfg, "changelog", &cfg.changelog_db_file).await {
        Ok(Some(data)) => {
            let list = data
                .get("posts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            posts = list.len();
            let horizon = now_ms() + 5 * 60 * 1000;
            let mut ids = HashSet::new();
            for post in &list {
                let id = js_str(post.get("id"));
                if !id.is_empty() && !ids.insert(id.clone()) {
                    issues.push(("duplicate_post_id", id.clone()));
                }
                match post
                    .get("created_at")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_ms)
                {
                    None => issues.push(("bad_post_date", id.clone())),
                    Some(created) if created > horizon => {
                        issues.push(("post_in_future", id.clone()));
                    }
                    Some(_) => {}
                }
                if post
                    .get("content")
                    .and_then(Value::as_str)
                    .is_none_or(|content| content.trim().is_empty())
                {
                    issues.push(("empty_post", id));
                }
            }
        }
        Ok(None) => {}
        Err(error) => issues.push(("unreadable_data_file", error.to_string())),
    }

    (profiles, posts, issues)
}

/// Creates the data files on first start so they are easy to find and edit.
pub async fn ensure_default_files(cfg: &Config) -> Result<(), Fail> {
    if !path_exists(&cfg.profile_db_file).await {
        write_profile_db(cfg, Users::new()).await?;
    }
    if !path_exists(&cfg.changelog_db_file).await {
        write_changelog_posts(cfg, vec![default_changelog_post()]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strict_config(dir: &Path) -> Config {
        let mut cfg = Config::load(None).unwrap();
        cfg.profile_db_file = dir.join("profiles.json");
        cfg.changelog_db_file = dir.join("changelog.json");
        cfg.strict_data_files = true;
        cfg
    }

    #[tokio::test]
    async fn damaged_files_are_kept_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("bladephoria-damaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = strict_config(&dir);

        // Truncated to an empty object: must not look like "no profiles".
        std::fs::write(&cfg.profile_db_file, "{}").unwrap();
        assert!(read_profile_db(&cfg).await.is_err());
        let user = json!({ "id": "1", "username": "someone" });
        assert!(upsert_profile(&cfg, &user).await.is_err());
        assert_eq!(std::fs::read_to_string(&cfg.profile_db_file).unwrap(), "{}");

        // Broken JSON: one copy per content, however often it is read.
        std::fs::write(&cfg.changelog_db_file, "{\"posts\": [").unwrap();
        for _ in 0..3 {
            assert!(read_changelog_posts(&cfg).await.is_err());
        }
        let copies = |prefix: &str| {
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.starts_with(prefix) && name.contains(".corrupt-")
                })
                .count()
        };
        assert_eq!(copies("changelog.json"), 1);
        assert_eq!(copies("profiles.json"), 1);

        // Blank file in strict mode, accepted when strict mode is off.
        std::fs::write(&cfg.profile_db_file, "").unwrap();
        assert!(read_profile_db(&cfg).await.is_err());
        let mut lenient = strict_config(&dir);
        lenient.strict_data_files = false;
        assert!(read_profile_db(&lenient).await.unwrap().is_empty());

        // A good file reads normally.
        std::fs::write(&cfg.profile_db_file, "{\"version\":1,\"users\":{}}").unwrap();
        assert!(read_profile_db(&cfg).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changelog_post_normalization() {
        let post = normalize_changelog_post(&json!({
            "id": 7,
            "title": "  Hello  ",
            "content": "Body",
            "media_url": "https://cdn.example/clip.mp4",
            "created_at": "2026-06-04T13:26:07.036Z",
            "created_by": { "id": "1", "username": "yhym" },
            "extra": true,
        }))
        .unwrap();
        assert_eq!(
            post,
            json!({
                "id": "7",
                "title": "Hello",
                "content": "Body",
                "media_url": "https://cdn.example/clip.mp4",
                "media_type": "video",
                "created_at": "2026-06-04T13:26:07.036Z",
                "created_by": { "id": "1", "username": "yhym", "global_name": "" },
            })
        );
        let translated = normalize_changelog_post(&json!({
            "id": "a",
            "content": "Body",
            "title_pl": "Tytuł",
            "content_pl": "Treść",
            "created_at": "2026-06-04T13:26:07.036Z",
        }))
        .unwrap();
        assert_eq!(translated["title_pl"], "Tytuł");
        assert_eq!(translated["content_pl"], "Treść");
        assert!(post.get("title_pl").is_none());
        assert!(post.get("content_pl").is_none());

        assert!(normalize_changelog_post(&json!("text")).is_none());
        assert!(normalize_changelog_post(&json!(null)).is_none());
    }

    #[test]
    fn profile_lookup_is_exact() {
        let users: Users = json!({
            "1": { "slug": "yhym" },
            "2": { "slug": "other" },
            "3": "broken",
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(find_profile_by_slug(&users, "yhym").is_some());
        assert!(find_profile_by_slug(&users, "YHYM").is_none());
        assert!(find_profile_by_slug(&users, "missing").is_none());
    }
}
