//! Discord API client and the mapping from Discord objects to the public
//! JSON shapes the frontend consumes.

use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::error::Fail;
use crate::util::{js_str, now_iso, truthy};

pub struct ServerConfig {
    pub key: &'static str,
    pub name: &'static str,
    /// Invite codes in order of preference. The first one that Discord still
    /// accepts is used for the data and shown as the main link; the one after
    /// it is shown as the alternative link.
    pub invite_codes: &'static [&'static str],
}

impl ServerConfig {
    pub fn primary_invite(&self) -> &'static str {
        self.invite_codes.first().copied().unwrap_or_default()
    }
}

pub const PUBLIC_SERVERS: [ServerConfig; 2] = [
    ServerConfig {
        key: "blade",
        name: "Bladephoriafonk",
        invite_codes: &[
            "Nh7XnSMhCp",
            "2xTr8pmk36",
            "7vxYEgKdgN",
            "umZ3rMnyXp",
            "z3NZvDHGzP",
            "TCx4K5JnSz",
        ],
    },
    ServerConfig {
        key: "pmc",
        name: "PMC Ashley Group",
        invite_codes: &[
            "2upsTHV5Bv",
            "8MAjxgmnAF",
            "BAVcR9Tm9W",
            "D7ZggjWSxc",
            "KtfDX8Js5H",
            "VVpgGXHFG9",
            "VmeuEg3SKZ",
            "XFg2Yt52Ew",
            "YZn7XmwY5P",
            "Z2ZVECnrgu",
            "fzcAQgJBtv",
            "pecFGxSjsb",
            "tfT8a8W8nM",
            "855QbtyBHz",
            "8xRHrrTpWm",
            "FqA3kQYWuS",
            "GHScwuPTQq",
            "HuPtndJmXF",
            "RCNgPAQr8U",
            "RcVSwaZ3GY",
            "UDNWqanx5G",
            "VUWjJWgcmx",
            "cVWGmCpqZ4",
        ],
    },
];

fn invite_link(invite_code: &str) -> String {
    format!("https://discord.gg/{invite_code}")
}

pub struct Discord {
    http: reqwest::Client,
    origin: String,
}

fn first_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

