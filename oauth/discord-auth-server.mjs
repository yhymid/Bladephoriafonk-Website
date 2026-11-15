import http from "node:http";
import crypto from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promises as fs } from "node:fs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const PORT = Number(process.env.PORT || 3001);
const HOST = process.env.HOST || "127.0.0.1";

const DISCORD_CLIENT_ID = process.env.DISCORD_CLIENT_ID || "";
const DISCORD_CLIENT_SECRET = process.env.DISCORD_CLIENT_SECRET || "";
const DISCORD_REDIRECT_URI = process.env.DISCORD_REDIRECT_URI || "";
const BASE_URL = process.env.BASE_URL || "https://bladephoriafonk.xyz";
const COOKIE_NAME = process.env.COOKIE_NAME || "bpf_session";
const USE_SECURE_COOKIE = (process.env.COOKIE_SECURE || (BASE_URL.startsWith("https://") ? "true" : "false")).toLowerCase() === "true";

const PROFILE_ROOT = process.env.PROFILE_ROOT || path.resolve(__dirname, "..", "profile");
const PROFILE_SOURCE_ROOT = process.env.PROFILE_SOURCE_ROOT || path.resolve(__dirname, "..", "src", "profile");
const PROFILE_TEMPLATE_FILE = process.env.PROFILE_TEMPLATE_FILE || path.resolve(__dirname, "profile-template.html");
const PROFILE_DB_FILE = process.env.PROFILE_DB_FILE || path.resolve(__dirname, "data", "profiles.json");
const CHANGELOG_DB_FILE = process.env.CHANGELOG_DB_FILE || path.resolve(__dirname, "data", "changelog.json");
const MEDIA_ROOT = process.env.MEDIA_ROOT || path.resolve(__dirname, "..", "media");
const MEDIA_ID_PATTERN = /^[a-f0-9]{32}$/;

const TOKEN_ENDPOINT = "https://discord.com/api/oauth2/token";
const USER_ENDPOINT = "https://discord.com/api/v10/users/@me";
const CONNECTIONS_ENDPOINT = "https://discord.com/api/v10/users/@me/connections";
const USER_GUILDS_ENDPOINT = "https://discord.com/api/v10/users/@me/guilds";
const AUTH_ENDPOINT = "https://discord.com/oauth2/authorize";
const PRIMARY_GUILD_ID = process.env.PRIMARY_GUILD_ID || "";
const PRIMARY_GUILD_NAME_HINT = (process.env.PRIMARY_GUILD_NAME_HINT || "bladephoria").toLowerCase();

const STATE_TTL_MS = 10 * 60 * 1000;
const SESSION_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const SESSION_DATA_TTL_MS = Number(process.env.SESSION_DATA_TTL_MS || 5 * 60 * 1000);
const INVITE_CACHE_TTL_MS = Number(process.env.INVITE_CACHE_TTL_MS || 60 * 1000);
const JSON_BODY_LIMIT_BYTES = Number(process.env.JSON_BODY_LIMIT_BYTES || 8 * 1024 * 1024);
const CHANGELOG_JSON_BODY_LIMIT_BYTES = Number(process.env.CHANGELOG_JSON_BODY_LIMIT_BYTES || 32 * 1024 * 1024);

const ADMIN_USER_IDS = new Set(
  String(process.env.ADMIN_USER_IDS || "780552865445969920")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean)
);

const ADMIN_USERNAMES = new Set(
  String(process.env.ADMIN_USERNAMES || "yhym,!yhym's")
    .split(",")
    .map((item) => normalizeAdminIdentity(item))
    .filter(Boolean)
);

const PUBLIC_SERVER_CONFIGS = [
  {
    key: "blade",
    name: "Bladephoriafonk",
    invite_code: "bladephoriafonk",
    main_url: "https://discord.gg/bladephoriafonk",
    alt_url: "https://discord.gg/6bcWUDa9pE",
    fallback_invite_code: "6bcWUDa9pE"
  },
  {
    key: "pmc",
    name: "PMC Ashley Group",
    invite_code: "2upsTHV5Bv",
    main_url: "https://discord.gg/2upsTHV5Bv",
    alt_url: ""
  }
];

const states = new Map();
const sessions = new Map();
const inviteCache = new Map();

let profileLock = Promise.resolve();
let changelogLock = Promise.resolve();

function now() {
  return Date.now();
}

function randomToken(size = 32) {
  return crypto.randomBytes(size).toString("hex");
}

