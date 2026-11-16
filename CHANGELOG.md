# Changelog

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
