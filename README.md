# Bladephoriafonk Website

**English** · [Polski](README.pl.md)

The website at https://bladephoriafonk.xyz. One small Rust program
(`bladephoria-web`, source in [`backend/`](backend)) serves:

- the pages and assets (`/`, `/server`, `/administracja`, `/changelog`, `/assets/`, `/profile/`, `/media/`),
- Discord OAuth2 login,
- profiles at `/profile/<slug>/` with an optional public mode,
- the changelog,
- public data of the Discord servers, fetched by the backend,
- deleting published media with its token.

The `oauth/` directory only holds configuration and data: `.env` and `data/`.

## Documentation

| Document | What it covers |
|---|---|
| [deploy/DEPLOY.md](deploy/DEPLOY.md) | Deploying to the VPS, first install, every file on the server, checks |
| [CHANGELOG.md](CHANGELOG.md) | Changes per version |
| [oauth/.env.example](oauth/.env.example) | Every configuration variable |

## Good to know

- `.env` and the files in `data/` are never public.
- Only the pages, assets, media, profiles, `/auth/*` and `/health` are served; everything
  else (`oauth/`, `diagnostics/`, the binary, files starting with a dot) gets the 404 page.
- Every profile uses the one template `profile/index.html`; a change to it applies to all
  profiles at once.
- Sessions and the public server data are cached to keep traffic to the Discord API low.
- Sessions are saved to a file, so a restart does not log anyone out.
- A failed login returns to the page with `?oauth_error=<code>`; `site-lang.js` shows a message.
- The Discord invites are ordered code lists in `backend/src/discord.rs`. When the first code
  expires the backend uses the next one, and `/server` shows the working link by itself.

## Configuration

Required in `.env`: `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_REDIRECT_URI`
(for the domain: `https://bladephoriafonk.xyz/auth/discord/callback`) and `BASE_URL`.
All variables: [oauth/.env.example](oauth/.env.example). Variables set in the environment
win over the `--env-file` file.

Check the configuration without starting the server (writes nothing, never prints secrets,
exit code 1 on errors):

```bash
cd backend
cargo run --release -- --env-file ../oauth/.env --check-config
```

## Running locally

```bash
cp oauth/.env.example oauth/.env   # fill in the Discord settings and local paths
cd backend
SITE_ROOT=.. PORT=8080 BASE_URL=http://127.0.0.1:8080 cargo run --release -- --env-file ../oauth/.env
```

Then open http://127.0.0.1:8080. The same program serves the pages and the API; no separate
development server is needed.

Tests and checks (CI runs the same on every push):

```bash
cd backend
cargo test                                   # unit tests + e2e (real binary, fake Discord)
BLADEPHORIA_UPDATE_SNAPSHOTS=1 cargo test    # refresh the expected files in tests/fixtures/discord/
cargo clippy --all-targets -- -D warnings
python3 ../scripts/check-links.py            # links between the Markdown files
```

## Diagnostics

Switched on in `.env` with `BLADEPHORIA_DIAGNOSTICS=1`. The server then writes to
`diagnostics/` (next to `index.html`; not public):

- `events-YYYY-MM-DD.jsonl`: one JSON line per event, times in ms:
  `http` (route, status, TTFB, total and send time, size, compression, time spent in Discord /
  files / locks), `discord` (endpoint, status, time, rate-limit headers), `io` (data file reads
  and writes), `lock`, `tick` (delay of periodic tasks), `resources` (every minute: memory,
  CPU, threads, file descriptors, event-loop lag, sessions), `anomaly` (expired invite, 429,
  bad JSON, clock skew, failed login, damaged data file) and `lifecycle` (start, stop, data audit);
- `summary-YYYY-MM-DD.md`: a report in Polish, refreshed every 15 minutes and on shutdown:
  p50/p90/p99 per route, where the time goes, the slowest requests, Discord API, files,
  resources (with the memory of the last 7 days), anomalies and hints.

Days are UTC. A separate thread writes through a bounded queue, so recording never slows a
response down (a full queue counts the event as dropped). No secrets are recorded: no query
strings, cookies, tokens, request bodies or IP addresses, only route labels (`/profile/:slug`,
`/media/*`, `(404)`), times, sizes and counters.

| Variable | Default | Meaning |
|---|---|---|
| `BLADEPHORIA_DIAGNOSTICS` | off | `1` switches it on |
| `BLADEPHORIA_DIAGNOSTICS_DIR` | `SITE_ROOT/diagnostics` | directory |
| `BLADEPHORIA_DIAGNOSTICS_RETENTION_DAYS` | `14` | files older than this are deleted |
| `BLADEPHORIA_DIAGNOSTICS_MAX_MB` | `50` | size limit of one day's event file |
| `BLADEPHORIA_DIAGNOSTICS_SUMMARY_MINUTES` | `15` | report interval |

## Damaged data files

When `profiles.json` or `changelog.json` is not valid JSON, is blank or has the wrong
structure (for example truncated to `{}`), the server:

- does not treat it as an empty database and never overwrites it (logins and new posts fail
  instead of wiping the data),
- keeps a copy `<file>.corrupt-<hash>` next to it (one per content; the original is untouched),
- reports a `damaged_data_file` anomaly in the diagnostics.

`bladephoria-web --check-config` shows what is wrong and, when the previous version
`<file>.bak` is fine, prints the `cp` command that restores it. `STRICT_DATA_FILES=0` brings
back the old lenient behaviour.

## Admin panel

A read-only panel on a separate port, bound to `127.0.0.1` of the VPS only and reached
through an SSH tunnel; nginx never exposes it. Set `ADMIN_PANEL_PORT` (3002 on production,
`0` = off) and open it with:

```bash
scripts/admin-tunnel.sh            # then http://127.0.0.1:3002, Ctrl+C closes the tunnel
```

| Page | What it shows |
|---|---|
| Overview | version and commit, uptime, memory, sessions and pending logins, which Discord invite works for each server (main or a backup code), today's hints and anomalies |
| Diagnostics | the daily reports, rendered |
| Data | the data files with size, time and whether they are valid, `.bak` / `.corrupt-*` copies, the latest changelog posts, the profiles |
| Logs | the last 300 lines of the service log, newest first, a warnings-and-errors filter and search |

No JavaScript, no forms that change anything, a strict CSP. Requests must be addressed to
`127.0.0.1`, `localhost` or `[::1]` (any port), which blocks DNS rebinding. Session ids and
tokens are never shown; log lines are masked (the client secret and any 32+ character token).
The logs come from `journalctl` without sudo (the service user is in the `adm` group).

## Endpoints

- `GET /health`
- `GET /auth/discord/login?return_to=<path>`
- `GET /auth/discord/callback`
- `GET /auth/session`
- `GET /auth/public-profile?slug=<slug>`
- `POST /auth/profile-mode`
- `POST /auth/profile-delete`
- `POST /auth/logout`
- `GET /auth/changelog`
- `POST /auth/changelog`
- `GET /auth/server-data`
- `POST /auth/media-delete`