function normalizeAdminIdentity(value) {
  return String(value || "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/\s+/g, "")
    .trim();
}

function parseCookies(req) {
  const header = req.headers.cookie || "";
  const result = {};

  for (const part of header.split(";")) {
    const trimmed = part.trim();
    if (!trimmed) {
      continue;
    }

    const eq = trimmed.indexOf("=");
    if (eq < 0) {
      continue;
    }

    const key = trimmed.slice(0, eq);
    const value = trimmed.slice(eq + 1);

    try {
      result[key] = decodeURIComponent(value);
    } catch {
      result[key] = value;
    }
  }

  return result;
}

async function readJsonBody(req, maxBytes = JSON_BODY_LIMIT_BYTES) {
  const chunks = [];
  let total = 0;

  for await (const chunk of req) {
    total += chunk.length;
    if (total > maxBytes) {
      throw new Error("payload_too_large");
    }
    chunks.push(chunk);
  }

  if (!chunks.length) {
    return {};
  }

  const text = Buffer.concat(chunks).toString("utf8");
  if (!text.trim()) {
    return {};
  }

  try {
    return JSON.parse(text);
  } catch {
    throw new Error("invalid_json");
  }
}

function setSecurityHeaders(res) {
  res.setHeader("cache-control", "no-store");
  res.setHeader("x-content-type-options", "nosniff");
  res.setHeader("x-frame-options", "DENY");
  res.setHeader("referrer-policy", "strict-origin-when-cross-origin");
  res.setHeader("permissions-policy", "camera=(), microphone=(), geolocation=()");
}

function json(res, statusCode, payload) {
  const body = JSON.stringify(payload);
  setSecurityHeaders(res);
  res.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body)
  });
  res.end(body);
}

function redirect(res, location) {
  setSecurityHeaders(res);
  res.writeHead(302, { location });
  res.end();
}

function setSessionCookie(res, sessionId) {
  const cookieParts = [
    `${COOKIE_NAME}=${encodeURIComponent(sessionId)}`,
    "Path=/",
    `Max-Age=${Math.floor(SESSION_TTL_MS / 1000)}`,
    "HttpOnly",
    "SameSite=Lax"
  ];

  if (USE_SECURE_COOKIE) {
    cookieParts.push("Secure");
  }

  res.setHeader("set-cookie", cookieParts.join("; "));
}

function clearSessionCookie(res) {
  const cookieParts = [
    `${COOKIE_NAME}=`,
    "Path=/",
    "Max-Age=0",
    "HttpOnly",
    "SameSite=Lax"
  ];

  if (USE_SECURE_COOKIE) {
    cookieParts.push("Secure");
  }

  res.setHeader("set-cookie", cookieParts.join("; "));
}

function sanitizeReturnTo(input) {
  const fallback = "/";
  const value = String(input || "");
  if (!value || !value.startsWith("/") || value.startsWith("//") || value.includes("://")) {
    return fallback;
  }
  return value;
}

function safeSlug(input) {
  const normalized = String(input || "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");

  return normalized || "user";
}

function sanitizeSlug(input) {
  const value = String(input || "").toLowerCase();
  if (!/^[a-z0-9-]{1,80}$/.test(value)) {
    return "";
  }
  return value;
}

function withProfileLock(fn) {
  const next = profileLock.then(fn, fn);
  profileLock = next.then(() => undefined, () => undefined);
  return next;
}

function withChangelogLock(fn) {
  const next = changelogLock.then(fn, fn);
  changelogLock = next.then(() => undefined, () => undefined);
  return next;
}

async function pathExists(filePath) {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

async function readJsonFile(filePath, fallbackFactory) {
  try {
    const text = await fs.readFile(filePath, "utf8");
    if (!text.trim()) {
      return fallbackFactory();
    }
    return JSON.parse(text);
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return fallbackFactory();
    }

    if (error instanceof SyntaxError) {
      throw new Error(`invalid_json_file:${path.basename(filePath)}`);
    }

    throw error;
  }
}

async function writeJsonAtomic(filePath, payload) {
  const directory = path.dirname(filePath);
  const body = JSON.stringify(payload, null, 2);
  const tempFile = `${filePath}.${process.pid}.${Date.now()}.tmp`;

  await fs.mkdir(directory, { recursive: true });

  if (await pathExists(filePath)) {
    await fs.copyFile(filePath, `${filePath}.bak`).catch(() => {});
  }

  await fs.writeFile(tempFile, body, "utf8");
  await fs.rename(tempFile, filePath);
}

async function ensureDefaultFile(filePath, payload) {
  if (await pathExists(filePath)) {
    return;
  }
  await writeJsonAtomic(filePath, payload);
}

function defaultProfileDb() {
  return {
    version: 1,
    users: {}
  };
}

async function readProfileDb() {
  const data = await readJsonFile(PROFILE_DB_FILE, defaultProfileDb);
  return {
    version: 1,
    users: typeof data.users === "object" && data.users ? data.users : {}
  };
}

async function writeProfileDb(db) {
  await writeJsonAtomic(PROFILE_DB_FILE, {
    version: 1,
    users: db.users || {}
  });
}

function isAdminUser(userOrId) {
  if (typeof userOrId === "string" || typeof userOrId === "number") {
    return ADMIN_USER_IDS.has(String(userOrId));
  }

  const user = userOrId && typeof userOrId === "object" ? userOrId : null;
  const userId = String(user?.id || "");
  if (userId && ADMIN_USER_IDS.has(userId)) {
    return true;
  }

  const username = normalizeAdminIdentity(user?.username || "");
  const globalName = normalizeAdminIdentity(user?.global_name || "");
  return (username && ADMIN_USERNAMES.has(username)) || (globalName && ADMIN_USERNAMES.has(globalName));
}

