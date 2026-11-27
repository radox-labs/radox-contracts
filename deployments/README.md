# deployments/

`registry.json` é a lista única de deployments por rede: o que está publicado onde (pedido do Pedro no radox-core#3, 27/11/2026). Os consumidores (Core, Rail) leem daqui. Os `DEPLOYMENTS.md` são a narrativa e o `node scripts/check-registry.mjs` confere-os contra este arquivo.

O [caatinga](https://github.com/Dione-b/caatinga) (`npx caatinga status`, `doctor`, `build`) é a ferramenta de consulta e de build testnet; lê o `caatinga.artifacts.json` da raiz, que é **gerado** deste registry (ver «caatinga» abaixo). A fonte continua a ser o `registry.json`.

O código-fonte não prova o que está implantado. Este registry também não prova: ele reúne o que as fontes registraram. Antes de operar, confirme o contract ID e o hash do wasm na rede (`stellar contract fetch --id … | sha256sum`).

## Formato (schema 1, contrato de interface)

```json
{
  "schema": 1,
  "networks": {
    "testnet": { "<crate>": [ { "wasmHash": "…", "contractId": "C…|null", "version": 5, "features": "testnet|mainnet|testing|null", "status": "live|legacy", "source": "…" } ] },
    "mainnet": { … }
  }
}
```

- Redes: `testnet` e `mainnet`. Crates: `token_sale`, `maturity_settlement`, `transfer_agent`, `yield_distributor`, `recovery_policy`, `custody_registry`.
- Uma entrada é um par (wasm instalado, instância). Um wasm com várias instâncias gera várias entradas; um wasm sem instância própria (contratos por oferta, upload de ensaio) leva `contractId: null`.
- Uma instância que fez `upgrade` aparece só no wasm que roda hoje. Exemplo: o singleton `CBIDF7ZW…` do yield está no `11887707…`, e o `ebb1ce41…` (v4) ficou com `contractId: null` e a história no `source`.
- `null` = a fonte não registra o valor. Nada foi inventado nem reconstruído; IDs truncados nas fontes (`CBRIKOD64…`, `CBTHW3BK…`) ficam fora.
- `features` = a feature de rede com que o wasm foi compilado (`--no-default-features --features <x>`). `null` junta dois casos: o crate não tem feature de rede, ou a fonte não registra os flags. O check recusa uma feature que o `Cargo.toml` do crate não declara, e na mainnet recusa `testnet` e `testing`.
- `status` (decisão do Leo, 27/11/2026):
  - `live` = aceito hoje pelo Core.
    - yield_distributor e transfer_agent: o conjunto `live` de cada rede é exatamente o `YIELD_WASM_ACCEPTED`/`AGENT_WASM_ACCEPTED` (`public` = mainnet), e os singletons `YIELD_SHARED_SINGLETON`/`AGENT_SHARED_SINGLETON` são entradas `live` (radox-core@c8b29bc `backend/src/config/yieldInstance.js:34-45, :58-61`, `agentInstance.js:25-30, :37-39`). Daí ficam `live` na testnet o yield `ebb1ce41…`, `11887707…` (singleton `CBIDF7ZW…`) e `f6fbba00…`, e o transfer_agent `3870748b…` (`CAF4JIUZ…` e a instância do Rail `CAYD3EFT…`). Um teste compara com a cópia destas listas em c8b29bc; com um checkout do Core, `--core` (ou `RADOX_CORE_DIR` nos testes) as lê ao vivo.
    - token_sale, maturity_settlement, recovery_policy e custody_registry: o Core não tem lista fechada (o wasm vem do env `SALE_WASM_HASH`/`SETTLEMENT_WASM_HASH`, e a policy de `RECOVERY_POLICY_CONTRACT_ID`). `live` = o wasm/instância corrente segundo a narrativa: env do dev, registrado, ou `live`/`uploaded` no `manifest.json` (o `uploaded` do maturity_settlement v3 mainnet `0f07a598…` = `live`). Pode haver vários (token_sale testnet v10 e v11, as 4 instâncias do custody).
    - O token_sale v11 e o maturity_settlement v5 de testnet são `live` com dúvida na própria fonte. A linha diz «registrado», mas o mesmo texto diz que o env segue no anterior (radox-core DEPLOYMENTS.md:164 e :178/:185); o `source` marca `UNCERTAIN`.
  - `legacy` = histórico, já não aceito. Inclui versões substituídas, provas, os uploads do ensaio F3 e o yield `88509c72…` de testnet, que é o build mainnet e não está na lista testnet do Core. Uma instância `legacy` pode continuar na rede.
  - Um crate sem deploy numa rede não tem chave: a mainnet não tem transfer_agent nem recovery_policy.
  - Os wasm `staged`/`superseded` do manifest nunca subiram à mainnet e não entram em `mainnet`. Existem na testnet como uploads do ensaio da F3 (`legacy`, `features: "mainnet"`).
- `source` = de onde veio cada valor: arquivo e linha no commit citado (`radox-core@c8b29bc`, `miami-rio-rail@1146786`) e as txs.

## Features por crate (Cargo.toml, 27/11/2026)

| crate | features declaradas | build de rede |
|---|---|---|
| token_sale, maturity_settlement, recovery_policy | `testnet`, `mainnet`, `testing` (default) | `--no-default-features --features testnet\|mainnet` |
| yield_distributor | `testnet`, `mainnet`, `testing` (default), `wasm-tests` (só testes) | `--no-default-features --features testnet\|mainnet` |
| transfer_agent | `testing` (default) | `--no-default-features`, sem feature de rede (o mesmo wasm serve as duas redes) |
| custody_registry | nenhuma | `stellar contract build`, sem features |

O `scripts/deploy-testnet.sh` do PR #5 (`needs_network_feature`) usa `--features testnet` exatamente nos quatro primeiros e nunca no transfer_agent e no custody_registry. Bate com a tabela. A lista `token_sale|yield_distributor|transfer_agent|custody_registry` do mesmo script é a dos crates com construtor (`needs_constructor_args`), não a das features.

## Ferramentas

```sh
node scripts/check-registry.mjs                 # schema + Cargo.toml + âncoras de contracts/*/DEPLOYMENTS.md; exit 1 com a lista dos problemas
node scripts/check-registry.mjs --core ../radox-core   # + live == listas aceitas do Core
node scripts/caatinga.mjs query yield_distributor --network mainnet   # só leitura; --json para máquina
node scripts/gen-caatinga-artifacts.mjs         # regenera caatinga.artifacts.json; --check só confere
npm ci --ignore-scripts && npx caatinga status --network testnet   # vista do caatinga, só leitura
RADOX_CORE_DIR=../radox-core npm test           # testes (sem o Core, 1 teste fica skip; os do caatinga pedem npm ci)
```

O check confere:
1. O schema e os invariantes:
   - `contractId` com o checksum do strkey (CRC16);
   - `(rede, contractId)` único, e o mesmo ID não pode estar em duas redes;
   - o mesmo wasm tem o mesmo crate, versão e features em qualquer rede;
   - as features têm de estar declaradas no `Cargo.toml` (na mainnet, nunca `testnet`/`testing`).
2. **Âncora por crate.** Cada um dos 6 `contracts/<crate>/DEPLOYMENTS.md` tem uma secção `## Registro` com a tabela `| rede | versão | features | status | wasm | contrato | nota |`, uma linha por entrada. O registry e a tabela têm de ser iguais nos dois sentidos (rede, wasm, contrato, versão, features, status). Mudar um sem o outro, apagar ou acrescentar uma entrada, ou trocar um carácter num hash ou ID falha. Um crate sem âncora também falha.
3. Que cada hash completo, hash abreviado (`8230ed93…1342`) e contract ID citado no resto do `DEPLOYMENTS.md` está no registry. Um hash de tx conta se estiver no `source` de uma entrada do crate. Sob um título `## testnet`/`## mainnet`, tem de estar nessa rede.
4. Que o `caatinga.artifacts.json` é igual ao gerado do registry (só depois de 1–3 passarem; `--artifacts <arquivo>` para outro caminho).
5. Com `--core <checkout do radox-core>`: que o `live` do yield_distributor e do transfer_agent é igual às listas do Core.

A âncora é uma segunda cópia de propósito. Os dados não ficam mais certos por isso (a origem de cada valor continua no `source`), mas um erro de edição num dos lados deixa de passar calado. A narrativa longa continua no `radox-core` `docs/Operations/DEPLOYMENTS.md`.

## caatinga (`caatinga.config.ts` + `caatinga.artifacts.json`)

- `caatinga.config.ts` veio do radox-core (PR radox-labs/radox-core#26, trazido do radox-internal), com os caminhos deste repo e o `custody_registry` acrescentado. `buildFeatures`: `--no-default-features --features testnet` no token_sale, maturity_settlement, yield_distributor e recovery_policy; `[]` no transfer_agent e no custody_registry. Nesses quatro, um `caatinga build` nunca gera o wasm `testing` nem o de mainnet. O transfer_agent com `[]` compila com os defaults do crate, ou seja `testing` (registry `5915649c…`, «crate defaults = testing»), e não `--no-default-features` como na tabela de features acima: decisão em aberto (Leo/Pedro), mantido como no original. O build mainnet continua manual, na cerimônia.
- `caatinga.artifacts.json` (schema v2 do `@caatinga/core` 3.9.2) **não se edita à mão**: `node scripts/gen-caatinga-artifacts.mjs` regenera-o, e o `check-registry` falha se o arquivo não for byte a byte igual ao gerado (um carácter, uma entrada a mais ou a menos). Ao mudar o registry: editar `registry.json` + âncoras, regenerar, commitar os três.
- Só entram as entradas `live`. O caatinga guarda uma entrada por contrato e rede; quando há várias `live`, a principal é **a primeira, na ordem do registry, com `contractId`, ou senão a primeira**. As outras vão para `otherLive` (campo extra, que o caatinga ignora). Hoje isso dá os singletons do Core (yield `CBIDF7ZW…`, transfer_agent `CAF4JIUZ…`) e o token_sale v10 `cda244f3…` (o `SALE_WASM_HASH` do dev). Ficam em `otherLive`: token_sale v11 `8d5ae7c2…`; yield `f6fbba00…` e `ebb1ce41…`; transfer_agent `CAYD3EFT…`; custody `CBTHLSQX…`, `CD7BUCY3…`, `CDDWEENF…`. A lista completa (com `legacy`) é a do `node scripts/caatinga.mjs query <crate>`.
- Campos que o caatinga exige e o registry não tem: `contractId` `null` → `"PER-OFFER"` (token_sale e maturity_settlement, deploy por oferta, convenção do radox-core; noutro crate sem `contractId` o gerador falha); `deployedAt` = a primeira data `AAAA-MM-DD` do `source` da entrada principal (pode ser a do upload, não a do deploy), ou `1970-01-01T00:00:00.000Z` se não houver nenhuma (hoje todas têm). `version` e `features` ficam como campos extra (o `version` é o espelho `SALE_WASM_VERSION` do Core).
- Nada de `caatinga deploy`, `upgrade`, `rollback` ou `migrate`: reescrevem o artifacts e o check passa a falhar. Nem `sync-env`, que escreve `.env`. O `caatinga inspect` já mentiu sobre o estado on-chain (radox-core, 13/08): confirmar com `stellar contract fetch --id … | sha256sum`.
- `scripts/caatinga.mjs` (consulta do registry) é outro programa, anterior a esta restauração; o nome coincide.

## Pendente

- **Cruzar com o radox-internal.** O Pedro tem 5 deployments de testnet e 3 de mainnet, com contractId e wasmHash. Esses dados ainda não entraram: falta o arquivo dele. Ao chegar: cada par entra (ou confirma uma entrada existente) com `source: "radox-internal <arquivo> …"`, e os `null` de `contractId`/`features`/`version` que ele resolver são preenchidos.
- Sem hash nas fontes, logo fora do registry: token_sale v6 (testnet), maturity_settlement v1 (testnet), yield_distributor v2 pré-F-004 (testnet). Também ficam fora os hashes mainnet antigos `dd868e25…`/`3e54eb72…`, citados truncados como builds que não se reproduziram, e o yield v3 mainnet `d3f23a9f…`, que nunca subiu.
- A CI ainda não roda o check nem a suite node (`npm ci --ignore-scripts && npm test`): o `contracts.yml` só compila e testa os crates. Acrescentar o job é mudança de CI, à espera de decisão.

## Proposta: `deploy-testnet.sh` (PR #5) escrever e validar o registry

Só proposta; o PR #5 não foi editado.

1. **Antes do upload:** `node scripts/check-registry.mjs` tem de passar. Um registry quebrado não recebe entradas novas.
2. **Depois do `upload`** (hash da rede == sha256 local, como o script já exige): `node scripts/registry-add.mjs --network testnet --crate <c> --wasm <hash> --version <version()> --features <testnet|null> --source "deploy-testnet.sh upload tx <tx> (<commit>)"`.
   - O helper ainda não existe; é um passo seguinte pequeno, sobre `scripts/lib/registry.mjs`.
   - Ele junta a entrada sem `contractId` ao registry **e** a linha correspondente à tabela `## Registro` do `contracts/<c>/DEPLOYMENTS.md` (ou não faz nada se o par já existe), com a ordem estável. O check exige os dois iguais.
   - `--features` sai de `needs_network_feature`, e o check recusa uma feature que o Cargo.toml não declara.
   - A versão é lida da `CONTRACT_VERSION` do crate, ou de `stellar contract invoke … -- version` depois do deploy.
3. **Depois do `deploy`:** a mesma chamada com `--contract-id <C…>`. A entrada nova entra `legacy` por omissão: `live` = aceito pelo Core, e um upload da testnet só passa a `live` quando o Core o puser na lista (yield/transfer_agent) ou no env (token_sale/maturity), no mesmo PR ou num seguinte. `--live` marca-a já `live`, e o `--core` do check confere.
4. **Na Action manual:** o `upload-results.txt` passa a levar uma linha JSON por crate (`{crate, wasmHash, tx}`). A Action não faz commit no registry (não tem permissão de escrita, e um commit automático em main é proibido): quem disparou a Action abre o PR com a linha juntada pelo helper. O check na CI do PR fecha o ciclo.
5. **Só testnet:** o helper recusa `--network mainnet`. A mainnet entra só por PR humano depois da cerimônia, com o `source` a apontar para a tx.
