# Deploying to the VPS

**English** · [Polski](DEPLOY.pl.md)

The whole site runs as one Rust program (`backend/`, binary `bladephoria-web`) that serves
the pages, profiles, media and the `/auth/*` API. Nginx only terminates TLS and forwards all
traffic to it (127.0.0.1:3001).

## Updating with one command

```bash
deploy/deploy.sh
```

The script:

1. uploads the committed state of the repository to `~/src/Bladephoriafonk-Website` on the VPS
   and builds the binary there (`cargo build --release --locked`; the VPS has a different glibc
   than the local machine, so it is built only there),
2. lets the new binary check the production configuration and data (`--check-config`); on an
   error it stops before anything on the server changes,
3. backs up the configuration and data (`oauth/.env`, `oauth/data/`, the systemd unit with its
   drop-ins, the nginx site) to `~/backups/website-config-<time>.tar.gz`; only the newest is kept,
4. copies only the files the server needs into the site directory (pages, `site-lang.js`,
   `profile/index.html`); files removed from the repository disappear from the server (list in
   `.deployed-files`),
5. when the binary changed: keeps the previous one as `~/backups/bladephoria-web.bin.<commit>`
   (only one), swaps `bladephoria-web` in the site directory, restarts the service and checks
   `/health`. When the new version does not answer, the previous binary is restored.

It never touches `oauth/.env`, `oauth/data/`, `media/`, `diagnostics/` or `assets/` (the
modpack uploads live there).

Settings (environment variables): `BPF_DEPLOY_SSH`, `BPF_DEPLOY_KEY`, `BPF_DEPLOY_DIR`,
described at the top of the script.

## First install

1. Rust on the VPS (`rustup`), site directory `/home/ubuntu/Main Files/Website`.
2. `oauth/.env` from [oauth/.env.example](../oauth/.env.example).
3. The systemd service: [bladephoria-oauth.service.example](bladephoria-oauth.service.example)

   ```bash
   sudo cp deploy/bladephoria-oauth.service.example /etc/systemd/system/bladephoria-oauth.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now bladephoria-oauth
   ```

   The sandbox for the service:
   [bladephoria-oauth.service.d/hardening.conf](bladephoria-oauth.service.d/hardening.conf)

   ```bash
   sudo install -D -m 644 deploy/bladephoria-oauth.service.d/hardening.conf \
     /etc/systemd/system/bladephoria-oauth.service.d/hardening.conf
   sudo systemctl daemon-reload && sudo systemctl restart bladephoria-oauth
   systemd-analyze security bladephoria-oauth   # 1.6 OK (9.2 without it)
   ```

   The service can then write only to `oauth/data/`, `diagnostics/` and `media/`; the rest of
   the system is read-only and new files get mode 600 (`UMask=0077`). Rollback: delete the
   file, `daemon-reload`, restart. Keep the data private: `chmod 700 oauth oauth/data diagnostics`.

4. Nginx: [nginx-site.conf.example](nginx-site.conf.example).
5. `deploy/deploy.sh` from the local machine.

## Every file on the server

Paths relative to the site directory unless they start with `~`.

| Path | Written by | Contents | Can it be deleted? | Back it up? |
|---|---|---|---|---|
| `bladephoria-web` | `deploy.sh` | the server binary | no (the next deploy puts it back) | the previous one is kept in `~/backups` |
| `*.html`, `site-lang.js`, `profile/index.html` | `deploy.sh` | pages and the profile template | no (redeploy restores them) | no, they are in git |
| `assets/` | you, by hand | modpack icons, favicon | no | yes |
| `media/` | you, by hand | downloads and videos; `<dir>/.delete-token` allows deleting a published item | no | yes |
| `oauth/.env` | you | configuration with the Discord secret (600) | no | yes (in every deploy backup) |
| `oauth/data/profiles.json` | the server (logins, profile settings) | profiles and their public mode | no | yes |
| `oauth/data/changelog.json` | the server (`POST /auth/changelog`) | changelog posts | no | yes |
| `oauth/data/sessions.json` | the server (every 15 s when changed, on stop) | logins with Discord tokens (600) | yes, everyone is logged out | only in the deploy backup |
| `oauth/data/*.bak` | the server (before every write) | the previous version of each file | yes | no |
| `oauth/data/*.corrupt-<hash>` | the server | copy of a damaged file | yes, once the file is fixed | no |
| `diagnostics/` | the server | `events-*.jsonl`, `summary-*.md`, deleted after 14 days | yes | no |
| `.deployed-commit`, `.deployed-files` | `deploy.sh` | deployed commit and file list | yes, but then the next deploy cannot remove stale files | no |
| `~/src/Bladephoriafonk-Website` | `deploy.sh` | build copy of the repository and the cargo cache | yes (the next build takes longer) | no |
| `~/backups/website-config-*.tar.gz` | `deploy.sh` | `.env`, `oauth/data/`, unit, nginx site (newest only) | keep the newest | it is the backup |
| `~/backups/bladephoria-web.bin.<commit>` | `deploy.sh` | the previous binary (one) | keep it for a rollback | it is the backup |

## What is public

- `/`, `/server`, `/administracja`, `/changelog`
- `/site-lang.js`, `/assets/`, `/media/`
- `/profile/` and `/profile/<slug>/` (only for existing profiles)
- `/auth/*`, `/health`

Everything else (`oauth/`, `diagnostics/`, the binary, files starting with a dot such as
`.env` or `.delete-token`) gets the 404 page.

## Checks after a deploy

```bash
curl -I https://bladephoriafonk.xyz/
curl -s https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env   # 404
systemctl show bladephoria-oauth -p NRestarts     # 0
ls "/home/ubuntu/Main Files/Website/diagnostics"  # when diagnostics are on
```
