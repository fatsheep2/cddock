#!/usr/bin/env bash
set -euo pipefail

target="${1:?usage: scripts/package-release.sh <rust-target>}"
version="${2:-dev}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="${root}/dist"
name="cddock-${version}-${target}"
stage="${dist}/${name}"

mkdir -p "${dist}"
rm -rf "${stage}" "${stage}.tar.gz"
mkdir -p "${stage}"

cp "${root}/target/${target}/release/cddock" "${stage}/cddock"
cp "${root}/scripts/install.sh" "${stage}/install.sh"
cp "${root}/README.md" "${stage}/README.md"
chmod +x "${stage}/cddock" "${stage}/install.sh"

tar -C "${dist}" -czf "${stage}.tar.gz" "${name}"
printf '%s\n' "${stage}.tar.gz"
