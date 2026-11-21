#!/usr/bin/env bash
# Opens the read-only admin panel of the website through an SSH tunnel.
#
#   scripts/admin-tunnel.sh [user@host] [local_port]
#
# Runs in the foreground; Ctrl+C closes the tunnel and nothing keeps running.
# The panel listens on 127.0.0.1 of the VPS only (ADMIN_PANEL_PORT, 3002 on
# production) and is never exposed by nginx.
set -euo pipefail

TARGET="${1:-${BPF_DEPLOY_SSH:-ubuntu@57.128.222.44}}"
LOCAL_PORT="${2:-3002}"
REMOTE_PORT="${BPF_PANEL_PORT:-3002}"
KEY="${BPF_DEPLOY_KEY:-$HOME/.ssh/operator_vps}"

SSH=(ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=30)
if [ -f "$KEY" ]; then SSH+=(-i "$KEY" -o IdentitiesOnly=yes); fi

echo "Admin panel: http://127.0.0.1:$LOCAL_PORT  (Ctrl+C to close)"
exec "${SSH[@]}" -L "127.0.0.1:$LOCAL_PORT:127.0.0.1:$REMOTE_PORT" "$TARGET"
