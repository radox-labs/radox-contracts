# Radox Contracts

Código-fonte dos contratos Soroban usados pelos produtos Radox. Este repositório foi preparado para ser a fonte única dos contratos; não se confunde com os registros de implantação.

## Contratos

- `token_sale`, `yield_distributor`, `maturity_settlement`, `transfer_agent` e `recovery_policy`: copiados de `radox-core` no commit `9414d8f10644a8028dfce8e62fbab8a2abc530d0`.
- `custody_registry`: copiado de `pwsaragossy/radox-rail` no commit `0961f83d6e553159021705131fd27a329aebc68a`.

O código-fonte não prova o estado implantado na rede. Confira IDs, hashes de WASM e registros de implantação antes de operar.

Cada crate é independente e deve ser compilado e testado dentro de `contracts/<nome>/`, com seu toolchain, lockfile e versão de `soroban-sdk`. A licença do repositório e de todos os crates é Apache-2.0, confirmada para esta publicação pelo responsável da Radox.

A migração dos scripts de build e deploy do Core ainda está em andamento. **Não use este diretório para cerimônias de produção** sem validar a equivalência byte a byte dos artefatos, hashes e configurações de rede.
