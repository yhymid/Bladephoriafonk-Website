# Deploy na VPS

## 1. Publiczny katalog WWW

Trzymaj stronę w katalogu podobnym do:

- `/home/ubuntu/Main Files/website`

Publiczne pliki:

- `index.html`
- `server.html`
- `administracja.html`
- `changelog.html`
- `profile/`
- `assets/`
- `site-lang.js`

Nie wystawiaj:

- `oauth/`
- `src/`
- `.env`

## 2. Dane poza katalogiem publicznym

Przykladowe sciezki:

- `PROFILE_DB_FILE=/var/lib/bladephoria/profiles.json`
- `CHANGELOG_DB_FILE=/var/lib/bladephoria/changelog.json`

## 3. Backend OAuth

Skopiuj przyklad uslugi:

- [bladephoria-oauth.service.example](/run/media/anon/Nowy/Marmak/git.marmak.net.pl/Website/deploy/bladephoria-oauth.service.example)

## 4. Nginx

Skopiuj i dostosuj:

- [nginx-site.conf.example](/run/media/anon/Nowy/Marmak/git.marmak.net.pl/Website/deploy/nginx-site.conf.example)

## 5. Kontrola po wdrozeniu

Sprawdz:

```bash
curl -I https://bladephoriafonk.xyz/
curl -I https://bladephoriafonk.xyz/server
curl -I https://bladephoriafonk.xyz/auth/session
curl -I https://bladephoriafonk.xyz/oauth/.env
```

Ostatnie zapytanie powinno zwrocic `404`.
