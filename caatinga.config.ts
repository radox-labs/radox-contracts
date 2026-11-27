// Caatinga config — contract/WASM bookkeeping only (testnet-first).
// Restored from radox-core (radox-labs/radox-core#26, brought from radox-internal); paths are now this repo's.
//
// SCOPE (decided 2026-07-02, adopted 2026-07-09 — see radox-core docs/Operations/DEPLOYMENTS.md):
//   OK    build / doctor / status (read-only) over caatinga.artifacts.json
//   NEVER writes .env* files — env stays manual + radox-core scripts/setup.js (single writer)
//   NEVER used for mainnet deploys — mainnet = manual ceremony + plan-deploy-review
//   NEVER `caatinga deploy` for token_sale / maturity_settlement: those deploy PER-OFFER at
//      runtime from the radox-core backend (sorobanSale.service.js / sorobanSettlement.service.js).
//
// caatinga.artifacts.json is GENERATED from deployments/registry.json (the only source):
//   node scripts/gen-caatinga-artifacts.mjs   — and scripts/check-registry.mjs fails if it drifts.
// So no caatinga command that writes artifacts (deploy, upgrade, migrate) is run here.
//
// WARNING BUILD FEATURES: default `cargo build` / `stellar contract build` with no
//    flags produces a `testing`-feature WASM that skips the USDC SAC check —
//    NEVER deploy that. caatinga passes `buildFeatures` verbatim to
//    `stellar contract build`, so every contract with network features PINS the testnet flags.
//    `caatinga build <contract>` can therefore never emit the footgun WASM.
//    caatinga's build features are STATIC per contract (network only affects
//    `generate`, not `build`). These are the TESTNET flags. MAINNET builds use
//    `--features mainnet` and are done MANUALLY in the Ledger ceremony — NOT via
//    `caatinga build` (see the scope guard-rails above).
const TESTNET_BUILD = ["--no-default-features", "--features", "testnet"];

import { defineConfig } from "@caatinga/core";

export default defineConfig({
  project: "radox",
  defaultNetwork: "testnet",
  contracts: {
    token_sale: {
      path: "./contracts/token_sale",
      wasm: "./contracts/token_sale/target/wasm32v1-none/release/token_sale.wasm",
      buildFeatures: TESTNET_BUILD,
    },
    maturity_settlement: {
      path: "./contracts/maturity_settlement",
      wasm: "./contracts/maturity_settlement/target/wasm32v1-none/release/maturity_settlement.wasm",
      buildFeatures: TESTNET_BUILD,
    },
    transfer_agent: {
      path: "./contracts/transfer_agent",
      wasm: "./contracts/transfer_agent/target/wasm32v1-none/release/transfer_agent.wasm",
      // F3 (24/08): o agente não tem gate de rede (F-006) — ele administra os SACs
      // que o admin registrar; o build default serve testnet e mainnet.
      //
      // 09/09: a isenção CONTINUA valendo, e o buraco que ela deixava foi fechado por
      // outro caminho. O `transfer_dvp` (04/09) trouxe uma perna de USDC que a decisão de
      // 24/08 não previa, e ela vinha do CHAMADOR, sem validação. Em vez de copiar o F-006
      // dos outros três — que exigiria features de rede e um WASM por rede, mudando quantos
      // binários a cerimônia manuseia —, o ativo de pagamento passou a ser fixado POR TOKEN
      // no `register_token`, sob a assinatura do admin 2-de-3. O agente segue agnóstico de
      // ativo (a razão original), e escolher "o que é dinheiro" saiu da chave quente do
      // operator. Um WASM, sem feature de rede: `buildFeatures` fica vazio.
      buildFeatures: [],
    },
    yield_distributor: {
      path: "./contracts/yield_distributor",
      wasm: "./contracts/yield_distributor/target/wasm32v1-none/release/yield_distributor.wasm",
      buildFeatures: TESTNET_BUILD,
    },
    // Guardião com espera (24/09): singleton compartilhado por todas as carteiras, sem
    // admin e sem upgrade. A feature de rede fixa o PISO da espera (testnet 60 s para a
    // prova; mainnet 7 dias). Implantação na testnet: radox-core `frontend/guardian-recovery-proof.mjs
    // deploy <wasm> <sha256>` (confere o hash antes de subir), não `caatinga deploy`.
    recovery_policy: {
      path: "./contracts/recovery_policy",
      wasm: "./contracts/recovery_policy/target/wasm32v1-none/release/recovery_policy.wasm",
      buildFeatures: TESTNET_BUILD,
    },
    // Custody registry (Rail): the crate declares no features; one WASM for both networks.
    custody_registry: {
      path: "./contracts/custody_registry",
      wasm: "./contracts/custody_registry/target/wasm32v1-none/release/custody_registry.wasm",
      buildFeatures: [],
    },
  },
  networks: {
    testnet: {
      rpcUrl: "https://soroban-testnet.stellar.org",
      networkPassphrase: "Test SDF Network ; September 2015",
    },
    mainnet: {
      rpcUrl: "https://mainnet.sorobanrpc.com",
      networkPassphrase: "Public Global Stellar Network ; September 2015",
    },
  },
});
