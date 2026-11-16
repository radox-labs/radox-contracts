# 03 — Porta F1: contagem de testes, equivalência de bytes e custody_registry

Data: 2026-11-16 · Autor: agente blockchain-stellar · Plano: `radox-core` `docs/consolidacao/02-migracao-core.md` §4, F1 (portas F1.2, F1.3 e F1.4; F1.1 = CI verde, ver §1).
Nada foi implantado nem alterado on-chain. Não houve comparação com os WASM vivos/staged do Core (fora do âmbito, sabe-se que não reproduzem do HEAD).

**Resultado:** as 3 verificações passam.

| Porta | Resultado |
|---|---|
| F1.2 contagem de testes | **≥ em todos** os 5 crates × features: contagens **iguais** às do Core (13/13 rodadas `--lib`/testnet/mainnet, mais as 5 rodadas default completas), mais 26 do `custody_registry` (sem par no Core) |
| F1.3 equivalência de bytes | **13/13 sha256 iguais**; interface e meta iguais nos 13 pares |
| F1.4 custody_registry | **reproduz** `8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342` com rustc 1.99.1 + stellar-cli 28.1.0 |

## 0. Entradas e ambiente

| | Valor |
|---|---|
| Candidato `X` | `radox-contracts@913798376f6fb658cff25117793f3c2007dbd390` (`main` à data; `d3da62e` só muda 4 `test_snapshots`, sem efeito em WASM nem em contagens, confirmado pela CI do PR, run `43911456637`) |
| Referência | `radox-core@042cfca946f95835029cd426ebae9cd0f665ce0b` (`main`), pasta `contracts/` |
| Extração | `git archive` de cada commit, com `core.autocrlf=false` (blobs LF, ver nota 1) |
| Máquina | WSL2, Linux 6.18.33.2 x86_64 |
| rustc / cargo | 1.86.0 (`05f9846f8 2025-03-31`) / 1.86.0, lido do `rust-toolchain.toml` de cada crate; `custody_registry`: 1.99.1 (`58cf8f9c3 2026-10-13`) |
| stellar-cli | 28.1.0 (`c0f4d0da891bbf214c08b8c5035ae6db80e9a3bd`), a mesma da CI |
| Diferença de fonte entre os lados | só `license = "Apache-2.0"` acrescentado aos 5 `Cargo.toml`. `src/`, `Cargo.lock`, `rust-toolchain.toml`, `shared/` e `test_snapshots/` são idênticos byte a byte (`diff -r`) em `042cfca` × `9137983`; em `d3da62e` os 4 `test_snapshots` atualizados deixam de ser iguais aos do Core, o que não afeta WASM nem contagens |

Nota 1: o worktree do Core nesta máquina tem `core.autocrlf=true`, e um `git archive` sem `-c core.autocrlf=false` entrega CRLF, o que dá um diff falso em todos os ficheiros. No repositório (`git ls-files --eol`) os ficheiros são `i/lf`.

## 1. F1.2 — contagem de testes por crate e por feature

Comandos do `scripts/ci-local.sh` do Core (`run_contract_crate`): `cargo test --lib` (default = `testing`), depois `cargo test --no-default-features --features testnet|mainnet`, mais o guarda `cargo check --features mainnet` (tem de falhar). Corridos com `--locked` nos dois lados, em cópias descartáveis (o `cargo test` reescreve `test_snapshots/`). Número = linha `test result: ok. N passed` (a soma de todas as linhas dá o mesmo: cada crate só tem um alvo de teste). 0 falhas e 0 ignorados em todas as rodadas.

| Crate | default (`--lib`) Core → rc | testnet Core → rc | mainnet Core → rc | guarda `testing+mainnet` |
|---|---|---|---|---|
| token_sale | 83 → 83 | 85 → 85 | 85 → 85 | rejeitado nos dois |
| maturity_settlement | 153 → 153 | 155 → 155 | 155 → 155 | rejeitado nos dois |
| yield_distributor | 68 → 68 | 70 → 70 | 70 → 70 | rejeitado nos dois |
| recovery_policy | 26 → 26 | 26 → 26 | 28 → 28 | rejeitado nos dois |
| transfer_agent | 77 → 77 | n/a (sem features de rede) | n/a | n/a |
| custody_registry | n/a → 26 | n/a | n/a | n/a |