function sanitizeHttpUrl(input) {
  if (typeof input !== "string") {
    return "";
  }

  const value = input.trim();
  if (!value) {
    return "";
  }

  try {
    const parsed = new URL(value);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return "";
    }
    return parsed.toString();
  } catch {
    return "";
  }
}

function inferMediaType(urlValue, explicitType = "") {
  const type = String(explicitType || "").toLowerCase();
  if (type === "image" || type === "video") {
    return type;
  }

  const lowered = String(urlValue || "").toLowerCase();
  if (!lowered) {
    return "";
  }

  const imageExt = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".bmp", ".svg"];
  const videoExt = [".mp4", ".webm", ".ogg", ".ogv", ".mov", ".m4v"];

  if (imageExt.some((ext) => lowered.includes(ext))) {
    return "image";
  }

  if (videoExt.some((ext) => lowered.includes(ext))) {
    return "video";
  }

  return "";
}

function defaultChangelogPost() {
  return {
    id: "website-1-0",
    title: "Website 1.0: Init Project",
    content: "First version of website Bladephoriafonk",
    media_url: "",
    media_type: "",
    created_at: "2026-01-01T00:00:00.000Z",
    created_by: {
      id: "system",
      username: "system",
      global_name: "System"
    }
  };
}

function normalizeChangelogPost(input) {
  if (!input || typeof input !== "object") {
    return null;
  }

  const mediaUrl = sanitizeHttpUrl(input.media_url || "");
  const mediaType = inferMediaType(mediaUrl, input.media_type);
  const content = typeof input.content === "string" ? input.content : "";
  const title = typeof input.title === "string" ? input.title.trim() : "";
  const createdBy = input.created_by && typeof input.created_by === "object" ? input.created_by : {};

  return {
    id: String(input.id || randomToken(10)),
    title,
    content,
    media_url: mediaUrl,
    media_type: mediaType,
    created_at: typeof input.created_at === "string" ? input.created_at : new Date().toISOString(),
    created_by: {
      id: String(createdBy.id || ""),
      username: String(createdBy.username || ""),
      global_name: String(createdBy.global_name || "")
    }
  };
}

function defaultChangelogDb() {
  return {
    version: 1,
    posts: [defaultChangelogPost()]
  };
}

async function readChangelogDb() {
  const data = await readJsonFile(CHANGELOG_DB_FILE, defaultChangelogDb);
  const posts = Array.isArray(data.posts)
    ? data.posts.map((item) => normalizeChangelogPost(item)).filter(Boolean)
    : [];

  return {
    version: 1,
    posts: posts.length ? posts : [defaultChangelogPost()]
  };
}

async function writeChangelogDb(db) {
  await writeJsonAtomic(CHANGELOG_DB_FILE, {
    version: 1,
    posts: Array.isArray(db.posts) ? db.posts : [defaultChangelogPost()]
  });
}

function usedSlugsFromDb(db) {
  const used = new Set();
  for (const entry of Object.values(db.users || {})) {
    const slug = sanitizeSlug(entry?.slug);
    if (slug) {
      used.add(slug);
    }
  }
  return used;
}

async function ensureProfilePage(slug) {
  const safeProfileSlug = sanitizeSlug(slug);
  if (!safeProfileSlug) {
    throw new Error("invalid_profile_slug");
  }

  const template = await fs.readFile(PROFILE_TEMPLATE_FILE, "utf8");
  const publicDir = path.join(PROFILE_ROOT, safeProfileSlug);
  const sourceDir = path.join(PROFILE_SOURCE_ROOT, safeProfileSlug);

  await fs.mkdir(publicDir, { recursive: true });
  await fs.mkdir(sourceDir, { recursive: true });
  await fs.writeFile(path.join(publicDir, "index.html"), template, "utf8");
  await fs.writeFile(path.join(sourceDir, "index.html"), template, "utf8");
}

async function removeProfilePage(slug) {
  const safeProfileSlug = sanitizeSlug(slug);
  if (!safeProfileSlug) {
    return;
  }

  await fs.rm(path.join(PROFILE_ROOT, safeProfileSlug), { recursive: true, force: true });
  await fs.rm(path.join(PROFILE_SOURCE_ROOT, safeProfileSlug), { recursive: true, force: true });
}

async function syncAllGeneratedProfiles() {
  const db = await readProfileDb();
  const slugs = [...new Set(Object.values(db.users || {}).map((entry) => sanitizeSlug(entry?.slug)).filter(Boolean))];
  for (const slug of slugs) {
    await ensureProfilePage(slug);
  }
}

async function createUniqueSlug(db, baseValue, currentSlug = "") {
  const used = usedSlugsFromDb(db);
  const normalizedCurrent = sanitizeSlug(currentSlug);
  if (normalizedCurrent) {
    used.delete(normalizedCurrent);
  }

  const baseSlug = safeSlug(baseValue);
  let slug = baseSlug;
  let suffix = 1;

  while (
    used.has(slug) ||
    (slug !== normalizedCurrent && (
      await pathExists(path.join(PROFILE_ROOT, slug)) ||
      await pathExists(path.join(PROFILE_SOURCE_ROOT, slug))
    ))
  ) {
    suffix += 1;
    slug = `${baseSlug}-${suffix}`;
  }

  return slug;
}

