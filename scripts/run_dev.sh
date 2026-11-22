#!/usr/bin/env bash
# Run Wysebee Agent from source, against a Wysebee Cloud of your choice, for development.
#
#   ./scripts/run_dev.sh                    # Wysebee Cloud (https://app.wysebee.com)
#   ./scripts/run_dev.sh --url <address>    # any other cloud (https://, or http:// to this computer)
#   ./scripts/run_dev.sh --fresh            # forget this cloud's dev state first (unpaired, no folders)
#   ./scripts/run_dev.sh --release          # a release build
#   ./scripts/run_dev.sh --headless "Dev agent"   # no window: the headless example
#
# Pair with a Wysebee that talks to the same cloud.
#
# Each cloud keeps its own state folder, apart from an installed copy:
#   ~/Library/Application Support/Wysebee Agent Dev/<prod|custom>/
# (WYSEBEE_AGENT_STATE_DIR overrides it). One instance runs per state folder, so this can
# run beside the installed app. The log goes to this terminal.
#
# The Keychain item follows the state folder: the installed app's is account link-secret,
# each folder here has its own (link-secret <hash of the folder>), so pairing here leaves
# the installed copy paired. macOS may ask before a development build reads its item.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"   # Homebrew's rustup is keg-only

ENV_NAME="prod"
URL="https://app.wysebee.com"
FRESH=0
PROFILE_ARGS=()
HEADLESS=""

usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --url) URL="${2:?--url needs an address}"; ENV_NAME="custom"; shift 2 ;;
    --fresh) FRESH=1; shift ;;
    --release) PROFILE_ARGS=(--release); shift ;;
    --headless) HEADLESS="${2:?--headless needs an agent name}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

STATE_DIR="${WYSEBEE_AGENT_STATE_DIR:-$HOME/Library/Application Support/Wysebee Agent Dev/$ENV_NAME}"

if [[ "$FRESH" == 1 && -d "$STATE_DIR" ]]; then
  echo "Removing dev state: $STATE_DIR"
  rm -rf -- "$STATE_DIR"
fi
mkdir -p "$STATE_DIR"

export WYSEBEE_CLOUD_URL="$URL"
export WYSEBEE_AGENT_STATE_DIR="$STATE_DIR"

echo "Cloud:  $WYSEBEE_CLOUD_URL (the default; an address saved in the Server tab wins)"
echo "State:  $WYSEBEE_AGENT_STATE_DIR"

cd "$REPO_ROOT"
if [[ -n "$HEADLESS" ]]; then
  exec cargo run ${PROFILE_ARGS[@]+"${PROFILE_ARGS[@]}"} -p agent-core --example headless -- "$STATE_DIR" "$HEADLESS"
fi
exec cargo run ${PROFILE_ARGS[@]+"${PROFILE_ARGS[@]}"} -p agent-app
