//! `bladephoria-web --check-config`: validates the configuration and the
//! data files without starting the server and without writing anything.
//! Secret values are never printed, only whether they are set.

use std::path::Path;

use serde_json::Value;

use crate::config::Config;
use crate::store;

#[derive(Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

pub struct Finding {
    pub level: Level,
    pub text: String,
}

fn finding(level: Level, text: impl Into<String>) -> Finding {
    Finding {
        level,
        text: text.into(),
    }
}

fn check_set(findings: &mut Vec<Finding>, name: &str, value: &str) {
    findings.push(if value.trim().is_empty() {
        finding(Level::Error, format!("{name} is not set"))
    } else {
        finding(Level::Ok, format!("{name} is set"))
    });
}

fn check_file(findings: &mut Vec<Finding>, what: &str, path: &Path, required: bool) {
    let level = if required { Level::Error } else { Level::Warn };
    findings.push(if path.is_file() {
        finding(Level::Ok, format!("{what}: {}", path.display()))
    } else {
        finding(level, format!("{what} missing: {}", path.display()))
    });
}

/// The file must be valid JSON when it exists (a missing one is created at
/// start) and its directory must be writable.
fn check_data_file(findings: &mut Vec<Finding>, what: &str, path: &Path) {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => {
            findings.push(finding(
                Level::Ok,
                format!("{what}: empty, {}", path.display()),
            ));
        }
        Ok(text) => findings.push(match serde_json::from_str::<Value>(&text) {
            Ok(_) => finding(Level::Ok, format!("{what}: valid JSON, {}", path.display())),
            Err(error) => finding(
                Level::Error,
                format!("{what} is not valid JSON ({error}): {}", path.display()),
            ),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => findings.push(finding(
            Level::Warn,
            format!(
                "{what} does not exist yet, it is created at start: {}",
                path.display()
            ),
        )),
        Err(error) => findings.push(finding(
            Level::Error,
            format!("{what} cannot be read ({error}): {}", path.display()),
        )),
    }
    check_writable_dir(findings, what, path.parent());
}

fn check_writable_dir(findings: &mut Vec<Finding>, what: &str, dir: Option<&Path>) {
    let Some(dir) = dir.filter(|dir| !dir.as_os_str().is_empty()) else {
        return;
    };
    // A missing directory is created at start; then its nearest existing
    // parent has to be writable.
    let existing = dir.ancestors().find(|ancestor| ancestor.exists());
    let writable = |path: &Path| {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_dir() && !meta.permissions().readonly())
    };
    match existing {
        Some(found) if found == dir && writable(found) => {}
        Some(found) if found != dir && writable(found) => findings.push(finding(
            Level::Warn,
            format!("{what}: directory {} is created at start", dir.display()),
        )),
        _ => findings.push(finding(
            Level::Error,
            format!("{what}: directory is not writable: {}", dir.display()),
        )),
    }
}

pub async fn check_config(cfg: &Config) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.push(finding(
        Level::Ok,
        format!("listen on {}:{}", cfg.host, cfg.port),
    ));

    check_set(&mut findings, "DISCORD_CLIENT_ID", &cfg.discord_client_id);
    check_set(
        &mut findings,
        "DISCORD_CLIENT_SECRET",
        &cfg.discord_client_secret,
    );
    check_set(
        &mut findings,
        "DISCORD_REDIRECT_URI",
        &cfg.discord_redirect_uri,
    );
    if !cfg.discord_redirect_uri.is_empty() {
        match url::Url::parse(&cfg.discord_redirect_uri) {
            Ok(redirect) if cfg.cookie_secure && redirect.scheme() != "https" => {
                findings.push(finding(
                    Level::Warn,
                    "DISCORD_REDIRECT_URI is not https while COOKIE_SECURE is on",
                ))
            }
            Ok(redirect) if !redirect.path().ends_with("/auth/discord/callback") => {
                findings.push(finding(
                    Level::Warn,
                    "DISCORD_REDIRECT_URI does not end with /auth/discord/callback",
                ))
            }
            Ok(_) => {}
            Err(error) => findings.push(finding(
                Level::Error,
                format!("DISCORD_REDIRECT_URI is not a URL: {error}"),
            )),
        }
    }

    check_file(
        &mut findings,
        "home page",
        &cfg.site_root.join("index.html"),
        true,
    );
    check_file(
        &mut findings,
        "profile template",
        &cfg.profile_template_file,
        true,
    );
    findings.push(if cfg.media_root.is_dir() {
        finding(Level::Ok, format!("media: {}", cfg.media_root.display()))
    } else {
        finding(
            Level::Warn,
            format!("media directory missing: {}", cfg.media_root.display()),
        )
    });

    check_data_file(&mut findings, "profiles", &cfg.profile_db_file);
    check_data_file(&mut findings, "changelog", &cfg.changelog_db_file);
    check_data_file(&mut findings, "sessions", &cfg.session_db_file);
    if findings.iter().all(|found| found.level != Level::Error) {
        let (profiles, posts, issues) = store::audit_data(cfg).await;
        findings.push(finding(
            Level::Ok,
            format!("data: {profiles} profiles, {posts} changelog posts"),
        ));
        for (kind, detail) in issues {
            findings.push(finding(Level::Warn, format!("data issue {kind}: {detail}")));
        }
    }

    if cfg.diagnostics.enabled {
        let dir = &cfg.diagnostics.dir;
        if dir.is_dir() {
            check_writable_dir(&mut findings, "diagnostics", Some(dir));
        } else {
            check_writable_dir(
                &mut findings,
                "diagnostics (created at start)",
                dir.parent(),
            );
        }
        findings.push(finding(
            Level::Ok,
            format!("diagnostics on: {}", dir.display()),
        ));
    } else {
        findings.push(finding(Level::Ok, "diagnostics off"));
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_files_are_validated_without_writing() {
        let dir = std::env::temp_dir().join(format!("bladephoria-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.json");
        let bad = dir.join("bad.json");
        std::fs::write(&good, "{\"users\":{}}").unwrap();
        std::fs::write(&bad, "{\"users\":").unwrap();

        let mut findings = Vec::new();
        check_data_file(&mut findings, "good", &good);
        check_data_file(&mut findings, "bad", &bad);
        check_data_file(&mut findings, "missing", &dir.join("missing.json"));
        check_data_file(&mut findings, "nowhere", &dir.join("no-dir/file.json"));
        let levels: Vec<&Level> = findings.iter().map(|found| &found.level).collect();
        assert_eq!(
            levels,
            [
                &Level::Ok,
                &Level::Error,
                &Level::Warn,
                &Level::Warn,
                &Level::Warn
            ]
        );
        // Nothing was created.
        assert!(!dir.join("missing.json").exists());
        assert!(!dir.join("no-dir").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secrets_are_reported_as_set_only() {
        let mut findings = Vec::new();
        check_set(&mut findings, "DISCORD_CLIENT_SECRET", "very-secret-value");
        check_set(&mut findings, "DISCORD_CLIENT_ID", " ");
        assert_eq!(findings[0].text, "DISCORD_CLIENT_SECRET is set");
        assert_eq!(findings[1].level, Level::Error);
        assert!(
            findings
                .iter()
                .all(|found| !found.text.contains("very-secret"))
        );
    }
}
