#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
set -a
. ./auth.env
set +a
exec ./bin/veloren-beta-auth
