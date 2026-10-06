#!/usr/bin/env bash
# Signed + notarized local macOS build. Reads the App Store Connect API key
# settings from ~/.fisilti-signing/notary.env (APPLE_API_KEY, APPLE_API_ISSUER,
# APPLE_API_KEY_PATH); the Developer ID identity comes from
# src-tauri/tauri.macsign.conf.json. See README → "Signed & notarized release".
set -euo pipefail
ENV_FILE="${FISILTI_NOTARY_ENV:-$HOME/.fisilti-signing/notary.env}"
if [[ ! -f "$ENV_FILE" ]]; then
  echo "Missing $ENV_FILE (APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH)" >&2
  exit 1
fi
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a
tauri build --config src-tauri/tauri.macsign.conf.json "$@"
APP="src-tauri/target/release/bundle/macos/Fısıltı.app"
spctl -a -vvv -t exec "$APP"
