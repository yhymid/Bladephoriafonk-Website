# Deploy na VPS

Cala strona dziala jako jeden program w Ruscie (`backend/`, binarka
`bladephoria-web`): podaje strony, profile, media i API `/auth/*`.
Nginx tylko terminuje TLS i przekazuje caly ruch do tego programu.

## Aktualizacja jednym poleceniem

```bash
deploy/deploy.sh
```

Skrypt:

1. wysyla zacommitowany stan repo do `~/src/Bladephoriafonk-Website` na VPS
   i buduje tam binarke (`cargo build --release --locked`; VPS ma inny glibc
   niz komputer lokalny, wiec buduje sie tylko tam),
2. sprawdza nowa binarka produkcyjny config i dane (`--check-config`); przy
   bledzie konczy, zanim cokolwiek na serwerze sie zmieni,
3. robi kopie konfiguracji i danych (`oauth/.env`, `oauth/data/`, unit systemd,
   strona nginx) do `~/backups/website-config-<czas>.tar.gz` - trzymana jest
   tylko najnowsza,
4. kopiuje do katalogu strony tylko pliki potrzebne do dzialania (strony,
   `site-lang.js`, `profile/index.html`); pliki usuniete z repo znikaja
   z serwera (lista w `.deployed-files`),
5. gdy binarka sie zmienila: zapisuje poprzednia jako
   `~/backups/bladephoria-web.bin.<commit>` (tylko jedna), podmienia
   `bladephoria-web` w katalogu strony, restartuje usluge i sprawdza
   `/health`. Gdy nowa wersja nie odpowiada, przywraca poprzednia binarke.

Nigdy nie rusza: `oauth/.env`, `oauth/data/`, `media/`, `diagnostics/` ani
`assets/` (tam sa recznie wgrane pliki modpacka).

Katalog strony na VPS zawiera tylko to, czego potrzebuje usluga:
`bladephoria-web`, `*.html`, `site-lang.js`, `profile/`, `assets/`, `media/`,
`oauth/.env`, `oauth/data/`, `diagnostics/`, `.deployed-commit`,
`.deployed-files`.

Ustawienia (zmienne srodowiskowe): `BPF_DEPLOY_SSH`, `BPF_DEPLOY_KEY`,
`BPF_DEPLOY_DIR` - opis na gorze skryptu.

## Pierwsza instalacja

1. Rust na VPS (`rustup`), katalog strony `/home/ubuntu/Main Files/Website`.
2. `oauth/.env` wedlug [oauth/.env.example](../oauth/.env.example).
3. Usluga systemd: [bladephoria-oauth.service.example](bladephoria-oauth.service.example)

   ```bash
   sudo cp deploy/bladephoria-oauth.service.example /etc/systemd/system/bladephoria-oauth.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now bladephoria-oauth
   ```

   Ograniczenia uslugi (sandbox systemd):
   [bladephoria-oauth.service.d/hardening.conf](bladephoria-oauth.service.d/hardening.conf)

   ```bash
   sudo install -D -m 644 deploy/bladephoria-oauth.service.d/hardening.conf \
     /etc/systemd/system/bladephoria-oauth.service.d/hardening.conf
   sudo systemctl daemon-reload && sudo systemctl restart bladephoria-oauth
   systemd-analyze security bladephoria-oauth   # 1.6 OK (bez niego 9.2)
   ```

   Usluga moze wtedy zapisywac tylko w `oauth/data/`, `diagnostics/` i
   `media/`; reszta systemu jest tylko do odczytu, nowe pliki dostaja prawa
   600 (`UMask=0077`). Powrot: usunac plik, `daemon-reload`, restart.
   Dane trzymaj z prawami 700/600: `chmod 700 oauth oauth/data diagnostics`.

4. Nginx: [nginx-site.conf.example](nginx-site.conf.example).
5. `deploy/deploy.sh` z komputera lokalnego.

## Dane

Wszystko w `oauth/data/` (albo tam, gdzie wskazuje `.env`):

- `profiles.json` - profile i ustawienia publiczne,
- `changelog.json` - wpisy changeloga,
- `sessions.json` - sesje logowania (tokeny Discorda, prawa 0600). Dzieki
  temu restart uslugi nikogo nie wylogowuje.

## Co jest publiczne

- `/`, `/server`, `/administracja`, `/changelog`
- `/site-lang.js`, `/assets/`, `/media/`
- `/profile/` i `/profile/<slug>/` (tylko dla istniejacych profili)
- `/auth/*`, `/health`

Wszystko inne (`oauth/`, `diagnostics/`, binarka, pliki z kropka na poczatku,
np. `.env` czy `.delete-token`) zwraca strone 404.

## Kontrola po wdrozeniu

```bash
curl -I https://bladephoriafonk.xyz/
curl -s https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env   # 404
systemctl show bladephoria-oauth -p NRestarts     # 0
ls "/home/ubuntu/Main Files/Website/diagnostics"  # gdy diagnostyka wlaczona
```
