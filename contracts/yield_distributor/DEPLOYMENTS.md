# yield_distributor — deployments

YieldDistributor (singleton do fornecedor + uma instância por instalação). A narrativa completa (txs, provas, toolchain, motivo de cada versão) está no `radox-core` `docs/Operations/DEPLOYMENTS.md` (linhas citadas no `source` de cada entrada, commit `c8b29bc`). Os builds de mainnet históricos estão no `deploy/mainnet-wasm/manifest.json` do Core.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

`status`: `live` = aceito hoje pelo Core (ver `deployments/README.md`); `legacy` = histórico, já não aceito.

## Registro

Âncora do [`deployments/registry.json`](../../deployments/registry.json): uma linha por entrada, igual nos dois sentidos (`node scripts/check-registry.mjs`). Mude os dois no mesmo commit. `—` = null; a origem de cada valor está no `source` do registry.

| rede | versão | features | status | wasm | contrato | nota |
|---|---|---|---|---|---|---|
| mainnet | 4 | mainnet | live | `88509c72c0a82812b8f110b04bcd2dd3f734a43aa4b8bb18a28160ca9b9e4203` | `CDTXAGKIABYIKCRBD5CBEDYLVYZEEL5R5P4JT25AS4QHQFY2UGLTIMNX` |  |
| testnet | 5 | testnet | live | `f6fbba0082eebe0b888e1dd44585a30abe666ceaef987b194817d3c3e4c3f251` | — |  |
| testnet | 5 | — | live | `118877079046c04ab8ed491c4cd812284db00c13e67bc2fd9bac5552066500bd` | `CBIDF7ZWOR2DZLPXKW553YOJP6FUWFSDG4UBYB25T7AIH2AQRZTSLXWC` | não reproduzível da fonte |
| testnet | 4 | testnet | live | `ebb1ce41efcfb65b0f169f78f2e0132d3512124b9182b43a78de32508f1376ad` | — |  |
| testnet | 2 | — | legacy | `0d2a203e800eb9a43649e3995ed096fd271431385c983c8ef09c550648daf13b` | — |  |
| testnet | 3 | — | legacy | `4efd1bfdff7ef8c90e8d8146521b2a447cb770634386378f8961b49883fb7507` | — |  |
| testnet | 4 | mainnet | legacy | `88509c72c0a82812b8f110b04bcd2dd3f734a43aa4b8bb18a28160ca9b9e4203` | — | ensaio F3 (upload só de código) |
