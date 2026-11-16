//! Runtime configuration, read from the process environment and an optional
//! `--env-file`. Variable names and defaults match the previous Node backend.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::util::normalize_admin_identity;

pub struct Config {
    pub host: String,
    pub port: u16,
    pub serve_static: bool,

    pub discord_origin: String,
    pub discord_client_id: String,
    pub discord_client_secret: String,
    pub discord_redirect_uri: String,
    pub primary_guild_id: String,
    pub primary_guild_name_hint: String,

    pub cookie_name: String,
    pub cookie_secure: bool,

    pub site_root: PathBuf,
    pub profile_root: PathBuf,
    pub profile_source_root: PathBuf,
    pub profile_template_file: PathBuf,
    pub profile_db_file: PathBuf,
    pub changelog_db_file: PathBuf,
    pub media_root: PathBuf,

    pub session_data_ttl_ms: i64,
    pub invite_cache_ttl_ms: i64,
    pub json_body_limit: usize,
    pub changelog_json_body_limit: usize,

    pub admin_user_ids: HashSet<String>,
    pub admin_usernames: HashSet<String>,
}

/// Environment lookup where real environment variables win over the env file
/// and empty values count as unset (like `process.env.X || default`).
struct Env {
    file: HashMap<String, String>,
}

impl Env {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.file
                    .get(key)
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
    }

    fn string(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or_else(|| default.to_owned())
    }

    fn number<T: std::str::FromStr>(&self, key: &str, default: T) -> T {
        self.get(key)
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(default)
    }

    fn path(&self, key: &str, default: PathBuf) -> PathBuf {
        self.get(key).map(PathBuf::from).unwrap_or(default)
    }
}

/// Parses `KEY=VALUE` lines; `#` comments, blank lines and an optional
/// `export ` prefix are ignored, matching quotes around a value are removed.
fn parse_env_file(text: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let unquoted = ['"', '\'', '`']
            .iter()
            .find_map(|&quote| {
                value
                    .strip_prefix(quote)
                    .and_then(|rest| rest.strip_suffix(quote))
            })
            .unwrap_or(value);
        vars.insert(key.trim().to_owned(), unquoted.to_owned());
    }
    vars
}

/// The site root is the directory holding `index.html`. Without `SITE_ROOT`
/// it is the working directory, or its parent when started from `oauth/`
/// like the old Node service was.
fn detect_site_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !cwd.join("index.html").is_file()
        && let Some(parent) = cwd.parent()
        && parent.join("index.html").is_file()
    {
        return parent.to_path_buf();
    }
    cwd
}

impl Config {
    pub fn load(env_file: Option<&Path>) -> Result<Self, String> {
        let file = match env_file {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|error| format!("cannot read env file {}: {error}", path.display()))?;
                parse_env_file(&text)
            }
            None => HashMap::new(),
        };
        let env = Env { file };

        let site_root = env
            .get("SITE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(detect_site_root);
        let base_url = env.string("BASE_URL", "https://bladephoriafonk.xyz");
        let cookie_secure = env
            .get("COOKIE_SECURE")
            .map(|value| value.eq_ignore_ascii_case("true"))
            .unwrap_or_else(|| base_url.starts_with("https://"));

        let list = |key: &str, default: &str| -> Vec<String> {
            env.string(key, default)
                .split(',')
                .map(|item| item.trim().to_owned())
                .collect()
        };

        Ok(Self {
            host: env.string("HOST", "127.0.0.1"),
            port: env.number("PORT", 3001),
            serve_static: !env
                .string("SERVE_STATIC", "true")
                .eq_ignore_ascii_case("false"),

            discord_origin: env
                .string("DISCORD_ORIGIN", "https://discord.com")
                .trim_end_matches('/')
                .to_owned(),
            discord_client_id: env.string("DISCORD_CLIENT_ID", ""),
            discord_client_secret: env.string("DISCORD_CLIENT_SECRET", ""),
            discord_redirect_uri: env.string("DISCORD_REDIRECT_URI", ""),
            primary_guild_id: env.string("PRIMARY_GUILD_ID", ""),
            primary_guild_name_hint: env
                .string("PRIMARY_GUILD_NAME_HINT", "bladephoria")
                .to_lowercase(),

            cookie_name: env.string("COOKIE_NAME", "bpf_session"),
            cookie_secure,

            profile_root: env.path("PROFILE_ROOT", site_root.join("profile")),
            profile_source_root: env.path("PROFILE_SOURCE_ROOT", site_root.join("src/profile")),
            profile_template_file: env.path(
                "PROFILE_TEMPLATE_FILE",
                site_root.join("oauth/profile-template.html"),
            ),
            profile_db_file: env.path(
                "PROFILE_DB_FILE",
                site_root.join("oauth/data/profiles.json"),
            ),
            changelog_db_file: env.path(
                "CHANGELOG_DB_FILE",
                site_root.join("oauth/data/changelog.json"),
            ),
            media_root: env.path("MEDIA_ROOT", site_root.join("media")),
            site_root,

            session_data_ttl_ms: env.number("SESSION_DATA_TTL_MS", 5 * 60 * 1000),
            invite_cache_ttl_ms: env.number("INVITE_CACHE_TTL_MS", 60 * 1000),
            json_body_limit: env.number("JSON_BODY_LIMIT_BYTES", 8 * 1024 * 1024),
            changelog_json_body_limit: env
                .number("CHANGELOG_JSON_BODY_LIMIT_BYTES", 32 * 1024 * 1024),

            admin_user_ids: list("ADMIN_USER_IDS", "780552865445969920")
                .into_iter()
                .filter(|item| !item.is_empty())
                .collect(),
            admin_usernames: list("ADMIN_USERNAMES", "yhym,!yhym's")
                .iter()
                .map(|item| normalize_admin_identity(item))
                .filter(|item| !item.is_empty())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_env_file;

    #[test]
    fn env_file_parsing() {
        let vars = parse_env_file(
            "# comment\nPORT=3001\n\nexport HOST = 127.0.0.1\nNAME=\"quoted value\"\nADMINS=yhym,!yhym's\nEMPTY=\nnot a pair\n",
        );
        assert_eq!(vars["PORT"], "3001");
        assert_eq!(vars["HOST"], "127.0.0.1");
        assert_eq!(vars["NAME"], "quoted value");
        assert_eq!(vars["ADMINS"], "yhym,!yhym's");
        assert_eq!(vars["EMPTY"], "");
        assert_eq!(vars.len(), 5);
    }
}
