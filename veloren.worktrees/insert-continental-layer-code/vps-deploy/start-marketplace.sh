#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
set -a
. ./marketplace.env
set +a
exec ./bin/veloren-marketplace-service
