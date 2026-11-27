# deployments/

`registry.json` é a lista única de deployments por rede: o que está publicado onde (pedido do Pedro no radox-core#3, 27/11/2026). Os consumidores (Core, Rail) leem daqui. Os `DEPLOYMENTS.md` são a narrativa e o `node scripts/check-registry.mjs` confere-os contra este arquivo.

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
- `status`:
  - `live`: o wasm/instância corrente daquele crate naquela rede segundo a fonte (registrado, em uso ou aceito para instâncias novas).
    Pode haver vários `live` por crate e rede. Exemplos: o token_sale testnet v10 (em uso no dev) e v11 (registrado), e as 4 instâncias do custody. Também pode não haver nenhum: o transfer_agent testnet só tem `legacy`, e a mainnet não tem transfer_agent nem recovery_policy (crate sem deploy numa rede = sem chave).
    O token_sale v11 e o maturity_settlement v5 de testnet são `live` com dúvida na própria fonte. A linha diz «registrado», mas o mesmo texto diz que o env segue no anterior (radox-core DEPLOYMENTS.md:164 e :178/:185); o `source` marca `UNCERTAIN`.
  - `legacy`: está na rede, mas não é o corrente. Inclui versões substituídas, provas, ensaios e os wasm não reproduzíveis a partir da fonte, como o `transfer_agent` `3870748b…` (Rail `legacy: true`) e o yield `11887707…`. Uma instância `legacy` pode continuar rodando. Exemplo: o singleton do yield na testnet.
  - Na mainnet, o `uploaded` do `manifest.json` do Core (maturity_settlement v3 `0f07a598…`, código instalado e wasm corrente das ofertas) foi mapeado para `live`.
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
node scripts/check-registry.mjs                 # schema + Cargo.toml + contracts/*/DEPLOYMENTS.md; exit 1 com a lista dos problemas
node scripts/caatinga.mjs query yield_distributor --network mainnet   # só leitura; --json para máquina
node --test scripts/test/registry.test.mjs      # testes
```

O check confere:
1. O schema e os invariantes: `(rede, contractId)` único; o mesmo wasm tem o mesmo crate, versão e features em qualquer rede.
2. Que cada hash completo, hash abreviado (`8230ed93…1342`) e contract ID citado num `contracts/<crate>/DEPLOYMENTS.md` está no registry. Um hash de tx conta se estiver no `source` de uma entrada do crate. Sob um título `## testnet`/`## mainnet`, tem de estar nessa rede.
3. O sentido inverso, só para os crates que têm `DEPLOYMENTS.md` aqui (hoje só o custody_registry): cada entrada `live` tem o wasm (completo) e o contract ID citados nele. Os outros crates têm a narrativa no `radox-core` `docs/Operations/DEPLOYMENTS.md`, fora deste repo.

## Pendente

- **Cruzar com o radox-internal.** O Pedro tem 5 deployments de testnet e 3 de mainnet, com contractId e wasmHash. Esses dados ainda não entraram: falta o arquivo dele. Ao chegar: cada par entra (ou confirma uma entrada existente) com `source: "radox-internal <arquivo> …"`, e os `null` de `contractId`/`features`/`version` que ele resolver são preenchidos.
- Sem hash nas fontes, logo fora do registry: token_sale v6 (testnet), maturity_settlement v1 (testnet), yield_distributor v2 pré-F-004 (testnet). Também ficam fora os hashes mainnet antigos `dd868e25…`/`3e54eb72…`, citados truncados como builds que não se reproduziram, e o yield v3 mainnet `d3f23a9f…`, que nunca subiu.
- A CI ainda não roda o check (ver o PR que adicionou este arquivo).

## Proposta: `deploy-testnet.sh` (PR #5) escrever e validar o registry

Só proposta; o PR #5 não foi editado.

1. **Antes do upload:** `node scripts/check-registry.mjs` tem de passar. Um registry quebrado não recebe entradas novas.
2. **Depois do `upload`** (hash da rede == sha256 local, como o script já exige): `node scripts/registry-add.mjs --network testnet --crate <c> --wasm <hash> --version <version()> --features <testnet|null> --source "deploy-testnet.sh upload tx <tx> (<commit>)"`.
   - O helper ainda não existe; é um passo seguinte pequeno, sobre `scripts/lib/registry.mjs`.
   - Ele junta a entrada sem `contractId` (ou não faz nada se o par já existe) e reescreve o arquivo com a ordem estável.
   - `--features` sai de `needs_network_feature`, e o check recusa uma feature que o Cargo.toml não declara.
   - A versão é lida da `CONTRACT_VERSION` do crate, ou de `stellar contract invoke … -- version` depois do deploy.
3. **Depois do `deploy`:** a mesma chamada com `--contract-id <C…>`. A entrada nova entra `live` e, por omissão, a `live` anterior do mesmo crate passa a `legacy`. Uma flag `--keep-live` mantém as duas, para instâncias paralelas como as do custody.
4. **Na Action manual:** o `upload-results.txt` passa a levar uma linha JSON por crate (`{crate, wasmHash, tx}`). A Action não faz commit no registry (não tem permissão de escrita, e um commit automático em main é proibido): quem disparou a Action abre o PR com a linha juntada pelo helper. O check na CI do PR fecha o ciclo.
5. **Só testnet:** o helper recusa `--network mainnet`. A mainnet entra só por PR humano depois da cerimônia, com o `source` a apontar para a tx.
