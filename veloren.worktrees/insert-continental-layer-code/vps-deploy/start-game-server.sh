#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
set -a
. ./.env
set +a
export VELOREN_AUTH_MODE=remote
export VELOREN_AUTH_SERVER_URL="${VELOREN_AUTH_SERVER_URL:-http://127.0.0.1:19253}"
exec ./bin/veloren-server-cli --non-interactive