A rodada default completa (`cargo test --locked`, sem `--lib`, que é a da CI do rc) dá os mesmos números.
A CI do GitHub no `main` (`9137983`, run `43901932399`, `success`) mostra as mesmas contagens (testnet, mainnet e default completo dos 5 crates) e 26 no `custody_registry`.

**Testes opcionais com WASM externo (`yield_distributor --features wasm-tests`, critério de `2b8d700`):** não compilam em nenhum dos lados, com o mesmo erro: `error[E0063]: missing field 'tokens' in initializer of 'SettleItem'` em `src/test.rs` (módulo `birth_to_death`). O teste ficou para trás em relação à API do `maturity_settlement`. Foram corridos com os pré-requisitos dos comentários: `deploy/mainnet-wasm/yield_distributor.wasm` do Core (sha256 `88509c72…4203`) no caminho relativo esperado e `maturity_settlement` construído para `wasm32v1-none --release`. É um defeito pré-existente e igual nos dois lados, por isso não bloqueia a equivalência. Corrigi-lo fica para um PR à parte (não é só docs). Fora do Core, o `worst_case_30_investors_fits_mainnet_wasm` precisa também desse artefacto, que não está neste repo.

## 2. F1.3 — equivalência de bytes (mesmo ambiente, mesmo caminho absoluto)

Método: cada lado foi copiado para o **mesmo caminho absoluto** (`~/.cache/radox-consol/f1/build/contracts/<crate>`, cópia real), construído com `stellar contract build --locked [--no-default-features --features <rede>] --out-dir …`, e a árvore e o `target/` foram apagados antes do outro lado (sem reaproveitar fingerprints do cargo). O lado do Core foi construído uma segunda vez do zero e deu os mesmos 13 hashes (o ambiente é determinístico).

| Crate | Features | sha256 (Core = rc) | bytes | Igual | interface + meta |
|---|---|---|---|---|---|
| token_sale | default (testing) | `4ceba7f388d211078de0dfc5a2006261704b0a38e52a058676c6d86bed40526c` | 34391 | sim | iguais |
| token_sale | testnet | `f8e61a6803a6b4e3d977375178c2df159d957e94a6d8f5424a5c413556101d04` | 34506 | sim | iguais |
| token_sale | mainnet | `142f3aa8e2b3844d3638aeb482e372e3612f0ebfd612699f9cc9e6c602e8c32d` | 34506 | sim | iguais |
| maturity_settlement | default (testing) | `e47fc85744dbb52305285f0692442b4d60b46988fc4715b44f3b9709b327996a` | 22547 | sim | iguais |
| maturity_settlement | testnet | `b7b53e673affad5dd5c3e22d0777ac068a0e9b6b1e9a99104a815fc86f90c99a` | 22657 | sim | iguais |
| maturity_settlement | mainnet | `09c95edb7b6309173a4f5897680f3e8244e8e9fa16c2ce9807349f3446acaea7` | 22657 | sim | iguais |
| yield_distributor | default (testing) | `3994b6eccc12f8770d8fd1981ffdabbb728115ac8e32664be184e32b301cbb55` | 10246 | sim | iguais |
| yield_distributor | testnet | `f6fbba0082eebe0b888e1dd44585a30abe666ceaef987b194817d3c3e4c3f251` | 10365 | sim | iguais |
| yield_distributor | mainnet | `7b99b8a73dfb10d6b979e634d2f8cbdd3c63253bac89a22a83142429539f5fa8` | 10365 | sim | iguais |
| recovery_policy | default (testing) | `153671657e4057012b3eb5b0305aef424460fa920877504439216f1f38b55f49` | 19137 | sim | iguais |
| recovery_policy | testnet | `71b2500b29a785f6c2b76381a986e9e272f635445b15605d45033242eb11049c` | 19137 | sim | iguais |
| recovery_policy | mainnet | `fc35c2cefd5f6dc3cf510afe8705674d168343b3372752ba1f9ebc6601cf23f7` | 19147 | sim | iguais |
| transfer_agent | default | `5915649c2fd4ab6aaa93fe90ae718fb8d26826ea39190257f584577cee8e3576` | 28199 | sim | iguais |

