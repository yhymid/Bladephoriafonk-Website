# Bladephoriafonk Website

Strona https://bladephoriafonk.xyz. Backend jest napisany w Ruscie i lezy w
[`backend/`](backend). Jeden
program (`bladephoria-web`) obsluguje:

- strony i assety (`/`, `/server`, `/administracja`, `/changelog`, `/assets/`, `/profile/`, `/media/`),
- logowanie Discord OAuth2,
- profile `/profile/<slug>/` i tryb publiczny profilu,
- changelog,
- publiczne dane serwerow Discord pobierane po stronie backendu,
- usuwanie opublikowanych mediow po tokenie.

Katalog `oauth/` trzyma tylko konfiguracje i dane: `.env` i `data/`.

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

Pelna lista zmiennych: [oauth/.env.example](oauth/.env.example). Zmienne ustawione
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

Sprawdzenie konfiguracji bez uruchamiania serwera (niczego nie zapisuje,
sekretow nie wypisuje, kod wyjscia 1 przy bledach):

```bash
cd backend
cargo run --release -- --env-file ../oauth/.env --check-config
```

Testy i kontrola kodu:

```bash
cd backend
cargo test            # testy jednostkowe + e2e (prawdziwa binarka i atrapa Discorda)
BLADEPHORIA_UPDATE_SNAPSHOTS=1 cargo test   # odswieza oczekiwane wyniki w tests/fixtures/discord/
cargo clippy --all-targets -- -D warnings
```

## Wdrozenie na VPS

Opis: [deploy/DEPLOY.md](deploy/DEPLOY.md). Przyklady:

- [deploy/nginx-site.conf.example](deploy/nginx-site.conf.example)
- [deploy/bladephoria-oauth.service.example](deploy/bladephoria-oauth.service.example)

## Diagnostyka

Wlaczana w `.env`: `BLADEPHORIA_DIAGNOSTICS=1`. Serwer zapisuje wtedy do
`diagnostics/` (obok `index.html`; katalog nie jest publiczny):

- `events-YYYY-MM-DD.jsonl` - kazde zdarzenie w osobnej linii JSON, czas w ms:
  `http` (trasa, status, TTFB, czas calosci i wysylki, rozmiar, kompresja,
  ile czasu zajal Discord / pliki / blokady), `discord` (endpoint, status,
  czas, naglowki rate limit), `io` (odczyt/zapis plikow danych), `lock`,
  `tick` (spoznienie zadan okresowych), `resources` (co minute: RAM, CPU,
  watki, deskryptory, opoznienie petli zdarzen, liczba sesji), `anomaly`
  (np. wygasle zaproszenie, 429, zly JSON, przesuniecie zegara, nieudane
  logowanie) i `lifecycle` (start, stop, audyt danych);
- `summary-YYYY-MM-DD.md` - raport po polsku odswiezany co 15 minut i przy
  zamknieciu: p50/p90/p99 dla kazdej trasy, gdzie idzie czas, najwolniejsze
  zadania, Discord API, pliki, zasoby, anomalie i automatyczne wskazowki.

Doba liczona w UTC. Zapis idzie przez osobny watek i ograniczona kolejke, wiec
nigdy nie spowalnia odpowiedzi (przy przepelnieniu zdarzenie jest liczone jako
pominiete). Pliki nie zawieraja sekretow: zadnych query stringow, ciasteczek,
tokenow, tresci zadan ani adresow IP - tylko etykiety tras (`/profile/:slug`,
`/media/*`, `(404)`), czasy, rozmiary i liczniki.

| Zmienna | Domyslnie | Znaczenie |
|---|---|---|
| `BLADEPHORIA_DIAGNOSTICS` | wylaczone | `1` wlacza |
| `BLADEPHORIA_DIAGNOSTICS_DIR` | `SITE_ROOT/diagnostics` | katalog plikow |
| `BLADEPHORIA_DIAGNOSTICS_RETENTION_DAYS` | `14` | po ilu dniach pliki sa usuwane |
| `BLADEPHORIA_DIAGNOSTICS_MAX_MB` | `50` | limit pliku zdarzen na dzien |
| `BLADEPHORIA_DIAGNOSTICS_SUMMARY_MINUTES` | `15` | co ile minut raport |

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

## Uszkodzone pliki danych

Gdy `profiles.json` albo `changelog.json` nie jest poprawnym JSON-em, jest
pusty albo ma zla strukture (np. uciety do `{}`), serwer:

- nie traktuje go jak pustej bazy i nigdy go nie nadpisuje (logowanie
  i dodawanie wpisow zwracaja blad zamiast skasowac dane),
- zostawia obok kopie `<plik>.corrupt-<hash>` (jedna na dana zawartosc;
  oryginal zostaje nietkniety),
- zglasza anomalie `damaged_data_file` w diagnostyce.

`bladephoria-web --check-config` pokazuje, co jest nie tak, i gdy poprzednia
wersja `<plik>.bak` jest dobra, podaje polecenie `cp` do jej przywrocenia.
`STRICT_DATA_FILES=0` przywraca dawne, luzne zachowanie.

## Uwagi

- Sesje sa zapisywane do pliku, wiec restart backendu nie wylogowuje uzytkownikow.
- Nieudane logowanie wraca na strone z `?oauth_error=<kod>`; `site-lang.js` pokazuje wtedy komunikat.
- Zaproszenia Discord sa w `backend/src/discord.rs` (lista kodow w kolejnosci). Gdy pierwszy kod wygasnie, backend bierze nastepny, a strona `/server` sama pokazuje dzialajacy link.
- `GET /auth/server-data` korzysta z cache, a reczny refresh na froncie moze wymusic odswiezenie.
