# Discord OAuth2 + Website backend

Backend jest napisany w Ruscie i lezy w [`backend/`](../backend). Jeden
program (`bladephoria-web`) obsluguje:

- strony i assety (`/`, `/server`, `/administracja`, `/changelog`, `/assets/`, `/profile/`, `/media/`),
- logowanie Discord OAuth2,
- profile `/profile/<slug>/` i tryb publiczny profilu,
- changelog,
- publiczne dane serwerow Discord pobierane po stronie backendu,
- usuwanie opublikowanych mediow po tokenie.

Ten katalog (`oauth/`) trzyma juz tylko konfiguracje i dane: `.env`,
`profile-template.html` i `data/`.

## Co jest wazne

- Trzymaj `.env`, `profiles.json` i `changelog.json` poza katalogiem publicznym strony.
- Serwer nie wystawia `oauth/`, `src/`, `backend/` ani plikow zaczynajacych sie od kropki.
- Publiczne strony profili korzystaja ze wspolnych assetow, wiec poprawki profilu propaguja sie po ponownym syncu backendu.
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

- `PROFILE_TEMPLATE_FILE` wskazuje wspolny szablon profilu.
- `PROFILE_ROOT` i `PROFILE_SOURCE_ROOT` dostaja wygenerowane profile.
- `PROFILE_DB_FILE` przechowuje profile i ustawienia publiczne.
- `CHANGELOG_DB_FILE` przechowuje wpisy changeloga.
- `MEDIA_ROOT` to katalog serwowany pod `/media/`.

Formaty plikow danych sa takie same jak w wersji Node, wiec istniejace
`profiles.json` i `changelog.json` dzialaja bez migracji.

## Uwagi

- Sesje sa trzymane w pamieci procesu. Restart backendu wyloguje aktywne sesje.
- Jesli zmienisz wspolny szablon profilu, backend przy starcie zsynchronizuje wygenerowane strony profilowe z aktualna wersja.
- `GET /auth/server-data` korzysta z cache, a reczny refresh na froncie moze wymusic odswiezenie.
