#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS_DIR="${RADOX_CONTRACTS_ARTIFACTS_DIR:-$ROOT/.artifacts/testnet}"

CONTRACTS=(
  token_sale
  maturity_settlement
  yield_distributor
  recovery_policy
  transfer_agent
  custody_registry
)

usage() {
  cat <<'EOF'
Uso:
  scripts/deploy-testnet.sh build  <contrato|all>
  scripts/deploy-testnet.sh upload <contrato|all> [--source <identidade-ou-chave-S>]
  scripts/deploy-testnet.sh deploy <contrato>     [--source <identidade-ou-chave-S>] [-- <argumentos do construtor>]

Exemplos:
  scripts/deploy-testnet.sh upload all --source radox-testnet
  scripts/deploy-testnet.sh deploy yield_distributor --source radox-testnet -- --admin G...
  scripts/deploy-testnet.sh deploy custody_registry --source radox-testnet -- --admin G... --operator G...

Sem `--source`, o Stellar CLI usa `STELLAR_ACCOUNT`. Somente a testnet é aceita.
`upload` instala o WASM e devolve seu hash; `deploy`
também cria uma instância. Os argumentos depois de `--` são repassados ao
construtor gerado pelo Stellar CLI.
EOF
}

die() {
  printf 'erro: %s\n' "$*" >&2
  exit 1
}

is_contract() {
  local wanted="$1" contract
  for contract in "${CONTRACTS[@]}"; do
    [[ "$contract" == "$wanted" ]] && return 0
  done
  return 1
}

needs_network_feature() {
  case "$1" in
    token_sale|maturity_settlement|yield_distributor|recovery_policy) return 0 ;;
    transfer_agent|custody_registry) return 1 ;;
  esac
}

needs_constructor_args() {
  case "$1" in
    token_sale|yield_distributor|transfer_agent|custody_registry) return 0 ;;
    maturity_settlement|recovery_policy) return 1 ;;
  esac
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

require_tools() {
  command -v stellar >/dev/null 2>&1 || die 'stellar-cli não encontrado no PATH'
  command -v cargo >/dev/null 2>&1 || die 'cargo não encontrado no PATH'
}

build_contract() {
  local contract="$1"
  local crate="$ROOT/contracts/$contract"
  local out="$ARTIFACTS_DIR/$contract"
  local wasm="$out/$contract.wasm"
  local args=(contract build --locked --no-default-features --out-dir "$out")

  [[ -f "$crate/Cargo.toml" ]] || die "crate ausente: contracts/$contract"
  mkdir -p "$out"
  if needs_network_feature "$contract"; then
    args+=(--features testnet)
  fi

  printf '==> build %s (testnet)\n' "$contract" >&2
  (cd "$crate" && stellar "${args[@]}")
  [[ -f "$wasm" ]] || die "build não produziu $wasm"
  printf '%s\n' "$wasm"
}

upload_contract() {
  local contract="$1" source="$2"
  local wasm local_hash remote_hash
  wasm="$(build_contract "$contract")"
  local_hash="$(sha256_file "$wasm")"

  printf '==> upload %s (sha256 %s)\n' "$contract" "$local_hash" >&2
  if [[ -n "$source" ]]; then
    remote_hash="$(stellar contract upload --wasm "$wasm" --source "$source" --network testnet)"
  else
    remote_hash="$(stellar contract upload --wasm "$wasm" --network testnet)"
  fi
  remote_hash="$(printf '%s' "$remote_hash" | tr -d '[:space:]')"
  [[ "$remote_hash" =~ ^[0-9a-fA-F]{64}$ ]] || die "upload devolveu um hash inválido: $remote_hash"
  remote_hash="$(printf '%s' "$remote_hash" | tr '[:upper:]' '[:lower:]')"
  [[ "$remote_hash" == "$local_hash" ]] || die "hash da rede $remote_hash difere do arquivo $local_hash"

  printf '%s\n' "$remote_hash"
}

main() {
  [[ $# -ge 2 ]] || { usage; exit 2; }
  local action="$1" contract="$2" source=''
  shift 2

  case "$action" in
    build|upload|deploy) ;;
    *) usage; die "ação desconhecida: $action" ;;
  esac
  if [[ "$contract" != all ]] && ! is_contract "$contract"; then
    die "contrato desconhecido: $contract"
  fi
  [[ "$action" != deploy || "$contract" != all ]] || die '`deploy all` não existe: cada instância requer parâmetros próprios'

  local constructor_args=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --source)
        [[ $# -ge 2 ]] || die '--source exige um valor'
        source="$2"
        shift 2
        ;;
      --)
        shift
        constructor_args=("$@")
        break
        ;;
      *) die "argumento desconhecido: $1" ;;
    esac
  done

  require_tools
  if [[ "$action" == upload || "$action" == deploy ]]; then
    [[ -n "$source" || -n "${STELLAR_ACCOUNT:-}" ]] || die "$action exige --source ou STELLAR_ACCOUNT"
  fi

  local selected=()
  if [[ "$contract" == all ]]; then selected=("${CONTRACTS[@]}"); else selected=("$contract"); fi

  local item hash wasm
  case "$action" in
    build)
      for item in "${selected[@]}"; do
        wasm="$(build_contract "$item")"
        printf '%s\t%s\n' "$item" "$(sha256_file "$wasm")"
      done
      ;;
    upload)
      for item in "${selected[@]}"; do
        hash="$(upload_contract "$item" "$source")"
        printf '%s\t%s\n' "$item" "$hash"
      done
      ;;
    deploy)
      if needs_constructor_args "$contract" && [[ ${#constructor_args[@]} -eq 0 ]]; then
        die "$contract exige argumentos de construtor depois de --"
      fi
      hash="$(upload_contract "$contract" "$source")"
      printf '==> deploy %s a partir de %s\n' "$contract" "$hash" >&2
      if [[ -n "$source" && ${#constructor_args[@]} -gt 0 ]]; then
        stellar contract deploy --wasm-hash "$hash" --source "$source" --network testnet -- "${constructor_args[@]}"
      elif [[ -n "$source" ]]; then
        stellar contract deploy --wasm-hash "$hash" --source "$source" --network testnet
      elif [[ ${#constructor_args[@]} -gt 0 ]]; then
        stellar contract deploy --wasm-hash "$hash" --network testnet -- "${constructor_args[@]}"
      else
        stellar contract deploy --wasm-hash "$hash" --network testnet
      fi
      ;;
  esac
}

main "$@"
