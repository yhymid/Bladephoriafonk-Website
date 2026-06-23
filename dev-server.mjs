import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promises as fs } from "node:fs";
import { Readable } from "node:stream";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const PORT = Number(process.env.DEV_PORT || 8080);
const HOST = process.env.DEV_HOST || "127.0.0.1";
const OAUTH_ORIGIN = process.env.DEV_OAUTH_ORIGIN || "http://127.0.0.1:3001";

const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".mp4": "video/mp4",
  ".webm": "video/webm",
  ".ogg": "video/ogg",
  ".txt": "text/plain; charset=utf-8"
};

const BLOCKED_PREFIXES = ["/oauth/", "/src/"];
const ALLOWED_TOP_LEVEL_FILES = new Set([
  "/index.html",
  "/server.html",
  "/administracja.html",
  "/changelog.html",
  "/site-lang.js"
]);

function contentSecurityPolicy() {
  return [
    "default-src 'self'",
    "base-uri 'self'",
    "form-action 'self'",
    "frame-ancestors 'none'",
    "object-src 'none'",
    "script-src 'self' 'unsafe-inline'",
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' https: data:",
    "media-src 'self' https:",
    "connect-src 'self' https://discord.com https://cdn.discordapp.com",
    "font-src 'self' data:"
  ].join("; ");
}

function safeDecode(pathname) {
  try {
    return decodeURIComponent(pathname);
  } catch {
    return null;
  }
}

async function fileExists(filePath) {
  try {
    const stat = await fs.stat(filePath);
    return stat.isFile();
  } catch {
    return false;
  }
}

function isHiddenPath(decodedPathname) {
  return decodedPathname.split("/").some((segment) => segment.startsWith("."));
}

function isAllowedPublicPath(decodedPathname) {
  if (decodedPathname === "/") {
    return true;
  }

  if (isHiddenPath(decodedPathname)) {
    return false;
  }

  if (BLOCKED_PREFIXES.some((prefix) => decodedPathname.startsWith(prefix))) {
    return false;
  }

  if (decodedPathname.startsWith("/assets/")) {
    return true;
  }

  if (decodedPathname.startsWith("/profile/")) {
    return true;
  }

  if (ALLOWED_TOP_LEVEL_FILES.has(decodedPathname)) {
    return true;
  }

  const pathSegments = decodedPathname.split("/").filter(Boolean);
  if (pathSegments.length === 1 && !path.extname(decodedPathname)) {
    return ["/server", "/administracja", "/changelog", "/profile"].includes(decodedPathname);
  }

  return false;
}

async function resolveStaticPath(pathname) {
  const decoded = safeDecode(pathname);
  if (!decoded) {
    return null;
  }

  if (!isAllowedPublicPath(decoded)) {
    return null;
  }

  let candidate = decoded;
  if (candidate === "/") {
    candidate = "/index.html";
  }

  if (candidate.endsWith("/")) {
    candidate = `${candidate}index.html`;
  }

  const noExt = !path.extname(candidate);
  if (noExt) {
    const htmlCandidate = `${candidate}.html`;
    const htmlPath = path.join(__dirname, htmlCandidate);
    if (await fileExists(htmlPath)) {
      candidate = htmlCandidate;
    } else {
      const indexCandidate = path.join(candidate, "index.html");
      const indexPath = path.join(__dirname, indexCandidate);
      if (await fileExists(indexPath)) {
        candidate = indexCandidate;
      }
    }
  }

  const normalized = path.normalize(path.join(__dirname, candidate));
  if (!normalized.startsWith(__dirname)) {
    return null;
  }

  return normalized;
}

function setStaticHeaders(res, contentType) {
  const isHtml = contentType.startsWith("text/html");
  const cacheControl = isHtml ? "no-store" : "public, max-age=300";

  res.setHeader("content-type", contentType);
  res.setHeader("cache-control", cacheControl);
  res.setHeader("x-content-type-options", "nosniff");
  res.setHeader("x-frame-options", "DENY");
  res.setHeader("referrer-policy", "strict-origin-when-cross-origin");
  res.setHeader("permissions-policy", "camera=(), microphone=(), geolocation=()");
  res.setHeader("content-security-policy", contentSecurityPolicy());
}

function setProxyResponseHeaders(res, headers) {
  for (const [key, value] of headers.entries()) {
    if (key.toLowerCase() === "set-cookie") {
      continue;
    }
    res.setHeader(key, value);
  }

  if (typeof headers.getSetCookie === "function") {
    const setCookies = headers.getSetCookie();
    if (setCookies.length) {
      res.setHeader("set-cookie", setCookies);
    }
  }
}

async function proxyToOauth(req, res, targetUrl) {
  const headers = new Headers();

  Object.entries(req.headers).forEach(([key, value]) => {
    if (key.toLowerCase() === "host") {
      return;
    }

    if (Array.isArray(value)) {
      headers.set(key, value.join(", "));
      return;
    }

    if (typeof value === "string") {
      headers.set(key, value);
    }
  });

  const method = req.method || "GET";
  const withBody = !(method === "GET" || method === "HEAD");

  const upstream = await fetch(targetUrl, {
    method,
    headers,
    body: withBody ? req : undefined,
    duplex: withBody ? "half" : undefined,
    redirect: "manual"
  });

  setProxyResponseHeaders(res, upstream.headers);
  res.writeHead(upstream.status);

  if (!upstream.body) {
    res.end();
    return;
  }

  Readable.fromWeb(upstream.body).pipe(res);
}

const server = http.createServer(async (req, res) => {
  try {
    const requestUrl = new URL(req.url || "/", `http://${HOST}:${PORT}`);
    const pathname = requestUrl.pathname;

    if (pathname.startsWith("/auth/") || pathname === "/health") {
      const targetUrl = new URL(requestUrl.pathname + requestUrl.search, OAUTH_ORIGIN);
      await proxyToOauth(req, res, targetUrl);
      return;
    }

    const resolvedPath = await resolveStaticPath(pathname);
    if (!resolvedPath || !(await fileExists(resolvedPath))) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("Not found");
      return;
    }

    const ext = path.extname(resolvedPath).toLowerCase();
    const contentType = MIME_TYPES[ext] || "application/octet-stream";
    const data = await fs.readFile(resolvedPath);

    setStaticHeaders(res, contentType);
    res.writeHead(200);
    res.end(data);
  } catch (error) {
    res.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
    res.end(error instanceof Error ? error.message : String(error));
  }
});

server.listen(PORT, HOST, () => {
  console.log(`Dev server running on http://${HOST}:${PORT}`);
  console.log(`Proxying /auth/* to ${OAUTH_ORIGIN}`);
});
