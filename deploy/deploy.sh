#!/usr/bin/env bash
# Deploys the committed HEAD of this repository to the VPS:
# binary -> upload -> check config -> backup -> install -> restart -> health
# check, with rollback of the binary when the new one does not answer.
#
# The binary comes from CI by default: the release build of the CI run for
# exactly this commit (artifact `bladephoria-web-linux`, checked by SHA256
# and glibc version), so the VPS does not compile next to the game servers.
# BPF_BUILD=vps builds on the VPS instead, as before.
#
#   deploy/deploy.sh
#
# Layout on the VPS:
#   ~/src/Bladephoriafonk-Website   copy of the repo (+ backend/target when
#                                   built on the VPS)
#   SITE_DIR                        only what the server needs: the binary
#                                   `bladephoria-web`, pages, assets/, media/,
#                                   profile/, oauth/.env + oauth/data/,
#                                   diagnostics/
#   ~/backups/website-config-*.tar.gz      .env, oauth/data/, systemd unit,
#                                          nginx site (only the newest one)
#   ~/backups/bladephoria-web.bin.<commit> the previous binary (only one)
#
# Settings (environment variables):
#   BPF_DEPLOY_SSH   ssh target            (default: ubuntu@57.128.222.44)
#   BPF_DEPLOY_KEY   ssh private key       (default: ~/.ssh/operator_vps)
#   BPF_DEPLOY_DIR   site directory on VPS (default: /home/ubuntu/Main Files/Website)
#   BPF_BUILD        ci (default) or vps
#
# Never touched on the server: oauth/.env, oauth/data/, media/, diagnostics/
# and anything in assets/ (modpack uploads live there).
set -euo pipefail

TARGET="${BPF_DEPLOY_SSH:-ubuntu@57.128.222.44}"
SITE_DIR="${BPF_DEPLOY_DIR:-/home/ubuntu/Main Files/Website}"
KEY="${BPF_DEPLOY_KEY:-$HOME/.ssh/operator_vps}"
BUILD_MODE="${BPF_BUILD:-ci}"
case "$BUILD_MODE" in ci|vps) ;; *) echo "BPF_BUILD must be ci or vps" >&2; exit 1 ;; esac

SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
if [ -f "$KEY" ]; then SSH+=(-i "$KEY" -o IdentitiesOnly=yes); fi

cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
if [ -n "$(git status --porcelain)" ]; then
  echo "!! Uncommitted changes are NOT deployed (only the committed HEAD is)." >&2
fi
NEW_COMMIT=$(git rev-parse HEAD)
echo "== deploying $(git log -1 --format='%h %s') (binary: $BUILD_MODE)"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
if [ "$BUILD_MODE" = ci ]; then
  repo=$(git remote get-url origin | sed -E 's#^(https://github.com/|git@github.com:)##; s#\.git$##')
  git fetch -q origin
  if [ -z "$(git branch -r --contains "$NEW_COMMIT")" ]; then
    echo "!! $NEW_COMMIT is not pushed; CI builds only pushed commits (or use BPF_BUILD=vps)" >&2
    exit 1
  fi
  echo "== waiting for CI of ${NEW_COMMIT:0:7} ($repo)"
  run=""
  for _ in $(seq 30); do
    run=$(gh run list --repo "$repo" --commit "$NEW_COMMIT" --workflow ci.yml \
      --json databaseId --jq '.[0].databaseId // empty')
    [ -n "$run" ] && break
    sleep 10
  done
  [ -n "$run" ] || { echo "!! no CI run for $NEW_COMMIT" >&2; exit 1; }
  if ! gh run watch "$run" --repo "$repo" --exit-status > /dev/null; then
    echo "!! CI run $run failed - nothing deployed" >&2
    exit 1
  fi
  gh run download "$run" --repo "$repo" -n bladephoria-web-linux -D "$WORK"
  (cd "$WORK" && sha256sum --quiet -c SHA256SUMS)
  need=$(cat "$WORK/GLIBC_REQUIRED")
  have=$("${SSH[@]}" "$TARGET" "ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+\$'")
  if [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -1)" != "$need" ]; then
    echo "!! the binary needs glibc $need, the VPS has $have (use BPF_BUILD=vps)" >&2
    exit 1
  fi
  echo "binary from CI run $run: sha256 $(cut -c1-16 "$WORK/SHA256SUMS" | head -1)..., glibc $need <= $have"
fi

echo "== upload"
# The build copy mirrors HEAD exactly; only the cargo cache survives.
"${SSH[@]}" "$TARGET" 'set -e
  BUILD="$HOME/src/Bladephoriafonk-Website"
  mkdir -p "$BUILD/backend"
  find "$BUILD" -mindepth 1 -maxdepth 1 ! -name backend -exec rm -rf {} +
  find "$BUILD/backend" -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +'
git archive HEAD | "${SSH[@]}" "$TARGET" 'tar -x -C "$HOME/src/Bladephoriafonk-Website"'
if [ "$BUILD_MODE" = ci ]; then
  tar -C "$WORK" -cf - bladephoria-web SHA256SUMS GLIBC_REQUIRED \
    | "${SSH[@]}" "$TARGET" 'mkdir -p "$HOME/src/Bladephoriafonk-Website/ci" && tar -x -C "$HOME/src/Bladephoriafonk-Website/ci"'
fi

