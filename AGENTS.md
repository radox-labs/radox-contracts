# Radox Contracts

Este repositório contém somente o código e os testes dos contratos Soroban. A API, a persistência, os scripts de operação da plataforma e as aplicações web pertencem aos repositórios consumidores.

- Cada crate em `contracts/<nome>/` é uma unidade independente de build e teste. Não unifique lockfiles, versões do SDK ou toolchains sem um plano de compatibilidade de WASM e implantação.
- Preserve ABI, storage layout, autorização, invariantes de dinheiro e eventos. Mudanças exigem testes, revisão de segurança e plano de upgrade/rollback.
- A fonte não prova o que está implantado. Confirme rede, contract ID, WASM hash e registro de cerimônia antes de afirmar estado on-chain.
- Não registre segredos, seed phrases ou chaves privadas.
- Antes de publicar, confirme direitos/licenças dos contratos migrados do Core e faça revisão de exposição de dados.
