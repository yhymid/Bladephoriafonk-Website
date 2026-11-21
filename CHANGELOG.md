# Changelog

## Unreleased

### Added
- Diagnostics 24/7 (off by default, `BLADEPHORIA_DIAGNOSTICS=1`): every request, Discord call,
  data file operation and background task is recorded to `diagnostics/events-<day>.jsonl`, and
  a Polish report with p50/p90/p99 per route, a time breakdown, the slowest requests, anomalies
  and hints is written to `diagnostics/summary-<day>.md` every 15 minutes. No secrets are
  recorded. Old files are removed after 14 days.
- An end-to-end test that runs the real server against a fake Discord API.
- The diagnostics report shows the memory used at the end of each of the last 7 days and warns
  when it grows every day.
- `bladephoria-web --check-config` validates the configuration and the data files without starting
  the server or writing anything; `--version` prints the version.
- GitHub Actions CI: fmt, clippy, unit and end-to-end tests, release build, `--check-config` on
  the example configuration, the MSRV (Rust 1.88) and a check of links between the Markdown
  files (`scripts/check-links.py`). Actions are pinned to commit SHAs.
- Tests on saved Discord responses (`backend/tests/fixtures/discord/`): real public invite
  responses and a login in the shape of the Discord API are compared with expected public JSON,
  and the tests check that no private field (e-mail, hidden connections, roles) reaches it.
  `BLADEPHORIA_UPDATE_SNAPSHOTS=1 cargo test` refreshes the expected files.
- A read-only admin panel on `127.0.0.1` (`ADMIN_PANEL_PORT`, off by default), opened with
  `scripts/admin-tunnel.sh`: overview (version, memory, sessions, which Discord invite works),
  the diagnostics reports, the data files and the service log with a filter and search. No
  JavaScript, strict CSP, loopback Host only, secrets masked.

### Changed
- `deploy/deploy.sh` builds in `~/src/Bladephoriafonk-Website`, keeps only the files the server
  needs in the site directory, backs up the configuration and the previous binary to
  `~/backups/` before every swap and uses the `operator_vps` SSH key.
- The server binary lives in the site directory (`bladephoria-web`).
- `README.md` moved from `oauth/` to the root of the repository.
- `deploy/deploy.sh` runs `--check-config` with the new binary before anything on the server
  changes, and the configuration backup also holds `oauth/data/`.
- Documentation in English and Polish: `README.md` / `README.pl.md` and `deploy/DEPLOY.md` /
  `deploy/DEPLOY.pl.md`, with a table of every file on the server (who writes it, what it
  holds, whether it can be deleted, what to back up). `oauth/.env.example` comments are in
  English.
- `deploy/deploy.sh` installs the release binary that CI built for the deployed commit (checked
  by SHA256 and glibc version) instead of compiling on the VPS; `BPF_BUILD=vps` keeps the old
  way. A static musl build was measured 1.5-2.8x slower per request and is not used.

### Fixed
- Diagnostics no longer count complete responses of streamed files (pages without compression)
  as aborted.
- `deploy/deploy.sh` copies runtime files by allowlist; before, `.github/` and `scripts/` ended
  up in the site directory (never served). The next deploy removes them and the empty folders.

### Security
- At most `MAX_PENDING_LOGINS` (default 10000, 0 = no limit) login attempts are kept while
  waiting for Discord, so a flood of login requests cannot fill the memory.
- A systemd sandbox (`deploy/bladephoria-oauth.service.d/hardening.conf`): the server can write
  only to `oauth/data/`, `diagnostics/` and `media/`, has no capabilities and creates files
  with mode 600. `systemd-analyze security` went from 9.2 (unsafe) to 1.6 (ok). Data and
  diagnostics files on the server are 600, their directories 700.
- Damaged data files are never overwritten: a `profiles.json` or `changelog.json` that is blank,
  not valid JSON or has the wrong structure (for example truncated to `{}`) is refused instead
  of being treated as empty, a copy `<file>.corrupt-<hash>` is kept next to it and
  `--check-config` shows how to restore the `.bak`. `STRICT_DATA_FILES=0` restores the old
  behaviour.

### Removed
- `assets/favicon.svg` from the repository (it stays on the server with the other uploads).

## 2.0 - Rust Update (2026-11-16)

### Changed
- The backend is written in Rust now. One small program replaces the two Node.js servers and
  serves the pages, profiles, media and the login API. The look of the site is unchanged.
- Pages are sent compressed and the server uses a fraction of the memory it needed before.
- All profiles use one template (`profile/index.html`); a new profile works right after the
  first login, without generated per-user copies.
- Polish texts have proper diacritics and the English wording is cleaner.
- Only the pages, assets, media and profiles are public. Everything else answers with a 404 page.
- The Server page no longer has the introduction paragraph.

### Added
- Logins survive a server restart.
- A 404 page for addresses that do not exist.
- A readable message when logging in with Discord fails.
- The Server page shows a Discord invite that works; when an invite expires, the next one from
  the list takes over.
- Large downloads from `/media/` can be resumed.
- `deploy/deploy.sh`: upload, build, restart, health check and rollback in one command.

### Fixed
- The Bladephoriafonk server card showed no data because its invite had expired.

### Removed
- Unused style and script files in `assets/` and the duplicate profile templates.

## 1.1 - Few Patch for Future (2026-05-26)

### Added
- Permission administration.
- English language (beta).

## 1.0 - Init Project (2026-01-01)

- First version of the Bladephoriafonk website.
