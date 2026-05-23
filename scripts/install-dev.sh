#!/usr/bin/env bash
set -euo pipefail

export CDDOCK_VERSION="${CDDOCK_VERSION:-dev-snapshot}"

exec bash -c "$(curl -fsSL https://raw.githubusercontent.com/fatsheep2/cddock/dev/scripts/install.sh)"
