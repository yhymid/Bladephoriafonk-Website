# Discord OAuth2 + Website backend

Ten backend obsluguje:
- logowanie Discord OAuth2,
- profile `/profile/<slug>/`,
- tryb publiczny profilu,
- changelog,
- publiczne dane serwerow Discord pobierane po stronie backendu.

## Co jest teraz wazne

- Trzymaj `.env`, `profiles.json` i `changelog.json` poza katalogiem publicznym strony.
- Nie wystawiaj `oauth/` ani `src/` przez Nginx.
- Publiczne strony profili korzystaja ze wspolnych assetow, wiec poprawki profilu propagują sie po ponownym syncu backendu.
- Sesje i publiczne dane serwerow sa cache'owane, zeby ograniczyc ruch do Discord API.

## Wymagane dane w `.env`

- `DISCORD_CLIENT_ID`
- `DISCORD_CLIENT_SECRET`
- `DISCORD_REDIRECT_URI`
- `BASE_URL`

Domyslny redirect dla domeny:

- `https://bladephoriafonk.xyz/auth/discord/callback`

## Lokalne uruchomienie

```bash
cd oauth
cp .env.example .env
npm start
```

Backend startuje domyslnie na `127.0.0.1:3001`.

Dla calej strony lokalnie:

```bash
cd ..
DEV_PORT=8080 DEV_OAUTH_ORIGIN=http://127.0.0.1:3001 node dev-server.mjs
```

Potem otworz:

- `http://127.0.0.1:8080`

## Wdrozenie na VPS

Gotowe przyklady sa w:

- [deploy/nginx-site.conf.example](/run/media/anon/Nowy/Marmak/git.marmak.net.pl/Website/deploy/nginx-site.conf.example)
- [deploy/bladephoria-oauth.service.example](/run/media/anon/Nowy/Marmak/git.marmak.net.pl/Website/deploy/bladephoria-oauth.service.example)

Najwazniejsze zasady:

1. Katalog publiczny Nginx ustaw na `Website/`.
2. Zablokuj publiczny dostep do `oauth/`, `src/` i dotfiles.
3. Proxy tylko `/auth/` do backendu OAuth.
4. Pliki danych trzymaj np. w `/var/lib/bladephoria/`.
5. Plik `.env` trzymaj poza katalogiem WWW.

## Endpointy

- `GET /auth/session`
- `GET /auth/public-profile?slug=<slug>`
- `POST /auth/profile-mode`
- `POST /auth/profile-delete`
- `POST /auth/logout`
- `GET /auth/changelog`
- `POST /auth/changelog`
- `GET /auth/server-data`

## Dane i pliki

- `PROFILE_TEMPLATE_FILE` wskazuje wspolny szablon profilu.
- `PROFILE_ROOT` i `PROFILE_SOURCE_ROOT` dostaja wygenerowane profile.
- `PROFILE_DB_FILE` przechowuje profile i ustawienia publiczne.
- `CHANGELOG_DB_FILE` przechowuje wpisy changeloga.

## Uwagi

- Sesje nadal sa trzymane w pamieci procesu. Restart backendu wyloguje aktywne sesje.
- Jesli zmienisz wspolny szablon profilu, backend przy starcie zsynchronizuje wygenerowane strony profilowe z aktualna wersja.
- `GET /auth/server-data` korzysta z cache, a reczny refresh na froncie moze wymusic odswiezenie.
