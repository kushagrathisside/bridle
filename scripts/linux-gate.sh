#!/usr/bin/env bash
# Linux gate for Bridle: the format check, clippy with warnings as errors, the full test suite, the release
# build, and a smoke test of the release binary in a temporary repository. Run it from anywhere:
#
#   scripts/linux-gate.sh
#
# It stops at the first failing step and exits with that step's code. Proxy variables are cleared for the tests
# only: the Jev tests use a loopback server, and a proxy answers that with 403.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

step() {
    local name="$1"
    shift
    echo
    echo "== $name"
    "$@"
    local code=$?
    if [ "$code" -ne 0 ]; then
        echo "FAILED ($code): $name"
        exit "$code"
    fi
    echo "ok: $name"
}

step "format check" cargo fmt --all -- --check
step "clippy, warnings as errors" cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests" env -u HTTP_PROXY -u HTTPS_PROXY -u http_proxy -u https_proxy -u NO_PROXY -u no_proxy \
    cargo test --workspace --locked --no-fail-fast
step "release build" cargo build --release -p harness-cli --locked

AGENT="$PWD/target/release/agent"
SMOKE="$(mktemp -d)"
trap 'rm -rf "$SMOKE"' EXIT
mkdir -p "$SMOKE/repo" "$SMOKE/home" "$SMOKE/models" "$SMOKE/runtimes"
git -C "$SMOKE/repo" init -q
export LOCAL_AGENT_HOME="$SMOKE/home" LOCAL_AGENT_MODELS_DIR="$SMOKE/models" LOCAL_AGENT_RUNTIMES_DIR="$SMOKE/runtimes"

step "smoke: version" "$AGENT" --version
step "smoke: init" "$AGENT" --workspace "$SMOKE/repo" init --profile local-only
step "smoke: config validate" "$AGENT" --workspace "$SMOKE/repo" config validate

echo "== smoke: repository memory reaches the system prompt"
printf 'Run the unit tests before you finish.\n' > "$SMOKE/repo/AGENTS.md"
answer="$(echo '{"sessionID":"smoke"}' | "$AGENT" --workspace "$SMOKE/repo" hook system-transform)"
if echo "$answer" | grep -q "repository instructions"; then
    echo "ok: memory block present"
else
    echo "FAILED: memory block missing from the hook answer"
    exit 1
fi

echo
echo "All steps passed."
