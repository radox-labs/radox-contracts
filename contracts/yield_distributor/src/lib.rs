#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype,
    symbol_short, token, Address, BytesN, Env, Map, Vec,
};

// v3 (May 2026, F-004 audit follow-up): 2-step admin rotation.
// v4 (Jun 2026, security review F-SOR-001 / F-SOR-011 follow-up):
//   - canonical USDC SAC hardcoded per network + validated in distribute()
//     (parity with token_sale v7 / maturity_settlement v3 F-006 hardening).
//   - initialize() replaced by __constructor — init is now atomic with deploy,
//     closing the front-run / unauthorized-init gap (F-SOR-011).
//   The `testing` feature disables the USDC check so unit tests can use
//   generated SACs; production WASMs build with
//   `--no-default-features --features testnet|mainnet`.
//   See docs/Operations/soroban_security_review_2026-06.md.
// v5 (Out 2026): papel `emergency_pauser` SEPARADO do admin. Chave NOVA e
//   OPCIONAL no storage (`DataKey::EmergencyPauser`): instancia v4 sem a chave continua
//   funcionando sem migracao (ausente = so o admin pausa, como hoje). Nenhuma chave existente
//   foi renomeada nem mudou de layout. O pauser so LIGA a mesma pausa (`emergency_pause`);
//   nao despausa, nao troca admin, nao faz upgrade, nao move dinheiro. O pauser pode RENUNCIAR
//   (`renounce_emergency_pauser`, so ele assina) e TODA troca (definir, trocar, remover,
//   renunciar) emite `pauserset` (anterior, novo). Trocar o admin NAO zera o pauser.
const CONTRACT_VERSION: u32 = 5;

// Canonical USDC Stellar Asset Contract — F-006 (must match token_sale /
// maturity_settlement). IMPORTANT: when changing, also update
// docs/Operations/DEPLOYMENTS.md.
#[cfg(feature = "testnet")]
const USDC_SAC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
#[cfg(feature = "mainnet")]
const USDC_SAC: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";

// PEND 159 (23/08): a escolha de rede e exclusiva e obrigatoria. `testing` (default do
// `cargo test`) DESLIGA a validacao do USDC canonico — combinada com `testnet`/`mainnet`
// ela produz um WASM de producao sem o gate F-006. O build correto e sempre
// `--no-default-features --features testnet|mainnet`; qualquer outra combinacao nao compila.
#[cfg(all(feature = "testing", any(feature = "testnet", feature = "mainnet")))]
compile_error!("feature `testing` junto com `testnet`/`mainnet`: use --no-default-features --features testnet|mainnet");
#[cfg(all(feature = "testnet", feature = "mainnet"))]
compile_error!("features `testnet` e `mainnet` sao mutuamente exclusivas");
#[cfg(not(any(feature = "testing", feature = "testnet", feature = "mainnet")))]
compile_error!("escolha uma rede: --features testnet|mainnet (ou o default `testing` para cargo test)");

// ═══════════════════════════════════════════════════════════════
//  Storage layout — flat keys (no Config struct = easier upgrades)
//
//  DataKey::Admin        → Address   (high-privilege: upgrade, pause, propose_admin)
//  DataKey::PendingAdmin → Address   (v3: 2-step rotation, awaiting accept)
//  DataKey::Paused       → bool      (circuit breaker)
//  DataKey::EmergencyPauser → Address (v5: opcional; so liga a pausa, nunca despausa)
//
//  v3 (May 2026, F-004 security audit follow-up): 2-step admin rotation
//  via propose_admin + accept_admin. The legacy set_admin remains for
//  back-compat but is marked deprecated.
// ═══════════════════════════════════════════════════════════════

/// TTL constants — same as maturity_settlement.
const TTL_THRESHOLD: u32 = 17_280;  // ~1 day at 5s ledgers
const TTL_EXTEND: u32 = 518_400;    // ~30 days