impl Discord {
    pub fn new(cfg: &Config) -> Result<Self, Fail> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            origin: cfg.discord_origin.clone(),
        })
    }

    pub fn authorize_url(&self) -> String {
        format!("{}/oauth2/authorize", self.origin)
    }

    fn user_url(&self) -> String {
        format!("{}/api/v10/users/@me", self.origin)
    }

    pub fn invite_url(&self, invite_code: &str) -> String {
        format!(
            "{}/api/v9/invites/{invite_code}?with_counts=true&with_expiration=true",
            self.origin
        )
    }

    async fn token_request(&self, label: &str, form: &[(&str, &str)]) -> Result<Value, Fail> {
        let response = self
            .http
            .post(format!("{}/api/oauth2/token", self.origin))
            .form(form)
            .send()
            .await?;
        let status = response.status();
        let data: Value = response.json().await?;
        if !status.is_success() {
            let reason = Some(js_str(data.get("error")))
                .filter(|reason| !reason.is_empty())
                .unwrap_or_else(|| "unknown".to_owned());
            return Err(Fail(format!("{label}:{}:{reason}", status.as_u16())));
        }
        Ok(data)
    }

    pub async fn exchange_code(&self, cfg: &Config, code: &str) -> Result<Value, Fail> {
        self.token_request(
            "token_exchange_failed",
            &[
                ("client_id", &cfg.discord_client_id),
                ("client_secret", &cfg.discord_client_secret),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &cfg.discord_redirect_uri),
            ],
        )
        .await
    }

    pub async fn refresh_token(&self, cfg: &Config, refresh_token: &str) -> Result<Value, Fail> {
        self.token_request(
            "token_refresh_failed",
            &[
                ("client_id", &cfg.discord_client_id),
                ("client_secret", &cfg.discord_client_secret),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ],
        )
        .await
    }

    async fn resource(&self, url: &str, access_token: &str) -> Result<Value, Fail> {
        let response = self
            .http
            .get(url)
            .bearer_auth(access_token)
            .header("user-agent", "BladephoriafonkOAuth/2.0")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Fail(format!(
                "discord_fetch_failed:{}:{}",
                status.as_u16(),
                first_chars(&text, 200)
            )));
        }
        Ok(response.json().await?)
    }

    pub async fn user(&self, access_token: &str) -> Result<Value, Fail> {
        self.resource(&self.user_url(), access_token).await
    }

    pub async fn connections(&self, access_token: &str) -> Result<Value, Fail> {
        self.resource(&format!("{}/connections", self.user_url()), access_token)
            .await
    }

    /// Server-profile details (banner, bio, nick) of the user in their
    /// preferred guild. Any failure simply yields `None`.
    pub async fn guild_profile(
        &self,
        cfg: &Config,
        access_token: &str,
        user: &Value,
    ) -> Option<Value> {
        let user_id = js_str(user.get("id"));
        if user_id.is_empty() {
            return None;
        }

        let guilds = self
            .resource(&format!("{}/guilds", self.user_url()), access_token)
            .await
            .ok()?;
        let guild = pick_preferred_guild(cfg, user, guilds.as_array()?)?;
        let guild_id = js_str(guild.get("id"));
        if guild_id.is_empty() {
            return None;
        }

        let member = self
            .resource(
                &format!("{}/guilds/{guild_id}/member", self.user_url()),
                access_token,
            )
            .await
            .ok()?;
        let text = |key: &str| member.get(key).and_then(Value::as_str).unwrap_or_default();

        Some(json!({
            "guild_id": guild_id,
            "guild_name": js_str(guild.get("name")),
            "guild_banner_url": guild_member_banner_url(&user_id, &guild_id, member.get("banner")),
            "guild_bio": text("bio"),
            "guild_nick": text("nick"),
        }))
    }

    async fn request_invite_snapshot(&self, invite_code: &str) -> Result<Map<String, Value>, Fail> {
        let endpoint = self.invite_url(invite_code);
        let response = self
            .http
            .get(&endpoint)
            .header("user-agent", "BladephoriafonkWebsite/2.0")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Fail(format!(
                "invite_fetch_failed:{}:{}",
                status.as_u16(),
                first_chars(&text, 200)
            )));
        }

        let data: Value = response.json().await?;
        let guild = data.get("guild").filter(|value| truthy(value));
        let profile = data.get("profile").filter(|value| truthy(value));
        let from_guild = |key: &str| guild.and_then(|guild| guild.get(key));
        let from_profile = |key: &str| profile.and_then(|profile| profile.get(key));
        let first_truthy = |candidates: [Option<&Value>; 2]| {
            candidates
                .into_iter()
                .map(js_str)
                .find(|value| !value.is_empty())
                .unwrap_or_default()
        };
        let first_present = |candidates: [Option<&Value>; 2]| {
            candidates
                .into_iter()
                .flatten()
                .find(|value| !value.is_null())
                .cloned()
                .unwrap_or_else(|| json!(0))
        };
        let number = |key: &str| from_guild(key).filter(|value| value.is_number()).cloned();

        let guild_id = first_truthy([from_guild("id"), data.get("guild_id")]);
        let icon_hash = first_truthy([from_guild("icon"), from_profile("icon_hash")]);
        let icon_url = if guild_id.is_empty() || icon_hash.is_empty() {
            String::new()
        } else {
            let ext = if icon_hash.starts_with("a_") {
                "gif"
            } else {
                "png"
            };
            format!("https://cdn.discordapp.com/icons/{guild_id}/{icon_hash}.{ext}?size=128")
        };

        let snapshot = json!({
            "guild_id": guild_id,
            "description": first_truthy([from_guild("description"), from_profile("description")]),
            "member_count": first_present([data.get("approximate_member_count"), from_profile("member_count")]),
            "online_count": first_present([data.get("approximate_presence_count"), from_profile("online_count")]),
            "verification_level": number("verification_level"),
            "premium_tier": number("premium_tier").unwrap_or_else(|| json!(0)),
            "premium_subscription_count": number("premium_subscription_count").unwrap_or_else(|| json!(0)),
            "icon_url": icon_url,
            "source_url": endpoint,
            "updated_at": now_iso(),
            "invite_code_used": invite_code,
            "stale": false,
        });
        match snapshot {
            Value::Object(map) => Ok(map),
            _ => unreachable!("json! object literal"),
        }
    }

    /// Public invite data of a server. A dead invite (404) moves on to the
    /// next code; any other failure (network, rate limit) stops, so an outage
    /// never fans out into one request per code.
    pub async fn invite_snapshot(&self, server: &ServerConfig) -> Result<Map<String, Value>, Fail> {
        let mut last_error = Fail("invite_fetch_failed:unknown".to_owned());
        for (index, invite_code) in server.invite_codes.iter().enumerate() {
            match self.request_invite_snapshot(invite_code).await {
                Ok(snapshot) => {
                    let mut payload = server_identity(server, index);
                    payload.extend(snapshot);
                    return Ok(payload);
                }
                Err(error) => {
                    let invite_is_dead = error.0.starts_with("invite_fetch_failed:404:");
                    last_error = error;
                    if !invite_is_dead {
                        break;
                    }
                }
            }
        }
        Err(last_error)
    }
}

