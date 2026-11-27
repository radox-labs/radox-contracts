# custody_registry — deployments

Fontes: `radox-labs/miami-rio-rail` (`DEPLOYMENTS.md`, `README.md` §Mainnet, `proofs/mainnet.json`, commit `fd82752`). Wasm reconferido on-chain com `stellar contract fetch` a 2026-11-14.

O código-fonte não prova o que está implantado. Antes de operar, confirme o contract ID e o hash do wasm na rede.

A fonte máquina-legível é [`deployments/registry.json`](../../deployments/registry.json) (ver [`deployments/README.md`](../../deployments/README.md)). Esta página é a narrativa: `node scripts/check-registry.mjs` falha se ela citar um hash ou contract ID que não está no registry, ou se uma entrada `live` do registry faltar aqui. Mude os dois no mesmo commit.

## Wasm

| | |
|---|---|
| sha256 (= install hash on-chain) | `8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342` |
| metadados no wasm on-chain | `rsver 1.99.1`, `rssdkver 29.0.1#e1671a0cb58d4b8c0bb8de115caf080ca01676e8`, `cliver 28.1.0#c0f4d0da891bbf214c08b8c5035ae6db80e9a3bd` |
| reproduzir | `cd contracts/custody_registry && stellar contract build` (rustc fixado em `rust-toolchain.toml` = 1.99.1, stellar-cli 28.1.0) |

## mainnet

| | |
|---|---|
| contract | `CDV7YI7KYTDH4LPOLVUVRTFERJKYOWBBT6KS56ZX3V7EOGDBWS7LD4UP` |
| deployed | 2026-10-26, deploy tx `f481387089e395e8e64f038aee3b00a672708bec76b421b472f2dbdc3435604a` |
| admin (Ledger) | `GAL7VXHAVJKBHOEWKTL3QIJTRUG47ZCY65QZC75FDUOWFPB5ACKPHMGU` |
| operator | `GBNKNDKT7T3PKT7ZLG2PU7KI5ZHYK6JX3AVTHQ4DAYBCU6KJHQ4WFELA` |
| notas | Imutável: o contrato não tem `upgrade` nem troca de admin. Correu um envio de prova com contas da equipa (status final `Delivered`, seq 4). |

## testnet

| | |
|---|---|
| contract | `CCBJVXMJYWPHTDJHDFPRGKPGXGEP24JOPJAMUS5C3BMZJTRTWR2H4LII` (deployed 2026-10-25) |
| admin (exporter) | `GAKTDHZIKXNSO7TIAQJAZPZBDLPKYONHDZYQBPYORQPYAH5UYAE7OAHZ` |
| operator | `GCXYI6RYMJQUNO2VFBPUTZ55WBRFVGBTZBOW3YF65ABLGPNNSHX3J4OF` |
| sandbox (`POST /api/sandbox/shipments`) | `CBTHLSQXQNEZ25UMVRP5TGOCPOTJXLFISJ2EF5NRNITAZPGMMW656ERX` |
| segundo registo (`proofs/three-machine.json`) | `CD7BUCY3DC7GTHRWFRBHNOFPMGVCHFDL4LHKXZNGEJISSGHI24KSNH3F` |
| registo multiempresa (`proofs/multi-chain.json`; `miami-rio-rail` `contracts.lock.json` `multi_chain`, README §multi-chain) | `CDDWEENF6DKM4AHFWDBHQTYPLC3STMVFPXHXYDORLD53WDZM3FF3KJX4` |

Todas as instâncias de testnet usam o mesmo wasm `8230ed93…1342`. Usam chaves de teste, e um reset da testnet apaga-as.