/// Maximum investors per distribute() call.
/// At ~12M CPU per SAC.transfer, 30 × 12M = 360M, within 600M budget.
/// Footprint: 30 investors × 2 reads/writes + base ≈ 64 entries (fits ~200 limit).
/// Same batch size as maturity_settlement for consistency.
///
/// ⚠️ §17.6 (09/09/2026): ESTE RACIOCINIO NAO E O LIMITE REAL. A PENDENCIAS 175 MEDIU a rede
/// recusando acima de **26** itens, e a trava e o `txMemoryLimit` de 40 MB — que NAO entra
/// no footprint nem no orcamento de CPU acima. Simulacao passa e a rede recusa:
/// `settle_batch` declara 25M de 400M instrucoes e 92 de 200 write entries, e morre assim
/// mesmo. Recurso declarado nao e o teto.
///
/// O backend ja compensa fatiando em 20 (`sorobanSettlement.service.js:64`), entao a
/// constante 30 nao morde hoje. Baixa-la exige WASM nova -> CERIMONIA da Ledger, decisao do
/// Pedro. O que da para consertar sem cerimonia e o MODELO MENTAL: quem ler as linhas acima
/// e mexer no numero vai calcular por CPU/footprint e errar de novo.
const MAX_BATCH_SIZE: u32 = 30;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
    /// v3: address proposed by current admin via propose_admin().
    /// Cleared on accept_admin().
    PendingAdmin,
    /// v5 (F6): quem pode LIGAR a pausa em emergencia, e so isso. Opcional: ausente = ninguem
    /// alem do admin pausa (instancias v4 nao tem a chave). Chave nova — nao renomeia nada.
    EmergencyPauser,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DistributeError {
    EmptyBatch = 1,
    BatchTooLarge = 2,
    InvalidAmount = 3,
    Overflow = 4,
    MismatchedArrays = 5,
    /// Fee exceeds 70% of total payout — safety cap
    FeeTooHigh = 6,
    /// Contract already initialized
    AlreadyInitialized = 7,
    /// Contract not yet initialized
    NotInitialized = 8,
    /// Contract is paused
    ContractPaused = 9,
    /// Duplicate recipient in the batch
    DuplicateRecipient = 10,
    /// Payer cannot be a recipient (self-transfer)
    SelfTransfer = 11,
    /// v3: accept_admin() called but no propose_admin() is pending
    NoPendingAdmin = 12,
    /// v4 (F-SOR-001): supplied token is not the canonical USDC SAC for this network.
    UnauthorizedToken = 13,
    /// v5 (F6): emergency_pause() chamado sem `emergency_pauser` configurado.
    NoEmergencyPauser = 14,
}

#[contract]
pub struct YieldDistributor;

// ─── Helpers ─────────────────────────────────────────────────

fn is_initialized(env: &Env) -> bool {
    env.storage().instance().has(&DataKey::Admin)
}

