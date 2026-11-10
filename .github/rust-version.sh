#!/usr/bin/env bash
# Imprime `version=<X.Y>` con la versión de Rust de la imagen (`FROM rust:X.Y-...`), para que el
# CI use la misma que `docker compose`. Falla si no hay ninguna o si las etapas no coinciden.
#
#   bash .github/rust-version.sh [Dockerfile]
set -euo pipefail

archivo="${1:-Dockerfile}"
versiones=$(sed -nE 's/^FROM rust:([0-9]+\.[0-9]+(\.[0-9]+)?)-.*/\1/p' "$archivo" | sort -u)

if [ -z "$versiones" ]; then
  echo "::error::$archivo no tiene ninguna línea 'FROM rust:X.Y-...'" >&2
  exit 1
fi
if [ "$(printf '%s\n' "$versiones" | wc -l)" -ne 1 ]; then
  echo "::error::$archivo usa versiones distintas de Rust: $(echo $versiones)" >&2
  exit 1
fi
echo "version=$versiones"