async function upsertProfile(user) {
  const userId = String(user?.id || "");
  if (!userId) {
    throw new Error("missing_user_id");
  }

  return withProfileLock(async () => {
    const db = await readProfileDb();
    const current = db.users[userId];
    const preferredName = String(user.username || user.global_name || "user");
    let slug = sanitizeSlug(current?.slug);

    if (!slug) {
      slug = await createUniqueSlug(db, preferredName, "");
    }

    await ensureProfilePage(slug);

    const createdAt = current?.created_at || new Date().toISOString();
    const publicMode = typeof current?.public_mode === "boolean" ? current.public_mode : false;
    const publicProfile = current?.public_profile && typeof current.public_profile === "object"
      ? current.public_profile
      : null;

    db.users[userId] = {
      slug,
      slug_source: current?.slug_source || "username",
      created_at: createdAt,
      last_login_at: new Date().toISOString(),
      username: String(user.username || ""),
      global_name: String(user.global_name || ""),
      public_mode: publicMode,
      public_profile: publicProfile
    };

    await writeProfileDb(db);

    return {
      slug,
      url: `/profile/${slug}/`,
      public_mode: publicMode
    };
  });
}

async function exchangeCodeForToken(code) {
  const body = new URLSearchParams({
    client_id: DISCORD_CLIENT_ID,
    client_secret: DISCORD_CLIENT_SECRET,
    grant_type: "authorization_code",
    code,
    redirect_uri: DISCORD_REDIRECT_URI
  });

  const response = await fetch(TOKEN_ENDPOINT, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body
  });

  const data = await response.json();
  if (!response.ok) {
    throw new Error(`token_exchange_failed:${response.status}:${data.error || "unknown"}`);
  }

  return data;
}

async function refreshAccessToken(refreshToken) {
  const body = new URLSearchParams({
    client_id: DISCORD_CLIENT_ID,
    client_secret: DISCORD_CLIENT_SECRET,
    grant_type: "refresh_token",
    refresh_token: refreshToken
  });

  const response = await fetch(TOKEN_ENDPOINT, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body
  });

  const data = await response.json();
  if (!response.ok) {
    throw new Error(`token_refresh_failed:${response.status}:${data.error || "unknown"}`);
  }

  return data;
}

