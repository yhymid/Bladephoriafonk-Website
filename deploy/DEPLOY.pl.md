# Wdrożenie na VPS

[English](DEPLOY.md) · **Polski**

Cała strona działa jako jeden program w Ruście (`backend/`, binarka `bladephoria-web`), który
podaje strony, profile, media i API `/auth/*`. Nginx tylko terminuje TLS i przekazuje cały
ruch do niego (127.0.0.1:3001).

## Aktualizacja jednym poleceniem

```bash
deploy/deploy.sh
```

Skrypt:

1. wysyła zacommitowany stan repo do `~/src/Bladephoriafonk-Website` na VPS i buduje tam
   binarkę (`cargo build --release --locked`; VPS ma inny glibc niż komputer lokalny, więc
   buduje się tylko tam),
2. nowa binarka sprawdza produkcyjną konfigurację i dane (`--check-config`); przy błędzie
   skrypt kończy, zanim cokolwiek na serwerze się zmieni,
3. robi kopię konfiguracji i danych (`oauth/.env`, `oauth/data/`, unit systemd z drop-inami,
   strona nginx) do `~/backups/website-config-<czas>.tar.gz`; trzymana jest tylko najnowsza,
4. kopiuje do katalogu strony tylko pliki potrzebne do działania (strony, `site-lang.js`,
   `profile/index.html`); pliki usunięte z repo znikają z serwera (lista w `.deployed-files`),
5. gdy binarka się zmieniła: zapisuje poprzednią jako `~/backups/bladephoria-web.bin.<commit>`
   (tylko jedną), podmienia `bladephoria-web` w katalogu strony, restartuje usługę i sprawdza
   `/health`. Gdy nowa wersja nie odpowiada, przywraca poprzednią binarkę.

Nigdy nie rusza `oauth/.env`, `oauth/data/`, `media/`, `diagnostics/` ani `assets/` (tam są
ręcznie wgrane pliki modpacka).

Ustawienia (zmienne środowiskowe): `BPF_DEPLOY_SSH`, `BPF_DEPLOY_KEY`, `BPF_DEPLOY_DIR` —
opis na górze skryptu.

## Pierwsza instalacja

1. Rust na VPS (`rustup`), katalog strony `/home/ubuntu/Main Files/Website`.
2. `oauth/.env` według [oauth/.env.example](../oauth/.env.example).
3. Usługa systemd: [bladephoria-oauth.service.example](bladephoria-oauth.service.example)

   ```bash
   sudo cp deploy/bladephoria-oauth.service.example /etc/systemd/system/bladephoria-oauth.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now bladephoria-oauth
   ```

   Sandbox usługi:
   [bladephoria-oauth.service.d/hardening.conf](bladephoria-oauth.service.d/hardening.conf)

   ```bash
   sudo install -D -m 644 deploy/bladephoria-oauth.service.d/hardening.conf \
     /etc/systemd/system/bladephoria-oauth.service.d/hardening.conf
   sudo systemctl daemon-reload && sudo systemctl restart bladephoria-oauth
   systemd-analyze security bladephoria-oauth   # 1.6 OK (bez niego 9.2)
   ```

   Usługa może wtedy zapisywać tylko w `oauth/data/`, `diagnostics/` i `media/`; reszta
   systemu jest tylko do odczytu, a nowe pliki dostają prawa 600 (`UMask=0077`). Powrót:
   usunąć plik, `daemon-reload`, restart. Dane trzymaj prywatnie:
   `chmod 700 oauth oauth/data diagnostics`.

4. Nginx: [nginx-site.conf.example](nginx-site.conf.example).
5. `deploy/deploy.sh` z komputera lokalnego.

## Każdy plik na serwerze

Ścieżki względem katalogu strony, chyba że zaczynają się od `~`.

| Ścieżka | Kto zapisuje | Zawartość | Można usunąć? | Kopia? |
|---|---|---|---|---|
| `bladephoria-web` | `deploy.sh` | binarka serwera | nie (następny deploy ją wstawi) | poprzednia leży w `~/backups` |
| `*.html`, `site-lang.js`, `profile/index.html` | `deploy.sh` | strony i szablon profilu | nie (deploy je odtworzy) | nie, są w gicie |
| `assets/` | ty, ręcznie | ikony modpacka, favicon | nie | tak |
| `media/` | ty, ręcznie | pliki do pobrania i filmy; `<katalog>/.delete-token` pozwala usunąć opublikowaną rzecz | nie | tak |
| `oauth/.env` | ty | konfiguracja z sekretem Discorda (600) | nie | tak (w każdym backupie deployu) |
| `oauth/data/profiles.json` | serwer (logowania, ustawienia profilu) | profile i ich tryb publiczny | nie | tak |
| `oauth/data/changelog.json` | serwer (`POST /auth/changelog`) | wpisy changeloga | nie | tak |
| `oauth/data/sessions.json` | serwer (co 15 s przy zmianie, przy zamknięciu) | logowania z tokenami Discorda (600) | tak, wszyscy zostaną wylogowani | tylko w backupie deployu |
| `oauth/data/*.bak` | serwer (przed każdym zapisem) | poprzednia wersja każdego pliku | tak | nie |
| `oauth/data/*.corrupt-<hash>` | serwer | kopia uszkodzonego pliku | tak, po naprawie pliku | nie |
| `diagnostics/` | serwer | `events-*.jsonl`, `summary-*.md`, usuwane po 14 dniach | tak | nie |
| `.deployed-commit`, `.deployed-files` | `deploy.sh` | wdrożony commit i lista plików | tak, ale wtedy następny deploy nie usunie starych plików | nie |
| `~/src/Bladephoriafonk-Website` | `deploy.sh` | kopia repo do budowania i cache cargo | tak (następny build potrwa dłużej) | nie |
| `~/backups/website-config-*.tar.gz` | `deploy.sh` | `.env`, `oauth/data/`, unit, strona nginx (tylko najnowsza) | zostaw najnowszą | to jest kopia |
| `~/backups/bladephoria-web.bin.<commit>` | `deploy.sh` | poprzednia binarka (jedna) | zostaw na wypadek rollbacku | to jest kopia |

## Co jest publiczne

- `/`, `/server`, `/administracja`, `/changelog`
- `/site-lang.js`, `/assets/`, `/media/`
- `/profile/` i `/profile/<slug>/` (tylko dla istniejących profili)
- `/auth/*`, `/health`

Wszystko inne (`oauth/`, `diagnostics/`, binarka, pliki z kropką na początku, np. `.env` czy
`.delete-token`) dostaje stronę 404.

## Kontrola po wdrożeniu

```bash
curl -I https://bladephoriafonk.xyz/
curl -s https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env   # 404
systemctl show bladephoria-oauth -p NRestarts     # 0
ls "/home/ubuntu/Main Files/Website/diagnostics"  # gdy diagnostyka włączona
```
