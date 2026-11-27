# transfer_agent — deployments

TransferAgent (singleton do fornecedor + um por instalação). A narrativa completa (txs, provas, toolchain, motivo de cada versão) está no `radox-core` `docs/Operations/DEPLOYMENTS.md` (linhas citadas no `source` de cada entrada, commit `c8b29bc`). Os builds de mainnet históricos estão no `deploy/mainnet-wasm/manifest.json` do Core.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

`status`: `live` = aceito hoje pelo Core (ver `deployments/README.md`); `legacy` = histórico, já não aceito.

## Registro

Âncora do [`deployments/registry.json`](../../deployments/registry.json): uma linha por entrada, igual nos dois sentidos (`node scripts/check-registry.mjs`). Mude os dois no mesmo commit. `—` = null; a origem de cada valor está no `source` do registry.

| rede | versão | features | status | wasm | contrato | nota |
|---|---|---|---|---|---|---|
| testnet | 4 | — | legacy | `bee83bccdfacde9b0f7fab8ebc80f52fbedd43c5ce30b80fab0da918074c26f5` | `CBCU47QHCGZWVP326UGT6FXIGD33DZIKGMXGOKXXDLWBJ3CRRGKYNKZ2` |  |
| testnet | 5 | — | live | `3870748b709f7b8531c7ebbbc975712333f61949831350ff7192f7076fc6bff1` | `CAF4JIUZBLTQCSL2ZOX4ZEZ7FQDCGVBDVCBXRAJJRM6RW77BBX44SFC7` | não reproduzível da fonte |
| testnet | 5 | — | live | `3870748b709f7b8531c7ebbbc975712333f61949831350ff7192f7076fc6bff1` | `CAYD3EFTN6YOHMHJ5C62WYGABBIN2G6MV5MLHUKWV2EVJP5FFTKXSJWH` |  |
| testnet | 5 | testing | legacy | `5915649c2fd4ab6aaa93fe90ae718fb8d26826ea39190257f584577cee8e3576` | — | ensaio F3 (upload só de código) |
