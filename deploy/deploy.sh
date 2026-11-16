#!/usr/bin/env bash
# Deploys the committed HEAD of this repository to the VPS:
# upload -> build -> restart -> health check, with rollback of the binary
# when the new one does not answer.
#
#   deploy/deploy.sh
#
# Settings (environment variables):
#   BPF_DEPLOY_SSH   ssh target            (default: ubuntu@bladephoriafonk.xyz)
#   BPF_DEPLOY_KEY   ssh private key       (default: ~/.ssh/id_ed25519_vps if present)
#   BPF_DEPLOY_DIR   site directory on VPS (default: /home/ubuntu/Main Files/Website)
#
# Never touched on the server: oauth/.env, oauth/data/, media/ and any file
# that was not part of this repository (uploads in assets/, for example).
set -euo pipefail

TARGET="${BPF_DEPLOY_SSH:-ubuntu@bladephoriafonk.xyz}"
REMOTE_DIR="${BPF_DEPLOY_DIR:-/home/ubuntu/Main Files/Website}"
KEY="${BPF_DEPLOY_KEY:-$HOME/.ssh/id_ed25519_vps}"

SSH=(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new)
if [ -f "$KEY" ]; then SSH+=(-i "$KEY" -o IdentitiesOnly=yes); fi

cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
if [ -n "$(git status --porcelain)" ]; then
  echo "!! Uncommitted changes are NOT deployed (only the committed HEAD is)." >&2
fi
NEW_COMMIT=$(git rev-parse HEAD)
echo "== deploying $(git log -1 --format='%h %s')"

# Files deleted from the repository since the last deploy are deleted on the
# server too. Only paths git tracked are ever removed.
OLD_COMMIT=$("${SSH[@]}" "$TARGET" "cat '$REMOTE_DIR/.deployed-commit' 2>/dev/null" || true)
if [ -n "$OLD_COMMIT" ] && git cat-file -e "$OLD_COMMIT^{commit}" 2>/dev/null; then
  git diff --name-only --diff-filter=D -z "$OLD_COMMIT" HEAD -- . ':(exclude)oauth/data' \
    | "${SSH[@]}" "$TARGET" "cd '$REMOTE_DIR' && xargs -0 -r rm -f --"
fi

echo "== upload"
git archive HEAD -- . ':(exclude)oauth/data' | "${SSH[@]}" "$TARGET" "tar -x -C '$REMOTE_DIR'"

echo "== build, restart, check"
"${SSH[@]}" "$TARGET" "REMOTE_DIR='$REMOTE_DIR' NEW_COMMIT='$NEW_COMMIT' bash -s" <<'REMOTE'
set -euo pipefail
BIN="$REMOTE_DIR/backend/target/release/bladephoria-web"
PREVIOUS="$HOME/.cache/bladephoria-web.previous"
mkdir -p "$HOME/.cache"

before=""
if [ -f "$BIN" ]; then
  before=$(sha256sum "$BIN" | cut -d' ' -f1)
  cp -p "$BIN" "$PREVIOUS"
fi

cd "$REMOTE_DIR/backend"
nice -n 15 "$HOME/.cargo/bin/cargo" build --release -j 3 2>&1 | tail -n 3
after=$(sha256sum "$BIN" | cut -d' ' -f1)

healthy() {
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -fsS http://127.0.0.1:3001/health > /dev/null 2>&1; then return 0; fi
    sleep 1
  done
  return 1
}

if [ "$before" = "$after" ]; then
  echo "binary unchanged - no restart needed (pages are read from disk)"
else
  sudo systemctl restart bladephoria-oauth
  if ! healthy; then
    echo "!! new server does not answer - restoring the previous binary" >&2
    sudo journalctl -u bladephoria-oauth -n 20 --no-pager >&2 || true
    if [ -f "$PREVIOUS" ]; then
      cp -p "$PREVIOUS" "$BIN"
      sudo systemctl restart bladephoria-oauth
    fi
    exit 1
  fi
  echo "restarted, healthy"
fi

healthy || { echo "!! server does not answer" >&2; exit 1; }
echo "$NEW_COMMIT" > "$REMOTE_DIR/.deployed-commit"
REMOTE

echo "== done: https://bladephoriafonk.xyz"
