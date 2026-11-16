# 01 — Diff dos contratos Soroban: radox-core × radox-contracts

Data: 2026-11-14 · Autor: agente blockchain-stellar. A análise não alterou nem implantou nada. Este ficheiro entra no branch `ci/contracts-consolidation` junto com o CI (`.github/workflows/contracts.yml`) e o `rust-toolchain.toml` do custody_registry (§4).

## 0. Resumo

| Contrato | Diff de código/testes/snapshots | Diff Cargo.toml | soroban-sdk (core / rc) | WASM sha256 igual nos dois lados | Canónica | Porte do Core necessário |
|---|---|---|---|---|---|---|
| token_sale | nenhum | `license = "Apache-2.0"` só no rc | 25.3.0 / 25.3.0 | sim (testnet e mainnet) | radox-contracts | nenhum |
| maturity_settlement | nenhum | idem | 25.3.0 / 25.3.0 | sim | radox-contracts | nenhum |
| yield_distributor | nenhum | idem | 25.3.0 / 25.3.0 | sim | radox-contracts | nenhum |
| transfer_agent | nenhum | idem | 25.3.0 / 25.3.0 | sim | radox-contracts | nenhum |
| recovery_policy | nenhum | idem | 25.3.0 / 25.3.0 | sim | radox-contracts | nenhum |
| shared (`mock_usdc.rs`) | nenhum | (não é crate) | — | — | radox-contracts | nenhum |
| custody_registry | só existe no rc | — | — / 29.0.1 | build reproduz o hash implantado `8230ed93…1342` | radox-contracts | — |

A premissa "já divergem" não se confirma no código. Nos HEADs atuais (core `5ef1edc`, rc `d282f7b`, iguais aos `origin/main` verificados com `git ls-remote` a 2026-11-14), a única diferença nos 5 contratos é uma linha de metadados de licença em cada `Cargo.toml`. Fontes, testes, `Cargo.lock`, `rust-toolchain.toml` e `test_snapshots/` são idênticos byte a byte, e os WASM compilados têm o mesmo sha256 nos dois lados.

No checkout local analisado, as alterações não commitadas que o `git status` mostrava (655 ficheiros em `radox-core/contracts`, 685 em `radox-contracts/contracts`) eram só fins de linha: `git diff --ignore-cr-at-eol` dá 0 ficheiros com diff real e 0 mudanças de modo. Por isso, a comparação usou `git archive HEAD` de cada repo, exportado para uma pasta temporária fora dos repos.

## 1. Diff por contrato

### 1.1 Método
- `git archive HEAD contracts` de cada repo → `diff -rq`. Também correu `diff -rq --strip-trailing-cr -x target` sobre as working trees reais, com o mesmo resultado.
- Proveniência: `git log --follow`/`git log -- contracts` nos dois repos e comparação com o commit de origem que o rc declara (`9414d8f`).

### 1.2 Resultado (igual para os 5)
```
+license = "Apache-2.0"      # só em radox-contracts/contracts/<crate>/Cargo.toml, secção [package]
```
- Classificação: **cosmético/metadados**. Não entra no WASM (hashes idênticos, §2) nem mexe em funções, eventos, storage keys, erros, auth ou features.
- `soroban-sdk` 25.3.0 (dep e dev-dep com `testutils`), `Cargo.lock` 25.3.0, toolchain `1.86.0` com targets `wasm32-unknown-unknown` e `wasm32v1-none`: iguais nos dois lados.
- Features de rede (`testing` por omissão, `testnet`, `mainnet`): iguais. `transfer_agent` só tem `testing`. O `compile_error!` que bloqueia `testing+mainnet` existe nos dois lados e dispara nos dois (rc=101).
- `shared/mock_usdc.rs`: idêntico. É incluído por `#[path = "../../shared/mock_usdc.rs"]` em `token_sale`, `yield_distributor` e `maturity_settlement` (`src/test.rs`), portanto só nos testes.

