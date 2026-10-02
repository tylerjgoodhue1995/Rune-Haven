#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
set -a
. ./auth.env
set +a
export BETA_ALPHA_ACCESS_PATH="$PWD/alpha-access.json"
exec ./bin/veloren-beta-auth