/// Name and links of a server, with `working` being the index of the invite
/// code to present as the main link.
pub fn server_identity(server: &ServerConfig, working: usize) -> Map<String, Value> {
    let link = |index: usize| {
        server
            .invite_codes
            .get(index)
            .map(|code| invite_link(code))
            .unwrap_or_default()
    };
    let identity = json!({
        "key": server.key,
        "name": server.name,
        "invite_code": server.primary_invite(),
        "main_url": link(working),
        "alt_url": link(working + 1),
    });
    match identity {
        Value::Object(map) => map,
        _ => unreachable!("json! object literal"),
    }
}

fn pick_preferred_guild<'a>(cfg: &Config, user: &Value, guilds: &'a [Value]) -> Option<&'a Value> {
    let by_id = |id: &str| guilds.iter().find(|guild| js_str(guild.get("id")) == id);

    if !cfg.primary_guild_id.is_empty()
        && let Some(guild) = by_id(&cfg.primary_guild_id)
    {
        return Some(guild);
    }

    let identity_guild_id = js_str(user.pointer("/primary_guild/identity_guild_id"));
    if !identity_guild_id.is_empty()
        && let Some(guild) = by_id(&identity_guild_id)
    {
        return Some(guild);
    }

    if !cfg.primary_guild_name_hint.is_empty()
        && let Some(guild) = guilds.iter().find(|guild| {
            js_str(guild.get("name"))
                .to_lowercase()
                .contains(&cfg.primary_guild_name_hint)
        })
    {
        return Some(guild);
    }

    guilds.first()
}

fn cdn_image_url(prefix: &str, hash: Option<&Value>, size: u32) -> String {
    let hash = js_str(hash);
    if hash.is_empty() {
        return String::new();
    }
    let ext = if hash.starts_with("a_") { "gif" } else { "png" };
    format!("https://cdn.discordapp.com/{prefix}/{hash}.{ext}?size={size}")
}

fn avatar_url(user: &Value) -> String {
    let id = js_str(user.get("id"));
    if id.is_empty() {
        return String::new();
    }
    cdn_image_url(&format!("avatars/{id}"), user.get("avatar"), 256)
}

fn banner_url(user: &Value) -> String {
    let id = js_str(user.get("id"));
    if id.is_empty() {
        return String::new();
    }
    cdn_image_url(&format!("banners/{id}"), user.get("banner"), 1024)
}

