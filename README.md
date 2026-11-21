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

## Build e deploy na testnet

O repositório compila, instala o WASM e cria instâncias na testnet sem depender do Core:

```bash
# Compilar todos os WASMs com as features corretas de testnet
scripts/deploy-testnet.sh build all

# Instalar todos os WASMs; a saída contém o sha256 devolvido pela rede
scripts/deploy-testnet.sh upload all --source <identidade do Stellar CLI ou chave S...>

# Instalar e criar uma instância; os argumentos após `--` vão ao construtor
scripts/deploy-testnet.sh deploy yield_distributor \
  --source <identidade do Stellar CLI ou chave S...> -- --admin G...
```

Contratos com construtor exigem seus argumentos: `token_sale`, `yield_distributor`, `transfer_agent` e
`custody_registry`. `maturity_settlement` cria uma instância vazia e continua exigindo a chamada posterior de
`initialize`. `recovery_policy` não tem construtor. Use `stellar contract deploy --help` dentro do crate para ver a
interface gerada e o formato de structs como `Config`.

O comando é deliberadamente limitado à testnet. Ele sempre usa `--locked` e `--no-default-features`; nos quatro crates
com diferenças por rede, também usa `--features testnet`. Depois do upload, compara o hash devolvido pela rede com o
SHA-256 do arquivo local. Os artefatos ficam em `.artifacts/testnet/` e não são versionados.

A Action manual **deploy testnet** faz o upload de um contrato ou de todos. Ela usa o environment protegido `testnet`,
que deve conter o secret `TESTNET_DEPLOYER_SECRET`. A Action instala somente o código WASM; a criação de instâncias fica
no comando local porque os parâmetros e os responsáveis variam por contrato.

O Core continua consumindo o commit fixado no `contracts.lock.json` dele. Um upload ou deploy de testnet feito aqui não
altera automaticamente a versão usada pelo Core.

**Não use este fluxo para cerimônias de produção.** Mainnet continua exigindo artefato staged, manifesto, conferência de
hash e runbook no repositório operacional.
