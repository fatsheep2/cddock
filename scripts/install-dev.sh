#!/usr/bin/env bash
set -euo pipefail

export CDDOCK_VERSION="${CDDOCK_VERSION:-dev-snapshot}"

install_url="https://raw.githubusercontent.com/fatsheep2/cddock/dev/scripts/install.sh?$(date +%s)"
exec bash -c "$(curl -fsSL -H 'Cache-Control: no-cache' "$install_url")"