fn load_admin(env: &Env) -> Result<Address, DistributeError> {
    env.storage()
        .instance()
        .get(&DataKey::Admin)
        .ok_or(DistributeError::NotInitialized)
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// v4 (F-SOR-001) — verify the supplied token matches the canonical USDC SAC
/// for this network. In `testing` feature builds the check is a no-op so unit
/// tests can use generated SACs. Production builds (testnet / mainnet feature)
/// enforce the check.
#[allow(unused_variables)]
fn validate_canonical_usdc(env: &Env, token: &Address) -> Result<(), DistributeError> {
    #[cfg(not(feature = "testing"))]
    {
        let canonical = Address::from_string(&soroban_sdk::String::from_str(env, USDC_SAC));
        if *token != canonical {
            return Err(DistributeError::UnauthorizedToken);
        }
    }
    Ok(())
}

/// Emit helper — wraps the deprecated API with a suppression.
#[allow(deprecated)]
fn emit<D: soroban_sdk::IntoVal<Env, soroban_sdk::Val>>(
    e: &Env,
    topic: soroban_sdk::Symbol,
    data: D,
) {
    e.events().publish((topic,), data);
}

// ═══════════════════════════════════════════════════════════════
//  Contract implementation
//
//  Lifecycle:
//    deploy → initialize(admin) → distribute(...) / pause / resume / upgrade
//
//  Auth model:
//    - distribute(): payer.require_auth() — company passkey signs
//    - admin functions: admin.require_auth() — platform key signs
//    - extend_ttl(): no auth — anyone (cron jobs)
// ═══════════════════════════════════════════════════════════════

#[contractimpl]
impl YieldDistributor {
    // ─── Admin lifecycle ──────────────────────────────────────

    /// v4 (F-SOR-011) — Contract constructor. Runs exactly once, atomically, as
    /// part of the deploy operation. This closes the front-run / unauthorized-init
    /// gap a separate `initialize()` had: there is no window in which an attacker
    /// can set the admin before the deployer does.
    ///
    /// `admin.require_auth()` ensures the admin address consents to being set
    /// (prevents installing an admin the deployer does not control). The deploy
    /// transaction must therefore carry the admin's authorization.
    pub fn __constructor(env: Env, admin: Address) {
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    }

    /// Upgrade contract WASM. Admin only (high-privilege).
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Pause the contract. Admin only. Blocks all distribute() calls.
    pub fn pause(env: Env) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        emit(&env, symbol_short!("paused"), true);
        Ok(())
    }

    /// Resume the contract. Admin only. Unblocks distribute() calls.
    pub fn resume(env: Env) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        emit(&env, symbol_short!("resumed"), true);
        Ok(())
    }

    /// v5 (F6) — Define (ou remove, com `None`) o `emergency_pauser`. Admin only.
    /// Trocar o pauser invalida o anterior no ato: `emergency_pause` le a chave a cada chamada.
    /// TODA troca emite `pauserset` com (anterior, novo) — e o que o monitor da instalacao e o
    /// fornecedor leem para saber que o pauser mudou.
    pub fn set_emergency_pauser(
        env: Env,
        pauser: Option<Address>,
    ) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        let anterior: Option<Address> = env.storage().instance().get(&DataKey::EmergencyPauser);
        match &pauser {
            Some(p) => env.storage().instance().set(&DataKey::EmergencyPauser, p),
            None => env.storage().instance().remove(&DataKey::EmergencyPauser),
        }
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("pauserset"), (anterior, pauser));
        Ok(())
    }

    /// v5 (F6) — O PROPRIO pauser renuncia: SO ele assina (o admin nao renuncia por ele — o admin
    /// remove com `set_emergency_pauser(None)`). Remove a chave; dai em diante `emergency_pause`
    /// cai em `NoEmergencyPauser` ate o admin definir outro. Emite `pauserset` (anterior, None)
    /// e `pauserren` (quem renunciou). Nao mexe em pausa, admin nem dinheiro.
    pub fn renounce_emergency_pauser(env: Env) -> Result<(), DistributeError> {
        let pauser: Address = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyPauser)
            .ok_or(DistributeError::NoEmergencyPauser)?;
        pauser.require_auth();
        env.storage().instance().remove(&DataKey::EmergencyPauser);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(
            &env,
            symbol_short!("pauserset"),
            (Some(pauser.clone()), None::<Address>),
        );
        emit(&env, symbol_short!("pauserren"), pauser);
        Ok(())
    }

    /// v5 (F6) — Pausa de emergencia: SO o `emergency_pauser` assina. Liga a MESMA pausa de
    /// `pause()` (bloqueia distribute()). NAO despausa (`resume` e so do admin), NAO move
    /// dinheiro, NAO troca admin, NAO faz upgrade — esta funcao nao tem outro efeito.
    /// Idempotente: pausar contrato ja pausado nao falha.
    pub fn emergency_pause(env: Env) -> Result<(), DistributeError> {
        let pauser: Address = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyPauser)
            .ok_or(DistributeError::NoEmergencyPauser)?;
        pauser.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("paused"), true);
        emit(&env, symbol_short!("emergpaus"), pauser);
        Ok(())
    }

    /// Transfer admin role in one step. Current admin must authorize.
    ///
    /// **Deprecated** in v3 — prefer the 2-step `propose_admin` + `accept_admin`
    /// flow which requires the new admin to prove they hold the keypair
    /// (prevents transfer-to-typo'd-address footgun).
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        // Clear any stale propose-admin proposal to avoid a misleading state.
        env.storage().instance().remove(&DataKey::PendingAdmin);
        emit(&env, symbol_short!("admchg"), new_admin);
        Ok(())
    }

    /// v3 — Step 1 of admin rotation. Current admin proposes a new admin.
    /// The new admin must call accept_admin() to take ownership.
    /// Overwrites any prior pending proposal.
    pub fn propose_admin(env: Env, new_admin: Address) -> Result<(), DistributeError> {
        let admin = load_admin(&env)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
        emit(&env, symbol_short!("propadm"), new_admin);
        Ok(())
    }

    /// v3 — Step 2 of admin rotation. Pending admin accepts ownership.
    /// The pending admin must sign — proves they hold the keypair.
    pub fn accept_admin(env: Env) -> Result<(), DistributeError> {
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(DistributeError::NoPendingAdmin)?;
        pending.require_auth();
        env.storage().instance().set(&DataKey::Admin, &pending);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        emit(&env, symbol_short!("admchg"), pending);
        Ok(())
    }

    /// Extend contract instance TTL. Anyone can call (allows cron jobs).
    pub fn extend_ttl(env: Env) {
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    }

    // ─── Core: Batched USDC distribution ─────────────────────

    /// Distribute USDC from `payer` to multiple `recipients` in a single transaction.
    ///
    /// **Stateful-minimal**: stores admin + paused flag only.
    /// The contract's primary purpose is to batch SAC.transfer() calls under one
    /// require_auth(payer) invocation tree — enabling one passkey prompt
    /// for N investor payments.
    ///
    /// ```text
    ///   Company Wallet (payer)
    ///        │
    ///        ├──► Investor 1  (amounts[0])
    ///        ├──► Investor 2  (amounts[1])
    ///        ├──► ...
    ///        ├──► Investor N  (amounts[N-1])
    ///        │
    ///        └──► Treasury    (fee_amount)  ← skipped if fee == 0
    /// ```
    ///
    /// Auth model:
    ///   - `payer.require_auth()` is called once at the top.
    ///   - Each `token.transfer(payer, recipient, amount)` creates a
    ///     sub-invocation automatically included in the auth tree.
    ///   - Simulation builds the full tree; passkey signs the root.
    ///
    /// # Arguments
    /// - `payer`: Company smart wallet (C... address). Signs via passkey.
    /// - `token`: USDC SAC contract address.
    /// - `recipients`: Investor addresses (C... or G...).
    /// - `amounts`: Per-investor USDC amounts in stroops (i128).
    /// - `fee_recipient`: Platform treasury address.
    /// - `fee_amount`: Total platform fee in stroops (i128). 0 = no fee.
    /// REENTRANCY SAFETY: Soroban's host is single-threaded and cross-contract
    /// calls cannot re-enter the caller. That is what makes this function safe.
    ///
    /// §38.3 (09/09/2026): this comment used to add that the function "ALSO
    /// follows checks-effects-interactions — all state writes land before the
    /// external token transfers". Here that was VACUOUS rather than false:
    /// `distribute` writes no state at all (this contract is stateful-minimal —
    /// only `admin` and `paused`), so there is no ordering to keep. The same
    /// paragraph was pasted into `maturity_settlement::settle_batch`, where it
    /// IS false. A claim that is vacuous in one place and false in another is
    /// not a defense; it is a habit.
    pub fn distribute(
        env: Env,
        payer: Address,
        token: Address,
        recipients: Vec<Address>,
        amounts: Vec<i128>,
        fee_recipient: Address,
        fee_amount: i128,
    ) -> Result<(), DistributeError> {
        // ── Pre-checks ──────────────────────────────────────────
        if !is_initialized(&env) {
            return Err(DistributeError::NotInitialized);
        }
        if is_paused(&env) {
            return Err(DistributeError::ContractPaused);
        }

        // v4 (F-SOR-001): only the canonical USDC SAC may be distributed —
        // a fake/typo'd token would otherwise pay investors in a worthless asset.
        validate_canonical_usdc(&env, &token)?;

        // ── Validation ──────────────────────────────────────────
        let count = recipients.len();

        if count == 0 {
            return Err(DistributeError::EmptyBatch);
        }
        if count > MAX_BATCH_SIZE {
            return Err(DistributeError::BatchTooLarge);
        }
        if count != amounts.len() {
            return Err(DistributeError::MismatchedArrays);
        }
        if fee_amount < 0 {
            return Err(DistributeError::InvalidAmount);
        }

        // Duplicate recipient check + validate amounts + compute total
        let mut seen: Map<Address, bool> = Map::new(&env);
        let mut total_payout: i128 = 0;

        for i in 0..count {
            let recipient = recipients.get(i).unwrap();
            let amt = amounts.get(i).unwrap();

            // No self-transfers (payer paying themselves)
            if recipient == payer {
                return Err(DistributeError::SelfTransfer);
            }

            // Duplicate detection
            if seen.contains_key(recipient.clone()) {
                return Err(DistributeError::DuplicateRecipient);
            }
            seen.set(recipient, true);

            if amt <= 0 {
                return Err(DistributeError::InvalidAmount);
            }
            total_payout = total_payout
                .checked_add(amt)
                .ok_or(DistributeError::Overflow)?;
        }

        // Fee safety cap: fee ≤ 70% of total payout
        if fee_amount > 0 && total_payout > 0 {
            let max_fee = total_payout * 7 / 10; // 70%
            if fee_amount > max_fee {
                return Err(DistributeError::FeeTooHigh);
            }
        }

        // ── Auth ────────────────────────────────────────────────
        // Single require_auth covers the root invocation.
        // All SAC.transfer sub-invocations are automatically included
        // in the auth tree via Soroban's sub-contract call authorization.
        payer.require_auth();

        // ── Execute transfers ───────────────────────────────────
        let usdc = token::Client::new(&env, &token);

        for i in 0..count {
            let recipient = recipients.get(i).unwrap();
            let amount = amounts.get(i).unwrap();
            usdc.transfer(&payer, &recipient, &amount);
        }

        // ── Fee to treasury (skip if zero) ──────────────────────
        if fee_amount > 0 {
            usdc.transfer(&payer, &fee_recipient, &fee_amount);
        }

        // ── Rich event: (payer, count, total_payout, fee_amount) ─
        emit(&env, symbol_short!("distrib"), (payer, count, total_payout, fee_amount));
        Ok(())
    }

    // ─── Read-only ────────────────────────────────────────────

    /// Returns the contract version.
    pub fn version(_env: Env) -> u32 {
        CONTRACT_VERSION // v5 (F6): emergency_pauser; v4 (F-SOR-001/011): canonical USDC + __constructor
    }

    /// Returns the admin address.
    pub fn get_admin(env: Env) -> Result<Address, DistributeError> {
        load_admin(&env)
    }

    /// Returns whether the contract is paused.
    pub fn get_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// v5 (F6) — O `emergency_pauser` configurado, ou None.
    pub fn emergency_pauser(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::EmergencyPauser)
    }

    /// v3 — Returns the pending admin if propose_admin() was called and not yet accepted.
    pub fn get_pending_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::PendingAdmin)
    }
}

mod test;
