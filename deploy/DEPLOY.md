# Deploy na VPS

Cala strona dziala jako jeden program w Ruscie (`backend/`, binarka
`bladephoria-web`). Zastepuje on stary `oauth/discord-auth-server.mjs`
i `dev-server.mjs`; Node nie jest juz potrzebny.

## 1. Build

Buduj na VPS (albo na maszynie z tym samym systemem), zeby binarka pasowala
do wersji glibc serwera:

```bash
cd "/home/ubuntu/Main Files/Website/backend"
cargo build --release
```

Wynik: `backend/target/release/bladephoria-web`.

## 2. Konfiguracja

Zmienne sa takie same jak w wersji Node, wiec istniejacy `oauth/.env` dziala
bez zmian. Wzor: [oauth/.env.example](../oauth/.env.example). Nowe, opcjonalne:

- `SITE_ROOT` - katalog z `index.html` (domyslnie katalog roboczy),
- `SERVE_STATIC` - `false` wylacza serwowanie stron (zostaje samo `/auth/*`),
- `MEDIA_ROOT` - katalog `/media/` (domyslnie `SITE_ROOT/media`).

Dane najlepiej trzymac poza katalogiem strony:

- `PROFILE_DB_FILE=/var/lib/bladephoria/profiles.json`
- `CHANGELOG_DB_FILE=/var/lib/bladephoria/changelog.json`

## 3. Usluga systemd

Wzor: [bladephoria-oauth.service.example](bladephoria-oauth.service.example).

```bash
sudo cp deploy/bladephoria-oauth.service.example /etc/systemd/system/bladephoria-oauth.service
sudo systemctl daemon-reload
sudo systemctl restart bladephoria-oauth
```

Restart wylogowuje aktywne sesje (sa trzymane w pamieci, jak wczesniej).

## 4. Nginx

Sa dwie mozliwosci:

1. **Bez zmian w Nginx.** Dotychczasowy config (Nginx serwuje pliki,
   `/auth/` idzie na `127.0.0.1:3001`) dziala dalej, bo API i generowane
   pliki profili sa identyczne.
2. **Wszystko przez serwer Rust.** Nginx tylko terminuje TLS:
   [nginx-site.conf.example](nginx-site.conf.example). Serwer sam pilnuje,
   co jest publiczne, ustawia naglowki bezpieczenstwa i kompresuje odpowiedzi
   (gzip/brotli).

## 5. Co jest publiczne

Serwer wystawia tylko:

- `/`, `/server`, `/administracja`, `/changelog`
- `/site-lang.js`, `/assets/`, `/profile/`, `/media/`
- `/auth/*`, `/health`

Wszystko inne (`oauth/`, `src/`, `backend/`, `deploy/`, pliki z kropka na
poczatku, np. `.env` czy `.delete-token`) zwraca `404`.

## 6. Kontrola po wdrozeniu

```bash
curl -I https://bladephoriafonk.xyz/
curl -I https://bladephoriafonk.xyz/server
curl -s https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env
```

Ostatnie zapytanie powinno zwrocic `404`.
