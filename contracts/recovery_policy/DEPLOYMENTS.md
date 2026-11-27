# recovery_policy — deployments

RecoveryPolicy (singleton, sem admin e sem upgrade). A narrativa completa (txs, provas, toolchain, motivo de cada versão) está no `radox-core` `docs/Operations/DEPLOYMENTS.md` (linhas citadas no `source` de cada entrada, commit `c8b29bc`). Os builds de mainnet históricos estão no `deploy/mainnet-wasm/manifest.json` do Core.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

`status`: `live` = aceito hoje pelo Core (ver `deployments/README.md`); `legacy` = histórico, já não aceito.

## Registro

Âncora do [`deployments/registry.json`](../../deployments/registry.json): uma linha por entrada, igual nos dois sentidos (`node scripts/check-registry.mjs`). Mude os dois no mesmo commit. `—` = null; a origem de cada valor está no `source` do registry.

| rede | versão | features | status | wasm | contrato | nota |
|---|---|---|---|---|---|---|
| testnet | 2 | testnet | live | `6fed6f775bd49b97962c5e51d9b68411f5748e2dc5b322dc32accd5e202c8c25` | `CAJKIIHP3R64TIIVCXZ2FDYSFUZFND3J6OYEQGL2HXDIHFURJ53O44C5` | features inferidas |
| testnet | 2 | mainnet | legacy | `aa04c8669124f4a8dde80edf6f7e4017f19ccd73aaab4e7226e8dd5048fc0abb` | `CCCV4LERF642OR3IHUOI64AX777BUMEUUYPSOCDOZCSRFCIOZX4KLUEX` | prova (build mainnet na testnet) |
| testnet | 1 | testnet | legacy | `23c1d04e1398b07507d49e2e1c7fad0c9d2d9bf382d172ebadcc16caa18838a5` | `CA2DYA7B763YF7EUIFCRDRPN3L723QDUDIEISIWJIN3OS74AIFLWVW5Z` |  |
| testnet | 1 | mainnet | legacy | `700cb1ee6f1e74630166a09a0ef92c8eea37dcd59b6c7fd565c040a6d2b607ec` | `CCTKD2YFLWHVAYJBFTTVLINCZPB3HS6YYUBPDOCZRVGIU6JPGOJKAXP5` | prova (build mainnet na testnet) |
