# Radox Contracts

Código-fonte dos contratos Soroban usados pelos produtos Radox. Este repositório foi preparado para ser a fonte única dos contratos; não se confunde com os registros de implantação.

## Aviso legal

Este repositório contém apenas código-fonte de contratos inteligentes. Ele não constitui oferta pública de valores mobiliários, nem recomendação, solicitação ou consultoria de investimento, e não descreve, anuncia nem promove nenhuma oferta específica.

Alguns contratos (por exemplo, `token_sale`) implementam mecanismos ligados à Resolução CVM nº 88/2022, que regula o crowdfunding de investimento no Brasil. Uma oferta feita com base nessa resolução só pode ser distribuída por uma plataforma eletrônica de investimento participativo registrada na CVM (Res. CVM 88, arts. 2º, IV, 5º e 18). Qualquer outra emissão pública de valores mobiliários no Brasil depende de registro na CVM ou de dispensa prevista na regulamentação (Lei nº 6.385/1976, art. 19). O cumprimento dessas regras é responsabilidade da plataforma e do emissor que operam a oferta, incluindo a parte que fica fora da blockchain (cadastro, termo de ciência de risco, limites por investidor, divulgação de informações). Usar este código não torna nenhuma oferta conforme.

O código é distribuído "no estado em que se encontra" ("AS IS"), sem garantias de qualquer tipo, nos termos das seções 7 e 8 da [licença Apache-2.0](LICENSE).

Salvo se um relatório de auditoria estiver publicado e indicado neste README, trate o código como não auditado. Menções a revisões, achados ou auditores nos comentários do código não são um relatório de auditoria e não implicam nenhuma.

## Contratos

- `token_sale`, `yield_distributor`, `maturity_settlement`, `transfer_agent` e `recovery_policy`: copiados de `radox-core` no commit `9414d8f10644a8028dfce8e62fbab8a2abc530d0`.
- `custody_registry`: copiado de `pwsaragossy/radox-rail` no commit `0961f83d6e553159021705131fd27a329aebc68a`.

O código-fonte não prova o estado implantado na rede. Confira IDs, hashes de WASM e registros de implantação antes de operar.

Cada crate é independente e deve ser compilado e testado dentro de `contracts/<nome>/`, com seu toolchain, lockfile e versão de `soroban-sdk`. A licença do repositório e de todos os crates é Apache-2.0, confirmada para esta publicação pelo responsável da Radox.

A migração dos scripts de build e deploy do Core ainda está em andamento. **Não use este diretório para cerimônias de produção** sem validar a equivalência byte a byte dos artefatos, hashes e configurações de rede.

## Deployments e caatinga

A fonte do que está publicado em cada rede é [`deployments/registry.json`](deployments/README.md). O [caatinga](https://github.com/Dione-b/caatinga) (`@caatinga/cli` 3.9.2, pinado em `package.json`) é a ferramenta de consulta e de build testnet:

```sh
npm ci --ignore-scripts
npx caatinga status --network testnet   # só leitura: o que está publicado, por contrato
npx caatinga doctor --network testnet   # toolchain, config e artifacts
npx caatinga build yield_distributor    # build testnet (--no-default-features --features testnet); 27/11: deu f6fbba00…, a entrada live v5
```

O `caatinga.artifacts.json` é gerado a partir do registry (`node scripts/gen-caatinga-artifacts.mjs`) e nunca se edita à mão; o `node scripts/check-registry.mjs` falha se ele divergir. O caatinga não faz deploy aqui, não escreve `.env` e nunca toca na mainnet (ver o topo do `caatinga.config.ts`).
