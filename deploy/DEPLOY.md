# Deploy na VPS

Cala strona dziala jako jeden program w Ruscie (`backend/`, binarka
`bladephoria-web`): podaje strony, profile, media i API `/auth/*`.
Nginx tylko terminuje TLS i przekazuje caly ruch do tego programu.

## Aktualizacja jednym poleceniem

```bash
deploy/deploy.sh
```

Skrypt wysyla zacommitowany stan repo na VPS, buduje binarke, restartuje
usluge (tylko gdy binarka sie zmienila) i sprawdza, czy serwer odpowiada.
Gdy nowa wersja nie odpowiada, przywraca poprzednia binarke.

Nigdy nie rusza: `oauth/.env`, `oauth/data/`, `media/` ani plikow, ktorych
nie bylo w repo (np. rzeczy wgrane recznie do `assets/`).

Ustawienia (zmienne srodowiskowe): `BPF_DEPLOY_SSH`, `BPF_DEPLOY_KEY`,
`BPF_DEPLOY_DIR` - opis na gorze skryptu.

## Pierwsza instalacja

1. Rust na VPS (`rustup`), kod strony w `/home/ubuntu/Main Files/Website`.
2. `oauth/.env` wedlug [oauth/.env.example](../oauth/.env.example).
3. Usluga systemd: [bladephoria-oauth.service.example](bladephoria-oauth.service.example)

   ```bash
   sudo cp deploy/bladephoria-oauth.service.example /etc/systemd/system/bladephoria-oauth.service
   sudo systemctl daemon-reload
   sudo systemctl enable --now bladephoria-oauth
   ```

4. Nginx: [nginx-site.conf.example](nginx-site.conf.example).

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

Wszystko inne (`oauth/`, `backend/`, `deploy/`, pliki z kropka na poczatku,
np. `.env` czy `.delete-token`) zwraca strone 404.

## Kontrola po wdrozeniu

```bash
curl -I https://bladephoriafonk.xyz/
curl -s https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env   # 404
```
