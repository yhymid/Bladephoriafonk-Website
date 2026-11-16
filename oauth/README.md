# Discord OAuth2 + Website backend

Backend jest napisany w Ruscie i lezy w [`backend/`](../backend). Jeden
program (`bladephoria-web`) obsluguje:

- strony i assety (`/`, `/server`, `/administracja`, `/changelog`, `/assets/`, `/profile/`, `/media/`),
- logowanie Discord OAuth2,
- profile `/profile/<slug>/` i tryb publiczny profilu,
- changelog,
- publiczne dane serwerow Discord pobierane po stronie backendu,
- usuwanie opublikowanych mediow po tokenie.

Ten katalog (`oauth/`) trzyma juz tylko konfiguracje i dane: `.env` i `data/`.

## Co jest wazne

- `.env` i pliki w `data/` nie sa nigdy wystawiane publicznie.
- Serwer nie wystawia `oauth/`, `backend/`, `deploy/` ani plikow zaczynajacych sie od kropki.
- Wszystkie profile uzywaja jednego szablonu `profile/index.html`; zmiana szablonu dziala od razu dla kazdego profilu.
- Sesje i publiczne dane serwerow sa cache'owane, zeby ograniczyc ruch do Discord API.

## Wymagane dane w `.env`

- `DISCORD_CLIENT_ID`
- `DISCORD_CLIENT_SECRET`
- `DISCORD_REDIRECT_URI`
- `BASE_URL`

Domyslny redirect dla domeny:

- `https://bladephoriafonk.xyz/auth/discord/callback`

Pelna lista zmiennych: [.env.example](.env.example). Zmienne ustawione
w srodowisku maja pierwszenstwo przed plikiem `--env-file`.

## Lokalne uruchomienie

```bash
cp oauth/.env.example oauth/.env   # uzupelnij dane Discorda, ustaw lokalne sciezki
cd backend
SITE_ROOT=.. PORT=8080 BASE_URL=http://127.0.0.1:8080 cargo run --release -- --env-file ../oauth/.env
```

Potem otworz:

- `http://127.0.0.1:8080`

Osobny serwer deweloperski nie jest juz potrzebny - ten sam program serwuje
strony i API.

Testy i kontrola kodu:

```bash
cd backend
cargo test
cargo clippy --all-targets -- -D warnings
```

## Wdrozenie na VPS

Opis: [deploy/DEPLOY.md](../deploy/DEPLOY.md). Przyklady:

- [deploy/nginx-site.conf.example](../deploy/nginx-site.conf.example)
- [deploy/bladephoria-oauth.service.example](../deploy/bladephoria-oauth.service.example)

## Endpointy

- `GET /health`
- `GET /auth/discord/login?return_to=<sciezka>`
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

## Dane i pliki

- `PROFILE_TEMPLATE_FILE` - szablon profilu (domyslnie `profile/index.html`), podawany pod `/profile/<slug>/` dla istniejacych profili.
- `PROFILE_DB_FILE` - profile i ustawienia publiczne.
- `CHANGELOG_DB_FILE` - wpisy changeloga.
- `SESSION_DB_FILE` - zapisane sesje logowania (prawa 0600).
- `MEDIA_ROOT` - katalog serwowany pod `/media/`.

## Uwagi

- Sesje sa zapisywane do pliku, wiec restart backendu nie wylogowuje uzytkownikow.
- Nieudane logowanie wraca na strone z `?oauth_error=<kod>`; `site-lang.js` pokazuje wtedy komunikat.
- Zaproszenia Discord sa w `backend/src/discord.rs` (lista kodow w kolejnosci). Gdy pierwszy kod wygasnie, backend bierze nastepny, a strona `/server` sama pokazuje dzialajacy link.
- `GET /auth/server-data` korzysta z cache, a reczny refresh na froncie moze wymusic odswiezenie.