async function fetchDiscordResource(url, accessToken) {
  const response = await fetch(url, {
    headers: {
      authorization: `Bearer ${accessToken}`,
      "user-agent": "BladephoriafonkOAuth/2.0"
    }
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(`discord_fetch_failed:${response.status}:${text.slice(0, 200)}`);
  }

  return response.json();
}

function buildAvatarUrl(user) {
  if (!user || !user.id || !user.avatar) {
    return "";
  }
  const ext = user.avatar.startsWith("a_") ? "gif" : "png";
  return `https://cdn.discordapp.com/avatars/${user.id}/${user.avatar}.${ext}?size=256`;
}

function buildBannerUrl(user) {
  if (!user || !user.id || !user.banner) {
    return "";
  }
  const ext = user.banner.startsWith("a_") ? "gif" : "png";
  return `https://cdn.discordapp.com/banners/${user.id}/${user.banner}.${ext}?size=1024`;
}

function buildGuildMemberBannerUrl(userId, guildId, bannerHash) {
  if (!userId || !guildId || !bannerHash) {
    return "";
  }
  const ext = String(bannerHash).startsWith("a_") ? "gif" : "png";
  return `https://cdn.discordapp.com/guilds/${guildId}/users/${userId}/banners/${bannerHash}.${ext}?size=1024`;
}

function pickPreferredGuild(user, guilds) {
  if (!Array.isArray(guilds) || !guilds.length) {
    return null;
  }

  if (PRIMARY_GUILD_ID) {
    const fromEnv = guilds.find((guild) => String(guild?.id || "") === PRIMARY_GUILD_ID);
    if (fromEnv) {
      return fromEnv;
    }
  }

  const primaryGuildId = String(user?.primary_guild?.identity_guild_id || "");
  if (primaryGuildId) {
    const fromIdentity = guilds.find((guild) => String(guild?.id || "") === primaryGuildId);
    if (fromIdentity) {
      return fromIdentity;
    }
  }

  if (PRIMARY_GUILD_NAME_HINT) {
    const byName = guilds.find((guild) => String(guild?.name || "").toLowerCase().includes(PRIMARY_GUILD_NAME_HINT));
    if (byName) {
      return byName;
    }
  }

  return guilds[0];
}

async function fetchGuildProfile(accessToken, user) {
  const userId = String(user?.id || "");
  if (!userId) {
    return null;
  }

  try {
    const guilds = await fetchDiscordResource(USER_GUILDS_ENDPOINT, accessToken);
    const selectedGuild = pickPreferredGuild(user, guilds);
    const guildId = String(selectedGuild?.id || "");
    if (!guildId) {
      return null;
    }

    const member = await fetchDiscordResource(`${USER_ENDPOINT}/guilds/${guildId}/member`, accessToken);

    return {
      guild_id: guildId,
      guild_name: String(selectedGuild?.name || ""),
      guild_banner_url: buildGuildMemberBannerUrl(userId, guildId, member?.banner),
      guild_bio: typeof member?.bio === "string" ? member.bio : "",
      guild_nick: typeof member?.nick === "string" ? member.nick : ""
    };
  } catch {
    return null;
  }
}

function toPublicUser(user, guildProfile = null) {
  return {
    username: user?.username || "",
    global_name: user?.global_name || "",
    banner_color: user?.banner_color || null,
    accent_color: typeof user?.accent_color === "number" ? user.accent_color : null,
    public_flags: typeof user?.public_flags === "number" ? user.public_flags : 0,
    avatar_url: buildAvatarUrl(user),
    banner_url: buildBannerUrl(user),
    guild_banner_url: guildProfile?.guild_banner_url || "",
    guild_name: guildProfile?.guild_name || "",
    guild_bio: guildProfile?.guild_bio || "",
    guild_nick: guildProfile?.guild_nick || ""
  };
}

function toPublicConnections(connections) {
  if (!Array.isArray(connections)) {
    return [];
  }

  return connections.map((item) => ({
    name: item?.name || "",
    type: item?.type || "unknown",
    verified: Boolean(item?.verified),
    visibility: typeof item?.visibility === "number" ? item.visibility : 0,
    show_activity: Boolean(item?.show_activity),
    friend_sync: Boolean(item?.friend_sync),
    two_way_link: Boolean(item?.two_way_link),
    revoked: Boolean(item?.revoked)
  }));
}

function buildPublicProfileSnapshot(user, connections, guildProfile = null) {
  const visibleConnections = toPublicConnections(connections)
    .filter((item) => item.visibility === 1 && !item.revoked);

  return {
    user: toPublicUser(user, guildProfile),
    connections: visibleConnections,
    updated_at: new Date().toISOString()
  };
}

function findProfileBySlug(db, slug) {
  for (const [userId, entry] of Object.entries(db.users || {})) {
    if (entry && entry.slug === slug) {
      return { userId, entry };
    }
  }
  return null;
}

function inviteSourceUrl(inviteCode) {
  return `https://discord.com/api/v9/invites/${inviteCode}?with_counts=true&with_expiration=true`;
}

async function requestInviteSnapshot(inviteCode) {
  const endpoint = inviteSourceUrl(inviteCode);
  const response = await fetch(endpoint, {
    headers: {
      "user-agent": "BladephoriafonkWebsite/2.0"
    }
  });

  if (!response.ok) {
    const text = await response.text();
    throw new Error(`invite_fetch_failed:${response.status}:${text.slice(0, 200)}`);
  }

  const data = await response.json();
  const guild = data.guild || {};
  const profile = data.profile || {};
  const guildId = String(guild.id || data.guild_id || "");
  const iconHash = String(guild.icon || profile.icon_hash || "");
  const iconExt = iconHash && iconHash.startsWith("a_") ? "gif" : "png";

  return {
    guild_id: guildId,
    description: String(guild.description || profile.description || ""),
    member_count: data.approximate_member_count ?? profile.member_count ?? 0,
    online_count: data.approximate_presence_count ?? profile.online_count ?? 0,
    verification_level: typeof guild.verification_level === "number" ? guild.verification_level : null,
    premium_tier: typeof guild.premium_tier === "number" ? guild.premium_tier : 0,
    premium_subscription_count: typeof guild.premium_subscription_count === "number" ? guild.premium_subscription_count : 0,
    icon_url: guildId && iconHash ? `https://cdn.discordapp.com/icons/${guildId}/${iconHash}.${iconExt}?size=128` : "",
    source_url: endpoint,
    updated_at: new Date().toISOString(),
    invite_code_used: inviteCode,
    stale: false
  };
}

async function fetchInviteSnapshot(config) {
  const inviteCodes = [config.invite_code, config.fallback_invite_code].filter(Boolean);
  let lastError = null;

  for (const inviteCode of inviteCodes) {
    try {
      const snapshot = await requestInviteSnapshot(inviteCode);
      return {
        key: config.key,
        name: config.name,
        invite_code: config.invite_code,
        main_url: config.main_url,
        alt_url: config.alt_url,
        ...snapshot
      };
    } catch (error) {
      lastError = error;
    }
  }

  throw lastError || new Error("invite_fetch_failed:unknown");
}

async function readInviteProfile(config, options = {}) {
  const cache = inviteCache.get(config.invite_code);
  const shouldUseCache = cache && (now() - cache.fetched_at) < INVITE_CACHE_TTL_MS && !options.force;

  if (shouldUseCache) {
    return {
      ...cache.payload,
      stale: false,
      cached: true
    };
  }

  try {
    const payload = await fetchInviteSnapshot(config);
    inviteCache.set(config.invite_code, {
      fetched_at: now(),
      payload
    });
    return {
      ...payload,
      cached: false
    };
  } catch (error) {
    if (cache) {
      return {
        ...cache.payload,
        stale: true,
        cached: true,
        error: "refresh_failed"
      };
    }

    return {
      key: config.key,
      name: config.name,
      invite_code: config.invite_code,
      main_url: config.main_url,
      alt_url: config.alt_url,
      guild_id: "",
      description: "",
      member_count: 0,
      online_count: 0,
      verification_level: null,
      premium_tier: 0,
      premium_subscription_count: 0,
      icon_url: "",
      source_url: inviteSourceUrl(config.invite_code),
      updated_at: "",
      stale: true,
      cached: false,
      error: error instanceof Error ? error.message : String(error)
    };
  }
}

async function updateSessionData(session, options = {}) {
  if (!session) {
    throw new Error("missing_session");
  }

  session.last_seen_at = now();

  const tokenNeedsRefresh = now() >= session.expires_at - 30_000;
  const hasFreshData = !options.force
    && session.user
    && Array.isArray(session.connections)
    && session.updated_at
    && (now() - session.updated_at) < SESSION_DATA_TTL_MS
    && !tokenNeedsRefresh;

  if (hasFreshData) {
    return session;
  }

  if (tokenNeedsRefresh) {
    if (!session.refresh_token) {
      throw new Error("token_expired");
    }

    const refreshed = await refreshAccessToken(session.refresh_token);
    session.access_token = refreshed.access_token;
    session.refresh_token = refreshed.refresh_token || session.refresh_token;
    session.expires_at = now() + Number(refreshed.expires_in || 3600) * 1000;
  }

  const [user, connections] = await Promise.all([
    fetchDiscordResource(USER_ENDPOINT, session.access_token),
    fetchDiscordResource(CONNECTIONS_ENDPOINT, session.access_token)
  ]);

  const guildProfile = await fetchGuildProfile(session.access_token, user);
  session.user = user;
  session.connections = connections;
  session.guild_profile = guildProfile;
  session.updated_at = now();
  session.last_seen_at = now();

  session.profile = await upsertProfile(user);

  session.profile = await withProfileLock(async () => {
    const db = await readProfileDb();
    const userId = String(user?.id || "");
    const current = db.users[userId];
    if (!current || !current.slug) {
      return session.profile || null;
    }

    const publicMode = current.public_mode === true;
    db.users[userId] = {
      ...current,
      last_login_at: new Date().toISOString(),
      username: String(user.username || ""),
      global_name: String(user.global_name || ""),
      public_mode: publicMode,
      public_profile: publicMode
        ? buildPublicProfileSnapshot(user, connections, guildProfile)
        : null
    };

    await writeProfileDb(db);

    return {
      slug: db.users[userId].slug,
      url: `/profile/${db.users[userId].slug}/`,
      public_mode: publicMode
    };
  });

  return session;
}

function readSession(req) {
  const cookies = parseCookies(req);
  const sessionId = cookies[COOKIE_NAME];
  if (!sessionId) {
    return { sessionId: "", session: null };
  }

  const session = sessions.get(sessionId) || null;
  if (session) {
    session.last_seen_at = now();
  }

  return { sessionId, session };
}

function cleanup() {
  const current = now();

  for (const [key, value] of states.entries()) {
    if (current - value.created_at > STATE_TTL_MS) {
      states.delete(key);
    }
  }

  for (const [key, value] of sessions.entries()) {
    const lastSeenAt = value.last_seen_at || value.created_at;
    if (current - lastSeenAt > SESSION_TTL_MS) {
      sessions.delete(key);
    }
  }
}

async function deletePublishedMedia(id, token) {
  if (!MEDIA_ID_PATTERN.test(id) || typeof token !== "string" || !/^[a-f0-9]{64}$/.test(token)) {
    return false;
  }

  const mediaDirectory = path.join(MEDIA_ROOT, id);
  let storedToken;
  try {
    storedToken = (await fs.readFile(path.join(mediaDirectory, ".delete-token"), "utf8")).trim();
  } catch {
    return false;
  }

  const expected = Buffer.from(storedToken, "utf8");
  const provided = Buffer.from(token, "utf8");
  if (expected.length !== provided.length || !crypto.timingSafeEqual(expected, provided)) {
    return false;
  }

  try {
    await fs.rm(mediaDirectory, { recursive: true, force: false });
  } catch {
    return false;
  }
  return true;
}

setInterval(cleanup, 60_000).unref();

const server = http.createServer(async (req, res) => {
  try {
    const reqUrl = new URL(req.url || "/", BASE_URL);
    const routePath = reqUrl.pathname;

    if (routePath === "/health") {
      return json(res, 200, { ok: true, service: "discord-oauth" });
    }

    if (routePath === "/auth/media-delete" && req.method === "POST") {
      const body = await readJsonBody(req, 64 * 1024);
      const id = typeof body?.id === "string" ? body.id : "";
      const token = typeof body?.token === "string" ? body.token : "";
      const deleted = await deletePublishedMedia(id, token);
      if (!deleted) {
        return json(res, 404, { error: "media_not_found_or_invalid_token" });
      }
      return json(res, 200, { ok: true, deleted: true });
    }

    if (routePath === "/auth/discord/login" && req.method === "GET") {
      if (!DISCORD_CLIENT_ID || !DISCORD_CLIENT_SECRET || !DISCORD_REDIRECT_URI) {
        return json(res, 500, {
          authenticated: false,
          error: "missing_server_config",
          required_env: ["DISCORD_CLIENT_ID", "DISCORD_CLIENT_SECRET", "DISCORD_REDIRECT_URI"]
        });
      }

      const returnTo = sanitizeReturnTo(reqUrl.searchParams.get("return_to") || "/");
      const state = randomToken(20);
      states.set(state, { created_at: now(), return_to: returnTo });

      const authParams = new URLSearchParams({
        client_id: DISCORD_CLIENT_ID,
        response_type: "code",
        redirect_uri: DISCORD_REDIRECT_URI,
        scope: "identify connections guilds guilds.members.read",
        state,
        prompt: "consent"
      });

      return redirect(res, `${AUTH_ENDPOINT}?${authParams.toString()}`);
    }

    if (routePath === "/auth/discord/callback" && req.method === "GET") {
      const code = reqUrl.searchParams.get("code");
      const state = reqUrl.searchParams.get("state");
      const discordError = reqUrl.searchParams.get("error");

      if (!state || !states.has(state)) {
        return json(res, 400, { authenticated: false, error: "invalid_state" });
      }

      const stateData = states.get(state);
      states.delete(state);
      const returnTo = sanitizeReturnTo(stateData?.return_to || "/");

      if (discordError) {
        return redirect(res, `${returnTo}?oauth_error=${encodeURIComponent(discordError)}`);
      }

      if (!code) {
        return redirect(res, `${returnTo}?oauth_error=missing_code`);
      }

      const tokenData = await exchangeCodeForToken(code);
      const sessionId = randomToken(24);
      const session = {
        created_at: now(),
        last_seen_at: now(),
        updated_at: 0,
        access_token: tokenData.access_token,
        refresh_token: tokenData.refresh_token,
        expires_at: now() + Number(tokenData.expires_in || 3600) * 1000,
        user: null,
        connections: [],
        guild_profile: null,
        profile: null
      };

      await updateSessionData(session, { force: true });
      sessions.set(sessionId, session);
      setSessionCookie(res, sessionId);

      const destination = session.profile?.url || returnTo;
      const cleanDestination = destination.replace(/\/+$/, "") || "/";
      return redirect(res, cleanDestination);
    }

    if (routePath === "/auth/session" && req.method === "GET") {
      const refresh = reqUrl.searchParams.get("refresh") === "1";
      const { sessionId, session } = readSession(req);

      if (!sessionId || !session) {
        return json(res, 200, { authenticated: false });
      }

      try {
        await updateSessionData(session, { force: refresh });
      } catch {
        sessions.delete(sessionId);
        clearSessionCookie(res);
        return json(res, 200, { authenticated: false, expired: true });
      }

      return json(res, 200, {
        authenticated: true,
        is_admin: isAdminUser(session.user),
        user: toPublicUser(session.user, session.guild_profile),
        connections: toPublicConnections(session.connections),
        profile: session.profile || null,
        updated_at: new Date(session.updated_at || now()).toISOString()
      });
    }

    if (routePath === "/auth/public-profile" && req.method === "GET") {
      const slug = sanitizeSlug(reqUrl.searchParams.get("slug"));
      if (!slug) {
        return json(res, 400, { error: "invalid_slug" });
      }

      const db = await readProfileDb();
      const found = findProfileBySlug(db, slug);
      if (!found || !found.entry) {
        return json(res, 404, { error: "profile_not_found" });
      }

      if (found.entry.public_mode !== true) {
        return json(res, 403, { error: "profile_private" });
      }

      if (!found.entry.public_profile) {
        return json(res, 404, { error: "public_profile_not_ready" });
      }

      return json(res, 200, {
        public_mode: true,
        slug: found.entry.slug,
        profile: found.entry.public_profile
      });
    }

    if (routePath === "/auth/server-data" && req.method === "GET") {
      const refresh = reqUrl.searchParams.get("refresh") === "1";
      const servers = await Promise.all(PUBLIC_SERVER_CONFIGS.map((config) => readInviteProfile(config, { force: refresh })));
      return json(res, 200, {
        updated_at: new Date().toISOString(),
        servers
      });
    }

    if (routePath === "/auth/changelog" && req.method === "GET") {
      const db = await readChangelogDb();
      const posts = [...db.posts].sort((a, b) => {
        const aTime = Date.parse(a.created_at || "") || 0;
        const bTime = Date.parse(b.created_at || "") || 0;
        return bTime - aTime;
      });

      return json(res, 200, { posts });
    }

    if (routePath === "/auth/changelog" && req.method === "POST") {
      const { sessionId, session } = readSession(req);
      if (!sessionId || !session) {
        return json(res, 401, { authenticated: false, error: "not_authenticated" });
      }

      try {
        await updateSessionData(session);
      } catch {
        sessions.delete(sessionId);
        clearSessionCookie(res);
        return json(res, 401, { authenticated: false, error: "session_expired" });
      }

      if (!isAdminUser(session.user)) {
        return json(res, 403, { error: "admin_required" });
      }

      const body = await readJsonBody(req, CHANGELOG_JSON_BODY_LIMIT_BYTES);
      const content = typeof body?.content === "string" ? body.content : "";
      const title = typeof body?.title === "string" ? body.title.trim() : "";
      const mediaUrl = sanitizeHttpUrl(body?.media_url || "");
      const mediaType = inferMediaType(mediaUrl, body?.media_type);

      if (!content.trim()) {
        return json(res, 400, { error: "content_required" });
      }

      const post = normalizeChangelogPost({
        id: `post_${now()}_${randomToken(6)}`,
        title,
        content,
        media_url: mediaUrl,
        media_type: mediaType,
        created_at: new Date().toISOString(),
        created_by: {
          id: String(session.user?.id || ""),
          username: session.user?.username || "",
          global_name: session.user?.global_name || ""
        }
      });

      await withChangelogLock(async () => {
        const db = await readChangelogDb();
        db.posts.unshift(post);
        await writeChangelogDb(db);
      });

      return json(res, 201, { ok: true, post });
    }

    if (routePath === "/auth/profile-mode" && req.method === "POST") {
      const { sessionId, session } = readSession(req);
      if (!sessionId || !session) {
        return json(res, 401, { authenticated: false, error: "not_authenticated" });
      }

      try {
        await updateSessionData(session);
      } catch {
        sessions.delete(sessionId);
        clearSessionCookie(res);
        return json(res, 401, { authenticated: false, error: "session_expired" });
      }

      const body = await readJsonBody(req);
      const publicMode = typeof body?.public_mode === "boolean" ? body.public_mode : null;
      if (publicMode === null) {
        return json(res, 400, { error: "invalid_public_mode" });
      }

      const userId = String(session.user?.id || "");
      if (!userId) {
        return json(res, 400, { error: "missing_user_id" });
      }

      const updatedProfile = await withProfileLock(async () => {
        const db = await readProfileDb();
        const current = db.users[userId];
        if (!current || !current.slug) {
          return null;
        }

        db.users[userId] = {
          ...current,
          public_mode: publicMode,
          public_profile: publicMode
            ? buildPublicProfileSnapshot(session.user, session.connections, session.guild_profile)
            : null
        };

        await writeProfileDb(db);

        return {
          slug: db.users[userId].slug,
          url: `/profile/${db.users[userId].slug}/`,
          public_mode: publicMode
        };
      });

      if (!updatedProfile) {
        return json(res, 404, { error: "profile_not_found" });
      }

      session.profile = updatedProfile;
      return json(res, 200, { ok: true, profile: updatedProfile });
    }

    if (routePath === "/auth/profile-delete" && req.method === "POST") {
      const { sessionId, session } = readSession(req);
      if (!sessionId || !session) {
        return json(res, 401, { authenticated: false, error: "not_authenticated" });
      }

      try {
        await updateSessionData(session);
      } catch {
        sessions.delete(sessionId);
        clearSessionCookie(res);
        return json(res, 401, { authenticated: false, error: "session_expired" });
      }

      const userId = String(session.user?.id || "");
      if (!userId) {
        return json(res, 400, { error: "missing_user_id" });
      }

      const deleted = await withProfileLock(async () => {
        const db = await readProfileDb();
        const current = db.users[userId];
        if (!current || !current.slug) {
          return null;
        }

        const slug = current.slug;
        delete db.users[userId];
        await writeProfileDb(db);
        await removeProfilePage(slug);
        return { slug };
      });

      if (!deleted) {
        return json(res, 404, { error: "profile_not_found" });
      }

      sessions.delete(sessionId);
      clearSessionCookie(res);
      return json(res, 200, { ok: true, deleted: true, slug: deleted.slug });
    }

    if (routePath === "/auth/logout" && req.method === "POST") {
      const { sessionId } = readSession(req);
      if (sessionId) {
        sessions.delete(sessionId);
      }
      clearSessionCookie(res);
      return json(res, 200, { ok: true, authenticated: false });
    }

    return json(res, 404, { error: "not_found" });
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : String(error);

    if (errorMessage === "invalid_json") {
      return json(res, 400, { error: "invalid_json" });
    }

    if (errorMessage === "payload_too_large") {
      return json(res, 413, { error: "payload_too_large" });
    }

    console.error("[oauth] request failed", error);
    return json(res, 500, { error: "internal_error" });
  }
});

await ensureDefaultFile(PROFILE_DB_FILE, defaultProfileDb());
await ensureDefaultFile(CHANGELOG_DB_FILE, defaultChangelogDb());
await syncAllGeneratedProfiles();

server.listen(PORT, HOST, () => {
  console.log(`Discord OAuth server listening on http://${HOST}:${PORT}`);
  console.log(`Profile template: ${PROFILE_TEMPLATE_FILE}`);
  console.log(`Profile output: ${PROFILE_ROOT}`);
});
