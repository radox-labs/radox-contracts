# maturity_settlement — deployments

MaturitySettlement (por oferta). A narrativa completa (txs, provas, toolchain, motivo de cada versão) está no `radox-core` `docs/Operations/DEPLOYMENTS.md` (linhas citadas no `source` de cada entrada, commit `c8b29bc`). Os builds de mainnet históricos estão no `deploy/mainnet-wasm/manifest.json` do Core.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

`status`: `live` = aceito hoje pelo Core (ver `deployments/README.md`); `legacy` = histórico, já não aceito.

## Registro

Âncora do [`deployments/registry.json`](../../deployments/registry.json): uma linha por entrada, igual nos dois sentidos (`node scripts/check-registry.mjs`). Mude os dois no mesmo commit. `—` = null; a origem de cada valor está no `source` do registry.

| rede | versão | features | status | wasm | contrato | nota |
|---|---|---|---|---|---|---|
| mainnet | 3 | mainnet | live | `0f07a598439581a0fc5990d4d1cbc4e2b7447109c505295ad9f7268790028cbf` | — | por oferta |
| testnet | 5 | — | live | `45f7b6fd051d178500b98d5c9f5d293ffaa7bd4974319a6f4403906f67185975` | — | por oferta; incerto na fonte |
| testnet | 4 | — | legacy | `565b30cb813c708121745201b17b5483db66a3677dcc14882713b9279030c6c5` | — | por oferta |
| testnet | 3 | testnet | legacy | `2de593160fa805e8b069a3455e6ef4b556fdbd3aa10839ce0a7f1994bcf8390d` | — | por oferta; features inferidas |
| testnet | 2 | — | legacy | `fb54e7a3e21839abe493a4ed0e95c639d6458975fa0c49291bd55318826f9abb` | — | por oferta |
| testnet | 3 | mainnet | legacy | `0f07a598439581a0fc5990d4d1cbc4e2b7447109c505295ad9f7268790028cbf` | — | ensaio F3 (upload só de código) |
| testnet | 4 | mainnet | legacy | `1499b4f3bc17faf80b13b27b743d12557554264ddb5a2a84598ba25a4e5efafb` | — | ensaio F3 (upload só de código) |
| testnet | 5 | mainnet | legacy | `09c95edb7b6309173a4f5897680f3e8244e8e9fa16c2ce9807349f3446acaea7` | — | ensaio F3 (upload só de código) |
