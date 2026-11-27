# token_sale — deployments

TokenSale (por oferta). A narrativa completa (txs, provas, toolchain, motivo de cada versão) está no `radox-core` `docs/Operations/DEPLOYMENTS.md` (linhas citadas no `source` de cada entrada, commit `c8b29bc`). Os builds de mainnet históricos estão no `deploy/mainnet-wasm/manifest.json` do Core.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

`status`: `live` = aceito hoje pelo Core (ver `deployments/README.md`); `legacy` = histórico, já não aceito.

## Registro

Âncora do [`deployments/registry.json`](../../deployments/registry.json): uma linha por entrada, igual nos dois sentidos (`node scripts/check-registry.mjs`). Mude os dois no mesmo commit. `—` = null; a origem de cada valor está no `source` do registry.

| rede | versão | features | status | wasm | contrato | nota |
|---|---|---|---|---|---|---|
| mainnet | 7 | mainnet | live | `524ede1c6122b15456329564878a191a00a7062e1a8c9009deaa44eeb5a59c57` | — | por oferta |
| testnet | 10 | testnet | live | `cda244f379ba2efc7690decb425089ec81117c1534e6e47c260f025e99c41b38` | — | por oferta; features inferidas |
| testnet | 11 | — | live | `8d5ae7c2369f6074b6492acb84895f8e223a0e3803c9464bad0af9b52ed9d0aa` | — | por oferta; incerto na fonte |
| testnet | 7 | testnet | legacy | `6b4a190695ff0fbe431e59a302d3ba0d97311ef07b1ae08003f38f40ed668e2c` | — | por oferta; features inferidas |
| testnet | 7 | mainnet | legacy | `524ede1c6122b15456329564878a191a00a7062e1a8c9009deaa44eeb5a59c57` | — |  |
| testnet | 8 | mainnet | legacy | `a404fce339d5742470b6650c695e110c8553cc3804a8340c0dc8baae4e8b01e2` | — | ensaio F3 (upload só de código) |
| testnet | 8 | mainnet | legacy | `60823d392e7aba018602cc8c936f8c4f292fa410f04e47b81323b9ea0b7e5551` | — | ensaio F3 (upload só de código) |
| testnet | 10 | mainnet | legacy | `7909d96f7dd0e617e1c27939dc4fd61431ef9210bc2706c8d36d9d171cdafcf2` | — | ensaio F3 (upload só de código) |
| testnet | 11 | mainnet | legacy | `142f3aa8e2b3844d3638aeb482e372e3612f0ebfd612699f9cc9e6c602e8c32d` | — | ensaio F3 (upload só de código) |