echo "== build, install, restart, check"
"${SSH[@]}" "$TARGET" "SITE_DIR='$SITE_DIR' NEW_COMMIT='$NEW_COMMIT' BUILD_MODE='$BUILD_MODE' bash -s" <<'REMOTE'
set -euo pipefail
BUILD="$HOME/src/Bladephoriafonk-Website"
BACKUPS="$HOME/backups"
BIN="$SITE_DIR/bladephoria-web"
UNIT=/etc/systemd/system/bladephoria-oauth.service
UNIT_DROPINS=/etc/systemd/system/bladephoria-oauth.service.d
NGINX_SITE=/etc/nginx/sites-available/bladephoriafonk.xyz
mkdir -p "$BACKUPS"

if [ "$BUILD_MODE" = ci ]; then
  # Checked again after the transfer.
  (cd "$BUILD/ci" && sha256sum --quiet -c SHA256SUMS)
  NEW_BIN="$BUILD/ci/bladephoria-web"
  chmod 755 "$NEW_BIN"
else
  cd "$BUILD/backend"
  nice -n 15 "$HOME/.cargo/bin/cargo" build --release --locked -j 3 2>&1 | tail -n 3
  NEW_BIN="$BUILD/backend/target/release/bladephoria-web"
fi

# The new binary validates the production config and data before anything
# on the server changes (it starts nothing and writes nothing).
if check=$(cd "$SITE_DIR" && "$NEW_BIN" --check-config --env-file "$SITE_DIR/oauth/.env" 2>&1); then
  printf '%s\n' "$check" | grep -v '^ok ' || true
else
  printf '%s\n' "$check" | grep -v '^ok ' >&2 || true
  echo "!! the new version rejects the configuration - nothing was changed" >&2
  exit 1
fi

# Configuration and data backup before anything changes; only the newest is kept.
stamp=$(date -u +%Y%m%d-%H%M%S)
config_backup="$BACKUPS/website-config-$stamp.tar.gz"
dropins=(); [ -d "$UNIT_DROPINS" ] && dropins=("$UNIT_DROPINS")
sudo tar -czf "$config_backup" -P "$SITE_DIR/oauth/.env" "$SITE_DIR/oauth/data" "$UNIT" "${dropins[@]}" "$NGINX_SITE"
sudo chown "$(id -u):$(id -g)" "$config_backup"
chmod 600 "$config_backup"
find "$BACKUPS" -maxdepth 1 -name 'website-config-*.tar.gz' ! -path "$config_backup" -delete
echo "config backup: $config_backup"

# Runtime files, by allowlist: the pages and scripts at the top level, the
# profile template and any tracked assets. Nothing else from the repository
# (CI files, scripts, sources, docs) belongs in the site directory.
cd "$BUILD"
git_files=$( { find . -maxdepth 1 -type f \( -name '*.html' -o -name '*.js' \);
  for dir in profile assets; do if [ -d "$dir" ]; then find "./$dir" -type f; fi; done; } \
  | sed 's|^\./||' | LC_ALL=C sort)
manifest="$SITE_DIR/.deployed-files"
if [ -f "$manifest" ]; then
  # Files deployed last time but gone from the repo now; never user data.
  # Byte order on both sides (the manifest is written in the same order).
  LC_ALL=C comm -23 <(LC_ALL=C sort "$manifest") <(printf '%s\n' "$git_files") \
    | { grep -Ev '^(assets|media|oauth|diagnostics)/' || true; } \
    | while IFS= read -r stale; do
        if [ -n "$stale" ]; then
          rm -f -- "$SITE_DIR/$stale" && echo "removed $stale"
          # and the directories it leaves empty
          dir=$(dirname -- "$stale")
          if [ "$dir" != . ]; then (cd "$SITE_DIR" && rmdir -p --ignore-fail-on-non-empty -- "$dir"); fi
        fi
      done
fi
printf '%s\n' "$git_files" | tar -cf - -T - | tar -xf - -C "$SITE_DIR"
printf '%s\n' "$git_files" > "$manifest"

healthy() {
  for _ in $(seq 15); do
    if curl -fsS http://127.0.0.1:3001/health > /dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

old_commit=$(cat "$SITE_DIR/.deployed-commit" 2>/dev/null || echo unknown)
if [ -f "$BIN" ] && cmp -s "$BIN" "$NEW_BIN"; then
  echo "$NEW_COMMIT" > "$SITE_DIR/.deployed-commit"
  echo "binary unchanged - no restart needed (pages are read from disk)"
  healthy || { echo "!! server does not answer" >&2; exit 1; }
  exit 0
fi

previous=""
if [ -f "$BIN" ]; then
  previous="$BACKUPS/bladephoria-web.bin.${old_commit:0:12}"
  cp -p "$BIN" "$previous.tmp" && mv "$previous.tmp" "$previous"
  find "$BACKUPS" -maxdepth 1 -name 'bladephoria-web.bin.*' ! -path "$previous" -delete
  echo "previous binary: $previous"
fi
cp "$NEW_BIN" "$BIN.new" && mv "$BIN.new" "$BIN"
echo "$NEW_COMMIT" > "$SITE_DIR/.deployed-commit"

sudo systemctl restart bladephoria-oauth
if ! healthy; then
  echo "!! new server does not answer - restoring the previous binary" >&2
  sudo journalctl -u bladephoria-oauth -n 20 --no-pager >&2 || true
  if [ -n "$previous" ]; then
    cp -p "$previous" "$BIN.new" && mv "$BIN.new" "$BIN"
    echo "$old_commit" > "$SITE_DIR/.deployed-commit"
    sudo systemctl restart bladephoria-oauth
  fi
  exit 1
fi
echo "restarted, healthy"
REMOTE

echo "== done: https://bladephoriafonk.xyz"
