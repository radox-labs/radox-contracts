#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/artifacts"

cat > "$TMP/bin/cargo" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

cat > "$TMP/bin/stellar" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$PWD :: $*" >> "$DEPLOY_TEST_LOG"

if [[ "$1 $2" == 'contract build' ]]; then
  shift 2
  out=''
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == --out-dir ]]; then out="$2"; shift 2; else shift; fi
  done
  [[ -n "$out" ]]
  mkdir -p "$out"
  printf 'fake wasm for %s\n' "$(basename "$PWD")" > "$out/$(basename "$PWD").wasm"
  exit 0
fi

if [[ "$1 $2" == 'contract upload' ]]; then
  shift 2
  wasm=''
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == --wasm ]]; then wasm="$2"; shift 2; else shift; fi
  done
  [[ -n "$wasm" ]]
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$wasm" | awk '{print $1}'
  else
    shasum -a 256 "$wasm" | awk '{print $1}'
  fi
  exit 0
fi

if [[ "$1 $2" == 'contract deploy' ]]; then
  printf '%s\n' 'CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM'
  exit 0
fi

exit 2
EOF
chmod +x "$TMP/bin/cargo" "$TMP/bin/stellar"

export PATH="$TMP/bin:$PATH"
export DEPLOY_TEST_LOG="$TMP/stellar.log"
export RADOX_CONTRACTS_ARTIFACTS_DIR="$TMP/artifacts"

"$ROOT/scripts/deploy-testnet.sh" build token_sale >/dev/null
grep -q -- '--no-default-features' "$DEPLOY_TEST_LOG"
grep -q -- '--features testnet' "$DEPLOY_TEST_LOG"

: > "$DEPLOY_TEST_LOG"
"$ROOT/scripts/deploy-testnet.sh" build transfer_agent >/dev/null
grep -q -- '--no-default-features' "$DEPLOY_TEST_LOG"
if grep -q -- '--features' "$DEPLOY_TEST_LOG"; then
  printf 'transfer_agent recebeu feature de rede indevida\n' >&2
  exit 1
fi

: > "$DEPLOY_TEST_LOG"
"$ROOT/scripts/deploy-testnet.sh" upload custody_registry --source test-source | grep -Eq '^custody_registry[[:space:]][0-9a-f]{64}$'
grep -q -- 'contract upload .* --source test-source --network testnet' "$DEPLOY_TEST_LOG"

: > "$DEPLOY_TEST_LOG"
export STELLAR_ACCOUNT='secret-only-through-env'
"$ROOT/scripts/deploy-testnet.sh" upload recovery_policy >/dev/null
if grep -q -- 'secret-only-through-env' "$DEPLOY_TEST_LOG"; then
  printf 'STELLAR_ACCOUNT vazou para os argumentos do processo\n' >&2
  exit 1
fi
unset STELLAR_ACCOUNT

: > "$DEPLOY_TEST_LOG"
if "$ROOT/scripts/deploy-testnet.sh" deploy yield_distributor --source test-source >/dev/null 2>&1; then
  printf 'yield_distributor sem construtor deveria falhar\n' >&2
  exit 1
fi
if grep -q -- 'contract upload\|contract deploy' "$DEPLOY_TEST_LOG"; then
  printf 'deploy inválido tocou a rede simulada\n' >&2
  exit 1
fi

: > "$DEPLOY_TEST_LOG"
"$ROOT/scripts/deploy-testnet.sh" deploy recovery_policy --source test-source >/dev/null
grep -q -- 'contract deploy .* --source test-source --network testnet' "$DEPLOY_TEST_LOG"

printf 'deploy-testnet: ok\n'
