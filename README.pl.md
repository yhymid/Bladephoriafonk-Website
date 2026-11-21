# Bladephoriafonk Website

[English](README.md) · **Polski**

Strona https://bladephoriafonk.xyz. Jeden mały program w Ruście (`bladephoria-web`, kod
w [`backend/`](backend)) obsługuje:

- strony i pliki (`/`, `/server`, `/administracja`, `/changelog`, `/assets/`, `/profile/`, `/media/`),
- logowanie przez Discord OAuth2,
- profile pod `/profile/<slug>/` z opcjonalnym trybem publicznym,
- changelog,
- publiczne dane serwerów Discord, pobierane przez backend,
- usuwanie opublikowanych mediów tokenem.

Katalog `oauth/` trzyma tylko konfigurację i dane: `.env` i `data/`.

## Dokumentacja

| Dokument | Co opisuje |
|---|---|
| [deploy/DEPLOY.pl.md](deploy/DEPLOY.pl.md) | Wdrożenie na VPS, pierwsza instalacja, każdy plik na serwerze, kontrola |
| [CHANGELOG.md](CHANGELOG.md) | Zmiany w kolejnych wersjach (po angielsku) |
| [oauth/.env.example](oauth/.env.example) | Wszystkie zmienne konfiguracji |

## Warto wiedzieć

- `.env` i pliki w `data/` nigdy nie są publiczne.
- Serwer podaje tylko strony, pliki, media, profile, `/auth/*` i `/health`; wszystko inne
  (`oauth/`, `diagnostics/`, binarka, pliki zaczynające się od kropki) dostaje stronę 404.
- Wszystkie profile używają jednego szablonu `profile/index.html`; zmiana działa od razu dla
  każdego profilu.
- Sesje i publiczne dane serwerów są trzymane w cache, żeby ograniczyć ruch do Discord API.
- Sesje są zapisywane do pliku, więc restart nikogo nie wylogowuje.
- Nieudane logowanie wraca na stronę z `?oauth_error=<kod>`; `site-lang.js` pokazuje komunikat.
- Zaproszenia Discord to listy kodów w kolejności w `backend/src/discord.rs`. Gdy pierwszy kod
  wygaśnie, backend bierze następny, a `/server` sam pokazuje działający link.

## Konfiguracja

Wymagane w `.env`: `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET`, `DISCORD_REDIRECT_URI`
(dla domeny: `https://bladephoriafonk.xyz/auth/discord/callback`) i `BASE_URL`.
Wszystkie zmienne: [oauth/.env.example](oauth/.env.example). Zmienne ustawione w środowisku
mają pierwszeństwo przed plikiem `--env-file`.

Sprawdzenie konfiguracji bez uruchamiania serwera (niczego nie zapisuje, nie wypisuje
sekretów, kod wyjścia 1 przy błędach):

```bash
cd backend
cargo run --release -- --env-file ../oauth/.env --check-config
```

## Uruchomienie lokalne

```bash
cp oauth/.env.example oauth/.env   # uzupełnij dane Discorda i lokalne ścieżki
cd backend
SITE_ROOT=.. PORT=8080 BASE_URL=http://127.0.0.1:8080 cargo run --release -- --env-file ../oauth/.env
```

Potem otwórz http://127.0.0.1:8080. Ten sam program podaje strony i API; osobny serwer
deweloperski nie jest potrzebny.

Testy i kontrola kodu (CI robi to samo przy każdym pushu):

```bash
cd backend
cargo test                                   # testy jednostkowe + e2e (prawdziwa binarka, atrapa Discorda)
BLADEPHORIA_UPDATE_SNAPSHOTS=1 cargo test    # odświeża oczekiwane wyniki w tests/fixtures/discord/
cargo clippy --all-targets -- -D warnings
python3 ../scripts/check-links.py            # linki między plikami Markdown
```

## Diagnostyka

Włączana w `.env`: `BLADEPHORIA_DIAGNOSTICS=1`. Serwer zapisuje wtedy do `diagnostics/`
(obok `index.html`; katalog nie jest publiczny):

- `events-RRRR-MM-DD.jsonl` — każde zdarzenie w osobnej linii JSON, czasy w ms:
  `http` (trasa, status, TTFB, czas całości i wysyłki, rozmiar, kompresja, ile czasu zajął
  Discord / pliki / blokady), `discord` (endpoint, status, czas, nagłówki rate limit), `io`
  (odczyt i zapis plików danych), `lock`, `tick` (spóźnienie zadań okresowych), `resources`
  (co minutę: RAM, CPU, wątki, deskryptory, opóźnienie pętli zdarzeń, sesje), `anomaly`
  (wygasłe zaproszenie, 429, zły JSON, przesunięcie zegara, nieudane logowanie, uszkodzony
  plik danych) i `lifecycle` (start, stop, audyt danych);
- `summary-RRRR-MM-DD.md` — raport po polsku odświeżany co 15 minut i przy zamknięciu:
  p50/p90/p99 każdej trasy, gdzie idzie czas, najwolniejsze żądania, Discord API, pliki,
  zasoby (z pamięcią z ostatnich 7 dni), anomalie i wskazówki.

Doba liczona w UTC. Zapis idzie przez osobny wątek i ograniczoną kolejkę, więc nigdy nie
spowalnia odpowiedzi (przy pełnej kolejce zdarzenie liczy się jako pominięte). Pliki nie
zawierają sekretów: żadnych query stringów, ciasteczek, tokenów, treści żądań ani adresów IP —
tylko etykiety tras (`/profile/:slug`, `/media/*`, `(404)`), czasy, rozmiary i liczniki.

| Zmienna | Domyślnie | Znaczenie |
|---|---|---|
| `BLADEPHORIA_DIAGNOSTICS` | wyłączone | `1` włącza |
| `BLADEPHORIA_DIAGNOSTICS_DIR` | `SITE_ROOT/diagnostics` | katalog plików |
| `BLADEPHORIA_DIAGNOSTICS_RETENTION_DAYS` | `14` | po ilu dniach pliki są usuwane |
| `BLADEPHORIA_DIAGNOSTICS_MAX_MB` | `50` | limit pliku zdarzeń na dzień |
| `BLADEPHORIA_DIAGNOSTICS_SUMMARY_MINUTES` | `15` | co ile minut raport |

## Uszkodzone pliki danych

Gdy `profiles.json` albo `changelog.json` nie jest poprawnym JSON-em, jest pusty albo ma złą
strukturę (np. ucięty do `{}`), serwer:

- nie traktuje go jak pustej bazy i nigdy go nie nadpisuje (logowanie i nowe wpisy zwracają
  błąd zamiast skasować dane),
- zostawia obok kopię `<plik>.corrupt-<hash>` (jedna na daną zawartość; oryginał zostaje
  nietknięty),
- zgłasza anomalię `damaged_data_file` w diagnostyce.

`bladephoria-web --check-config` pokazuje, co jest nie tak, a gdy poprzednia wersja
`<plik>.bak` jest dobra, podaje polecenie `cp`, które ją przywraca. `STRICT_DATA_FILES=0`
przywraca dawne, luźne zachowanie.

## Endpointy

- `GET /health`
- `GET /auth/discord/login?return_to=<ścieżka>`
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