Interface: `stellar contract info interface --wasm … --output rust`. Meta: `stellar contract info meta` (`rsver 1.86.0`, `rssdkver 25.3.0#dcbea445…`, `cliver 28.1.0#c0f4d0da…`). Os ficheiros de saída dos dois lados foram comparados com `cmp`: 13 pares, 0 diferenças.

Os 9 hashes de testnet, de mainnet e do `transfer_agent` que a CI do GitHub publica no resumo do run `43901932399` (runner Ubuntu, caminho `/home/runner/work/radox-contracts/radox-contracts/contracts/<crate>`) são **iguais** aos desta tabela. A CI não constrói o default (testing) dos 4 crates com rede. Com rustc 1.86.0 e stellar-cli 28.1.0, um caminho absoluto diferente não mudou o hash. Não se pode generalizar para outras versões, porque o fato 5 do plano (`MANIFEST.txt`) foi observado com outras versões da CLI.

## 3. F1.4 — custody_registry

`custody_registry@9137983`, com o toolchain fixado em `rust-toolchain.toml` (1.99.1, `wasm32v1-none`) e `stellar contract build --locked` (stellar-cli 28.1.0):

- sha256 `8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342` (10672 bytes) = o valor de `contracts/custody_registry/DEPLOYMENTS.md`. **Reproduz.**
- Meta: `rsver 1.99.1`, `rssdkver 29.0.1#e1671a0c…`, `rssdk_spec_shaking 2`, `cliver 28.1.0#c0f4d0da…`.
- A CI do GitHub do mesmo commit dá o mesmo hash (run `43901932399`).
- Com o hash igual, a interface é por construção a do WASM on-chain. Não houve `stellar contract fetch` nesta tarefa. A confirmação on-chain do mesmo sha256 (testnet `CCBJVXMJ…4LII`, mainnet `CDV7YI7K…LD4UP`) está em `01-diff-contratos.md` §2.2 (2026-11-14).

## 4. Observações (sem ação nesta porta)

- `recovery_policy`: o build testnet tem o mesmo tamanho que o default (19137 bytes, hashes diferentes) e corre os mesmos 26 testes; o mainnet corre 28. É igual nos dois lados, por isso não afeta a equivalência. Fica registado para quem rever as features de rede desse crate.
- A CI do rc não constrói o WASM default (testing). A porta F1.3 inclui-o; se a F2 quiser repetir a equivalência na CI, falta esse build.

## 5. Reproduzir

```sh
# fontes (LF)
git -C <radox-core> -c core.autocrlf=false archive 042cfca contracts | tar -x -C src/core
git -C <radox-contracts> -c core.autocrlf=false archive 9137983 contracts | tar -x -C src/rc
# OUT absoluto e fora de build/ (build/ é apagado entre lados), ex.: OUT=$HOME/.cache/radox-consol/f1/out
# por lado, no MESMO caminho, árvore e target apagados entre lados (rm -rf build):
cp -a src/<lado>/contracts build/contracts && cd build/contracts/<crate>
stellar contract build --locked --out-dir "$OUT/<lado>/default"
stellar contract build --locked --no-default-features --features testnet --out-dir "$OUT/<lado>/testnet"
stellar contract build --locked --no-default-features --features mainnet --out-dir "$OUT/<lado>/mainnet"
sha256sum "$OUT"/*/*/*.wasm
stellar contract info interface --wasm <f> --output rust; stellar contract info meta --wasm <f>
```
