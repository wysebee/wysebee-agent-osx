#!/usr/bin/env bash
# Everything must pass, every time: the tests, clippy with no warning, and 100% of
# agent-core's lines covered. Run it before every commit.
#
#   ./scripts/check.sh
#
# The one file left out of the coverage rule is crates/agent-core/src/secrets/keychain.rs,
# the real Keychain, which no test may touch (CLAUDE.md). Nothing else is excluded: cover
# new code with a real test, never with an exclusion.
#
# agent-app (the Tauri shell) is built and linted here but not measured: it is the window
# and the real Options, and holds no logic the core's tests do not already cover.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"   # Homebrew's rustup is keg-only
cd "$REPO_ROOT"

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov is needed: cargo install cargo-llvm-cov" >&2
  exit 1
fi

echo "== clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "== tests"
cargo test --workspace

echo "== coverage (agent-core, 100% of lines)"
cargo llvm-cov -p agent-core --show-missing-lines \
  --ignore-filename-regex 'agent-core/src/secrets/keychain\.rs$' \
  --fail-uncovered-lines 0
# (--fail-uncovered-lines, not --fail-under-lines: the TOTAL row counts each test binary's
# own copy of a function, so it stays under 100% even when every line is run.)

echo "All green."