### 1.3 Interface pública (inventário de referência, igual nos dois lados)
| Contrato | `CONTRACT_VERSION` | `pub fn` | variantes de erro | eventos (ocorrências) | upgrade | `#[test]` |
|---|---|---|---|---|---|---|
| token_sale | 11 | 35 | 29 | 20 | sim (`update_current_contract_wasm`) | 85 |
| maturity_settlement | 5 | 25 | 21 | 2 | sim | 155 |
| yield_distributor | 5 | 17 | 14 | 1 | sim | 72 |
| transfer_agent | 5 | 34 | 14 | 17 | sim | 77 |
| recovery_policy | 2 | 10 | 16 | 6 | não, de propósito ("Sem admin, sem upgrade — de propósito", cabeçalho do `lib.rs`) | 28 |

### 1.4 Datação e atribuição (git)
| Repo | Commit | Data | Autor | Efeito em `contracts/` |
|---|---|---|---|---|
| core | `4398759` | 2026-10-31 | Miguel Cruz | Initial import (traz os 5 crates e `shared`) |
| core | `9414d8f` | 2026-11-01 | — | refactor de arquitetura; não toca em `contracts/`. É o commit de origem que o rc declara |
| core (branch `codex/contracts-extraction`, **fora da main**) | `f432027` | 2026-11-04 12:10:15 | Miguel Cruz | Só docs: `docs/CONTRACTS_EXTRACTION.md` (plano de cutover), AGENTS/README/CORE_ARCHITECTURE |
| rc | `b954648` | 2026-11-04 12:10:15 (mesmo segundo do `f432027`) | Miguel Cruz | Cria o repo: 5 crates + `shared` + `custody_registry` + licença |
| core | `b74f796` | 2026-11-05 11:33 | Pedro Wakigawa Saragossy | PR #1: troca o nome de um projeto terceiro ("F6/F7 do Token Flow") em **comentários** de 4 `lib.rs` (token_sale, maturity_settlement, transfer_agent, yield_distributor; 5 linhas) |
| rc | `c8e2a0f`, `6fca50a`, `d282f7b` | 2026-11-06/07 | pwsaragossy | Só `custody_registry/ARCHITECTURE.md` (SCF #46) |

Facto verificado: `rc@b954648` = `core@9414d8f` + exatamente as 5 linhas de comentário de `core@b74f796` + a linha de licença. O diff `9414d8f→core HEAD` e o diff `9414d8f→rc` (sem os `.toml`) são iguais linha a linha. Isto contradiz o `NOTICE` do rc ("Their source was not modified by the extraction"): o código executável não mudou, mas os comentários vêm já com a troca de nome do `b74f796`, que no Core só entrou um dia depois.

Inferência, não provada: o rc foi gerado ou reescrito com a troca de nome já aplicada, mantendo a data de 11-04. O head do PR #1 (`5c19bd6`) não está no clone local e não foi buscado. Para fechar a dúvida basta `git fetch origin pull/1/head` no Core.

Conclusão: nenhum lado tem mudança funcional mais recente. Depois de `b954648`/`b74f796`, ninguém mexeu nos 5 contratos em nenhum dos repos.

## 2. Build e testes (os dois lados)

Ambiente: rustc 1.86.0 (fixado pelo `rust-toolchain.toml`; com os targets wasm instalados para o efeito), stellar-cli 28.1.0, `custody_registry` com rustc 1.99.1 (sem toolchain fixado na altura; ver §4). Os comandos foram corridos por um script local fora dos repos. A sequência espelha `radox-core/scripts/ci-local.sh` (test default, test testnet, test mainnet, guarda testing+mainnet), mais `stellar contract build --no-default-features --features <rede>`, o comando do `deploy/mainnet-wasm/MANIFEST.txt`.

### 2.1 Testes (iguais nos dois lados)
| Crate | default | testnet | mainnet | guarda testing+mainnet |
|---|---|---|---|---|
| token_sale | 83/0 | 85/0 | 85/0 | recusa (ok) |
| maturity_settlement | 153/0 | 155/0 | 155/0 | recusa (ok) |
| yield_distributor | 68/0 | 70/0 | 70/0 | recusa (ok) |
| transfer_agent | 77/0 | — | — | — (sem features de rede) |
| recovery_policy | 26/0 | 26/0 | 28/0 | recusa (ok) |
| custody_registry (só rc) | 26/0 | — | — | — |

Nota: o `cargo test` reescreve `test_snapshots/` conforme a feature da última corrida (326 ficheiros mudam depois da sequência default→testnet→mainnet). Acontece de igual forma nos dois lados (0 diferenças entre core e rc no fim) e o `ci-local.sh` já o avisa. Não é divergência.

### 2.2 sha256 dos WASM (`stellar contract build`, CLI 28.1.0)
| WASM | radox-core | radox-contracts |
|---|---|---|
| token_sale (testnet) | `f8e61a6803a6b4e3d977375178c2df159d957e94a6d8f5424a5c413556101d04` | igual |
| token_sale (mainnet) | `142f3aa8e2b3844d3638aeb482e372e3612f0ebfd612699f9cc9e6c602e8c32d` | igual |
| maturity_settlement (testnet) | `b7b53e673affad5dd5c3e22d0777ac068a0e9b6b1e9a99104a815fc86f90c99a` | igual |
| maturity_settlement (mainnet) | `09c95edb7b6309173a4f5897680f3e8244e8e9fa16c2ce9807349f3446acaea7` | igual |
| yield_distributor (testnet) | `f6fbba0082eebe0b888e1dd44585a30abe666ceaef987b194817d3c3e4c3f251` | igual |
| yield_distributor (mainnet) | `7b99b8a73dfb10d6b979e634d2f8cbdd3c63253bac89a22a83142429539f5fa8` | igual |
| transfer_agent (default) | `5915649c2fd4ab6aaa93fe90ae718fb8d26826ea39190257f584577cee8e3576` | igual |
| recovery_policy (testnet) | `71b2500b29a785f6c2b76381a986e9e272f635445b15605d45033242eb11049c` | igual |
| recovery_policy (mainnet) | `fc35c2cefd5f6dc3cf510afe8705674d168343b3372752ba1f9ebc6601cf23f7` | igual |
| custody_registry | — | `8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342` |

- Os dois lados foram compilados em caminhos diferentes e deram hashes iguais. A linha de licença não afeta o binário e, neste ambiente, o caminho do checkout também não.
- Estes hashes **não** são os implantados: o `stellar contract build` grava a versão do CLI nos metadados e o fonte HEAD é mais novo do que o que está em produção (token_sale v11 vs. v7 `524ede1c…` vivo e v10 `7909d96f…` staged; maturity_settlement v5 vs. v3 `0f07a598…` vivo; ver `radox-core/deploy/mainnet-wasm/MANIFEST.txt`). Não reconstruí esses binários antigos.
- `custody_registry`: o build do rc HEAD reproduz byte a byte o hash `8230ed93…1342`. Confirmei-o on-chain com `stellar contract fetch` a 2026-11-14: testnet `CCBJVXMJ…4LII` e mainnet `CDV7YI7K…LD4UP` devolvem esse mesmo sha256, com metadados `rsver: 1.99.1`, `rssdkver: 29.0.1#e1671a0c…` e `cliver: 28.1.0#c0f4d0da…`. O "Rust 1.97.1" do README do miami-rio-rail está desatualizado: com 1.97.1 o build dá `b62d8e7d9982e713f01ff32c2722e89f5d53b9cd17334aa720cb94d67256f914`. O PR fixa `1.99.1` em `contracts/custody_registry/rust-toolchain.toml`.

## 3. Decisão por contrato

| Contrato | Canónica | Evidência | Porte do Core antes da consolidação |
|---|---|---|---|
| token_sale | radox-contracts | fonte/testes/lock idênticos; mesmo WASM; rc tem licença declarada; Core já aponta para o rc como dono (`f432027`, branch) | nenhum |
| maturity_settlement | radox-contracts | idem | nenhum |
| yield_distributor | radox-contracts | idem | nenhum |
| transfer_agent | radox-contracts | idem; `miami-rio-rail/AGENTS.md` também o põe no rc | nenhum |
| recovery_policy | radox-contracts | idem | nenhum |
| shared | radox-contracts | idêntico | nenhum |

Não há código do Core a portar. O que falta na consolidação é infraestrutura, não código: os consumidores do Core que leem `contracts/` (lista verificada na main a 2026-11-14):
1. `backend/package.json` → `build:e2e-wasm` compila `../contracts/token_sale` e `../contracts/maturity_settlement`.
2. `backend/tests/e2e/*` (contractManagement, tokenLifecycle, emergencyPauser, dvpVendedorAssina, transferAgentPauser, tokenSaleV11, yieldPorInstalacao, instalacoesInfra) leem `contracts/<crate>/target/.../*.wasm`; `backend/tests/unit/contracts/upgradeProseMatchesCode.test.js` lê fontes.
3. `scripts/check-core-architecture.mjs` (5 referências) e `scripts/ci-local.sh` (`run_contract_crate`, cuja atestação alimenta o gate `deploy/mainnet-build.sh` [0/3]).
4. `.github/workflows/ci.yml` (dormant, 9 referências).
5. `docker-compose.local.yml:40`, `docker-compose.dev.yml:42` e `docker-compose.prod.yml:312` montam `./contracts:/app/contracts:ro`. `deploy/install/provision-chain.mjs` tem um caminho de WASM por omissão.
6. O plano de cutover (`docs/CONTRACTS_EXTRACTION.md`) está **só** no branch `codex/contracts-extraction` (`f432027`), que não foi juntado à main. Na main, o AGENTS.md do Core ainda diz que `contracts/` é dele.

Pontos a corrigir no rc (documentação, não código): o `NOTICE` e o `README` dizem "copiados de `9414d8f` sem modificação", mas os comentários vêm de `b74f796`. Convém registar `b74f796` como base efetiva.

## 4. custody_registry (só em radox-contracts)

**Papel.** Registo de custódia de carga em trânsito (Radox Rail, corredor Rio–Miami). Cada envio (`key = sha256(container|origin)`) segue uma rota fixa. Cada handover exige a assinatura de quem entrega e de quem recebe e é encadeado num `head` SHA-256 que qualquer um pode verificar. A propriedade do token não muda aqui: o burn/redeem é feito pelo `transfer_agent` através de script off-chain (`miami-rio-rail/scripts/lifecycle.mjs`). Origem: `pwsaragossy/radox-rail@0961f83` (NOTICE). Spec aberta em `miami-rio-rail/CUSTODY-SPEC.md`. Preparado para o SCF #46 (`ARCHITECTURE.md`).

**Interface** (`src/lib.rs`, `VERSION = 1`, soroban-sdk 29.0.1):
- Escrita: `__constructor(admin, operator)`; `open(key, token, route, docs_root)` (operator); `handover(key, from, to, evidence_hash, assurance 1..3)` (from + to); `attest(key, source, kind, event_hash)` (source registada); `deliver(key, final_custodian, holder)` (os dois); `freeze`/`unfreeze` (admin; o freeze funciona com o contrato em pausa); `add_source`/`remove_source`, `pause`/`resume`, `set_pauser(Option)` (admin); `emergency_pause` (pauser).
- Leitura: `get`, `head`, `is_paused`, `version`.
- Erros 1–13: WrongCustodian, OutOfRoute, NotOpen, DuplicateKey, UnknownSource, Paused, Frozen, NotFound, BadRoute, NotFinal, NotFrozen, NoPauser, BadAssurance.
- Eventos `#[contractevent]`: Opened, Handover, Attested, Delivered, StatusChanged.
- Storage: instance (Admin, Operator, Paused, Pauser), persistent (Source(addr), Shipment(key)), TTL 120 960 → 518 400 ledgers.
- Implantado: testnet `CCBJVXMJYWPHTDJHDFPRGKPGXGEP24JOPJAMUS5C3BMZJTRTWR2H4LII` (+ sandbox `CBTHLSQX…6ERX`, `CD7BUCY3…NH3F`); mainnet `CDV7YI7KYTDH4LPOLVUVRTFERJKYOWBBT6KS56ZX3V7EOGDBWS7LD4UP` (admin em Ledger), wasm `8230ed93…1342` (§2.2).

**O Core referencia-o?** Não. `git grep` na main do Core por custody_registry, CDV7YI7K, 8230ed93, radox-rail e REGISTRY_ só encontra nomes de fixtures ("Rio-Miami 2026") e o conceito de custódia do investidor, que é outra coisa. O `phygital*` (levantamento físico) é o ponto de integração natural, mas não há código. O README do Rail diz que "a read-only Radox API endpoint already reads this registry in a local development run": não existe na main do Core, logo deve estar num branch ou checkout local que não vi. Hoje o único consumidor é o `miami-rio-rail`, que compila a partir de `../radox-contracts/contracts/custody_registry`.

**Lacunas de cobertura:**
1. **CI:** o rc não tinha `.github/` nem um script equivalente ao `ci-local.sh`. **Este PR acrescenta** `.github/workflows/contracts.yml` (os 6 crates, testes, guarda, build e sha256 por commit). Ainda falta a atestação SEP-0055 (item 3).
2. **Toolchain não fixado:** faltava o `rust-toolchain.toml`. **Resolvido neste PR**: fixa `1.99.1`, a versão gravada no WASM on-chain e a que reproduz `8230ed93…1342` (§2.2). Os outros 5 crates fixam 1.86.0.
3. **Verified build:** não há atestação SEP-0055 / `source_repo` nos metadados. Raven, `skills.stellar-dev.smart-contracts` §verify-your-build, recomenda-a antes de qualquer deploy em mainnet, e este contrato já está em mainnet.
4. **Registo de deploy no rc:** os IDs e hashes estão só no `miami-rio-rail` (DEPLOYMENTS.md/README). O rc não tem `DEPLOYMENTS.md`.
5. **Auditoria:** "v0.1 … has not been audited" (ARCHITECTURE §7). O plano é a SCF Audit Bank.
6. **Notas de segurança/desenho** (para o v2, não bloqueiam a consolidação):
   - Sem `upgrade`, sem troca de admin nem de operator: a instância em mainnet é imutável e perder a chave Ledger do admin congela as funções de admin para sempre.
   - `pause`, `resume`, `set_pauser`, `emergency_pause`, `add_source` e `remove_source` não emitem eventos, o que deixa um buraco para os indexadores.
   - `open` não valida `token` e `deliver` não verifica se o `holder` detém o token (está documentado no README do Rail).
   - `attest` aceita envios Delivered/Frozen ("blocks nothing", de propósito).
7. **SDK misto no repo:** 29.0.1 (custody) vs. 25.3.0 (os outros 5). O AGENTS.md proíbe unificar sem plano, e é correto. Fica a nota de que a SDK 29 corresponde ao protocolo 29 (Raven, §versions) e os 5 crates continuam na 25.

## 5. Riscos
1. **Remover `contracts/` do Core antes de migrar os consumidores do §3** parte o E2E, o `ci-local.sh` e, com ele, o gate de atestação do `deploy/mainnet-build.sh`, além das montagens do compose de produção.
2. **Proveniência dos binários vivos:** nenhum dos repos reproduz hoje os WASM vivos dos 5 contratos (o HEAD é v11/v5, os binários vivos são v7/v3/v4 de commits antigos do Core, alguns com o fonte "não commitado" segundo o MANIFEST). A consolidação não deve reescrever essa história. O `MANIFEST.txt` e o `DEPLOYMENTS.md` do Core continuam a ser a prova.
3. **NOTICE do rc impreciso** quanto ao commit de origem (§1.4). É documental, mas pesa numa due diligence ou no SCF.
4. **custody_registry em mainnet sem CI, pin de toolchain, verified build nem auditoria** (§4).
5. **Snapshots instáveis:** correr `cargo test` com features diferentes suja `test_snapshots/`. O CI deste PR corre a default por último e só avisa da deriva, sem falhar. Mesmo assim, há 2 snapshots commitados desatualizados em `maturity_settlement`: `test_initialize_happy_path` e `test_initialize_different_admin_and_treasury` registam a versão `4`, mas `CONTRACT_VERSION` é `5`. O mesmo acontece no Core. Não os alterei neste PR, para manter a identidade com o Core.

## Fontes
- Git: `radox-core` (`5ef1edc`, `b74f796`, `9414d8f`, `4398759`, branch `f432027`), `radox-contracts` (`b954648`, `c8e2a0f`, `6fca50a`, `d282f7b`), `miami-rio-rail` (`DEPLOYMENTS.md`, `README.md`, `AGENTS.md`, `scripts/lifecycle.mjs`).
- `radox-core/deploy/mainnet-wasm/MANIFEST.txt`, `scripts/ci-local.sh`, `deploy/mainnet-build.sh`.
- Stellar Raven: `skills.stellar-dev.smart-contracts`, secções `verify-your-build` (SEP-0055), `versions` e `before-mainnet` (consultado a 2026-11-14).