fn guild_member_banner_url(user_id: &str, guild_id: &str, banner: Option<&Value>) -> String {
    cdn_image_url(
        &format!("guilds/{guild_id}/users/{user_id}/banners"),
        banner,
        1024,
    )
}

/// The subset of a Discord user that is safe to send to browsers.
pub fn to_public_user(user: &Value, guild_profile: Option<&Value>) -> Value {
    let guild = |key: &str| js_str(guild_profile.and_then(|profile| profile.get(key)));
    let number = |key: &str| user.get(key).filter(|value| value.is_number()).cloned();

    json!({
        "username": js_str(user.get("username")),
        "global_name": js_str(user.get("global_name")),
        "banner_color": user.get("banner_color").filter(|value| truthy(value)),
        "accent_color": number("accent_color"),
        "public_flags": number("public_flags").unwrap_or_else(|| json!(0)),
        "avatar_url": avatar_url(user),
        "banner_url": banner_url(user),
        "guild_banner_url": guild("guild_banner_url"),
        "guild_name": guild("guild_name"),
        "guild_bio": guild("guild_bio"),
        "guild_nick": guild("guild_nick"),
    })
}

pub fn to_public_connections(connections: &Value) -> Vec<Value> {
    let Some(items) = connections.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .map(|item| {
            let flag = |key: &str| item.get(key).is_some_and(truthy);
            let kind = Some(js_str(item.get("type")))
                .filter(|kind| !kind.is_empty())
                .unwrap_or_else(|| "unknown".to_owned());
            json!({
                "name": js_str(item.get("name")),
                "type": kind,
                "verified": flag("verified"),
                "visibility": item.get("visibility").filter(|value| value.is_number()).cloned().unwrap_or_else(|| json!(0)),
                "show_activity": flag("show_activity"),
                "friend_sync": flag("friend_sync"),
                "two_way_link": flag("two_way_link"),
                "revoked": flag("revoked"),
            })
        })
        .collect()
}

/// What is stored and shown for a profile switched to public mode: only the
/// connections the user made visible on Discord.
pub fn public_profile_snapshot(
    user: &Value,
    connections: &Value,
    guild_profile: Option<&Value>,
) -> Value {
    let visible: Vec<Value> = to_public_connections(connections)
        .into_iter()
        .filter(|item| item["visibility"].as_f64() == Some(1.0) && item["revoked"] == false)
        .collect();

    json!({
        "user": to_public_user(user, guild_profile),
        "connections": visible,
        "updated_at": now_iso(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_user_shape() {
        let user = json!({
            "id": "780552865445969920",
            "username": "yhym",
            "global_name": null,
            "avatar": "a_abc",
            "banner": "def",
            "accent_color": 0,
            "email": "secret@example.com",
        });
        let guild = json!({ "guild_name": "Bladephoriafonk", "guild_bio": "", "guild_nick": "Y" });
        let public = to_public_user(&user, Some(&guild));
        assert_eq!(
            serde_json::to_string(&public).unwrap(),
            r#"{"username":"yhym","global_name":"","banner_color":null,"accent_color":0,"public_flags":0,"avatar_url":"https://cdn.discordapp.com/avatars/780552865445969920/a_abc.gif?size=256","banner_url":"https://cdn.discordapp.com/banners/780552865445969920/def.png?size=1024","guild_banner_url":"","guild_name":"Bladephoriafonk","guild_bio":"","guild_nick":"Y"}"#
        );
    }

    #[test]
    fn snapshot_keeps_only_visible_connections() {
        let connections = json!([
            { "name": "a", "type": "steam", "visibility": 1, "verified": true },
            { "name": "b", "type": "github", "visibility": 0 },
            { "name": "c", "visibility": 1, "revoked": true },
        ]);
        let snapshot = public_profile_snapshot(&json!({}), &connections, None);
        let visible = snapshot["connections"].as_array().unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0]["name"], "a");
        assert_eq!(to_public_connections(&connections)[2]["type"], "unknown");
        assert!(to_public_connections(&json!(null)).is_empty());
    }
}
