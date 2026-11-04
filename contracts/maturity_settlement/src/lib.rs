#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype,
    symbol_short, token, Address, BytesN, Env, Map, String, Vec,
};

// v2 (May 2026): adds pause/resume + 2-step admin rotation (propose_admin/accept_admin).
// v3 (May 2026, F-006 audit follow-up): canonical USDC SAC is hardcoded per
// network and validated at initialize(). Prevents address-poisoning attacks
// where a malicious or compromised admin could initialize the contract with
// a fake USDC SAC. The `testing` feature disables the check so unit tests
// can use generated SACs; production WASMs are built with
// `--no-default-features --features testnet|mainnet`.
// See docs/Operations/security_audit_stellar37.md F-003, F-006.
//
// v5 (Out 2026): papel `emergency_pauser` SEPARADO do admin. Chave NOVA e
// OPCIONAL no storage (`DataKey::EmergencyPauser`, ultima variante): instancia v4 sem a chave
// continua funcionando sem migracao (ausente = so o admin pausa). Nenhuma chave existente foi
// renomeada nem mudou de layout (`Config`, `Admin`, `Paused`... intactos). O pauser so LIGA a
// mesma pausa (`emergency_pause`); nao despausa, nao troca admin, nao faz upgrade, nao move
// dinheiro. O pauser pode RENUNCIAR (`renounce_emergency_pauser`, so ele assina) e TODA troca
// (definir, trocar, remover, renunciar) emite `pauserset` (anterior, novo). Trocar o admin NAO zera o
// pauser. O QUE A PAUSA BLOQUEIA nao mudou: deposit, settle_batch, withdraw, finalize_round,
// claim_residual, next_round e refund. Continuam abertos: upgrade, resume, propose/accept_admin,
// extend_ttl e as leituras.
const CONTRACT_VERSION: u32 = 5;
// ~30 days at 5s per ledger
const TTL_THRESHOLD: u32 = 518_400;
const TTL_EXTEND: u32 = 518_400;

// Canonical USDC Stellar Asset Contract — F-006.
// IMPORTANT: when changing, also update docs/Operations/DEPLOYMENTS.md.
#[cfg(feature = "testnet")]
const USDC_SAC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";

// Mainnet USDC SAC — verified 2026-05-29 against:
//   - Stellar Docs (x402): https://developers.stellar.org/docs/build/agentic-payments/x402
//   - Issuer GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN (Circle)
//   - CLI: stellar contract id asset --asset USDC:GA5ZS... --network mainnet
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
/// Maximum investors per settle_batch() call.
/// At ~12M CPU per investor, 30 × 12M = 360M, within 600M budget.
/// Footprint: 30 investors × 4 reads + 4 base = 124 entries (62% of ~200 limit).
/// Reduced from 40 in R4 review for mainnet safety margin.
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

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SettleError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 3,
    Overflow = 4,
    EmptyBatch = 5,
    /// Investor already settled (in this or a previous batch).
    AlreadySettled = 6,
    BatchTooLarge = 7,
    NoDeposit = 8,
    /// Duplicate investor address within the same batch.
    DuplicateInvestor = 9,
    /// Payout to address holding zero tokens.
    PhantomInvestor = 10,
    /// Fee exceeds the max_fee_bps cap declared at initialization.
    FeeTooHigh = 11,
    /// Contract is paused — settle/deposit/withdraw/refund blocked.
    ContractPaused = 12,
    /// accept_admin() called but no propose_admin() is pending.
    NoPendingAdmin = 13,
    /// v3 (F-006): supplied usdc_sac is not the canonical USDC SAC for this network.
    UnauthorizedToken = 14,
    InsufficientDeposit = 15,
    RoundClosed = 16,
    RoundOpen = 17,
    WrongState = 18,
    ResidualPending = 19,
    TokensExceedBalance = 20,
    /// v5 (F6): emergency_pause() chamado sem `emergency_pauser` configurado.
    NoEmergencyPauser = 21,
}

/// Per-offer contract state keys.
/// Each deployed contract instance = one offer.
/// No offer_id needed — the contract address IS the offer scope.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    /// Tracks USDC deposited by a specific address (typically the company).
    Deposit(Address),
    /// v4: per-investor settlement flag, once per ROUND (not once forever).
    /// Stored in persistent storage. Prevents double-payout across batches.
    Settled(u32, Address),
    /// Counter of investors settled in the CURRENT round. Guard for refund()
    /// and upgrade(). Instance storage; reset by next_round().
    SettledCount,
    /// v4: current round number (u32, starts at 1).
    Round,
    /// v4: whether the current round was finalized.
    RoundClosed,
    /// v4: USDC deposited this round and still owed back to depositors.
    DepositTotal,
    /// v4: USDC left in the contract when the round was finalized.
    ResidualSnapshot,
    /// v4: deposit total at finalize — the pro-rata base for claim_residual().
    DepositTotalSnapshot,
    /// Active admin override. Set after a successful accept_admin().
    /// If absent (fresh-init contract), Config.admin is the active admin.
    /// Added in v2.
    Admin,
    /// Admin proposed by current admin via propose_admin(), awaiting
    /// new admin's accept_admin() call. Cleared on acceptance.
    /// Added in v2.
    PendingAdmin,
    /// Pause flag — when true, deposit/settle_batch/withdraw/refund return
    /// ContractPaused. Upgrade and admin-rotation are intentionally
    /// NOT gated (the recovery path must remain accessible).
    /// Added in v2.
    Paused,
    /// v5 (F6): quem pode LIGAR a pausa em emergencia, e so isso. Opcional: ausente = ninguem
    /// alem do admin pausa (instancias v4 nao tem a chave). Chave nova — nao renomeia nada.
    EmergencyPauser,
}

/// Immutable configuration set once during initialize().
///
/// `admin`: issuer public key — must be SAC admin for both USDC (transfer) and
///          security token (clawback). Signs settle_batch() and withdraw().
/// `usdc_sac`: USDC SAC contract address (for payouts + fee routing).
/// `token_sac`: security token SAC contract address (for clawback/burn).
/// `treasury`: platform treasury address (receives aggregated fees).
/// `max_fee_bps`: maximum fee in basis points (e.g. 200 = 2%). Immutable after init.
///                Enforced on-chain: total_fee ≤ sum(payouts) × max_fee_bps / 10_000.
///                CVM transparency: auditors can verify fee cap on-chain.
#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub admin: Address,
    pub usdc_sac: Address,
    pub token_sac: Address,
    pub treasury: Address,
    pub max_fee_bps: u32,
}

/// One item in a settlement batch — represents a single investor's payout.
///
/// Clawback is AUTOMATIC: the contract reads the investor's on-chain token
/// balance and burns ALL of it. No clawback_amount needed — the chain is
/// the source of truth.
///
/// ```text
///   investor ◄── payout USDC ── contract
///   investor ──► ALL tokens clawbacked (read from chain)
/// ```
#[contracttype]
#[derive(Clone)]
pub struct SettleItem {
    pub investor: Address,
    pub payout: i128,
    /// v4: tokens to burn from the investor. `i128::MAX` = the whole balance (v3
    /// bullet semantics); any other value must be 0 < tokens <= balance.
    pub tokens: i128,
}

#[contract]
pub struct MaturitySettlement;

/// Emit helper — wraps the deprecated API with a suppression.
/// When soroban-sdk stabilizes #[contractevent], migrate these calls.
#[allow(deprecated)]
fn emit<D: soroban_sdk::IntoVal<Env, soroban_sdk::Val>>(
    e: &Env,
    topic: soroban_sdk::Symbol,
    data: D,
) {
    e.events().publish((topic,), data);
}

/// Returns the contract's pause flag. Default false (fresh deploys).
fn current_round(env: &Env) -> u32 {
    env.storage().instance().get(&DataKey::Round).unwrap_or(1)
}
fn round_closed(env: &Env) -> bool {
    env.storage().instance().get(&DataKey::RoundClosed).unwrap_or(false)
}
fn deposit_total(env: &Env) -> i128 {
    env.storage().instance().get(&DataKey::DepositTotal).unwrap_or(0)
}
fn set_deposit_total(env: &Env, v: i128) {
    env.storage().instance().set(&DataKey::DepositTotal, &v);
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

/// v3 (F-006) — verify the supplied usdc_sac matches the canonical USDC SAC
/// for this network. In `testing` feature builds the check is a no-op so unit
/// tests can use generated SACs. Production builds (testnet / mainnet
/// feature) enforce the check.
#[allow(unused_variables)]
fn validate_canonical_usdc(env: &Env, usdc_sac: &Address) -> Result<(), SettleError> {
    #[cfg(not(feature = "testing"))]
    {
        let canonical = Address::from_string(&String::from_str(env, USDC_SAC));
        if *usdc_sac != canonical {
            return Err(SettleError::UnauthorizedToken);
        }
    }
    Ok(())
}

/// Returns the address currently authorized as admin.
///
/// v1 → v2 upgrade-safe: if `DataKey::Admin` is unset (never rotated),
/// falls back to `Config.admin` (which v1 set during initialize()).
/// After a successful `accept_admin()`, `DataKey::Admin` is set and
/// takes precedence over `Config.admin`.
fn get_active_admin(env: &Env) -> Result<Address, SettleError> {
    if let Some(admin) = env
        .storage()
        .instance()
        .get::<DataKey, Address>(&DataKey::Admin)
    {
        return Ok(admin);
    }
    let config = load_config(env)?;
    Ok(config.admin)
}

#[contractimpl]
impl MaturitySettlement {
    /// Initialize the per-offer settlement contract. Called once after deployment.
    ///
    /// `admin`: issuer public key (SAC admin for clawback authority).
    /// `usdc_sac`: USDC SAC contract address.
    /// `token_sac`: security token SAC contract address.
    /// `treasury`: platform treasury for fee routing.
    /// `max_fee_bps`: maximum platform fee in basis points (200 = 2%).
    ///                Immutable after init. Enforced in settle_batch().
    pub fn initialize(
        env: Env,
        admin: Address,
        usdc_sac: Address,
        token_sac: Address,
        treasury: Address,
        max_fee_bps: u32,
    ) -> Result<(), SettleError> {
        // Prevent double init
        if env.storage().instance().has(&DataKey::Config) {
            return Err(SettleError::AlreadyInitialized);
        }

        // v3 (F-006): enforce canonical USDC SAC at initialize() — prevents
        // address-poisoning by a malicious or compromised admin.
        validate_canonical_usdc(&env, &usdc_sac)?;

        admin.require_auth();

        let config = Config {
            admin,
            usdc_sac,
            token_sac,
            treasury,
            max_fee_bps,
        };
        env.storage().instance().set(&DataKey::Config, &config);

        // Extend TTL on init
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);

        emit(&env, symbol_short!("init"), CONTRACT_VERSION);
        Ok(())
    }

    /// Company deposits USDC into this settlement contract.
    /// Depositor signs via passkey (C... smart wallet).
    /// Multiple deposits from the same depositor accumulate (checked_add).
    pub fn deposit(
        env: Env,
        depositor: Address,
        amount: i128,
    ) -> Result<(), SettleError> {
        let config = load_config(&env)?;

        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }

        if amount <= 0 {
            return Err(SettleError::InvalidAmount);
        }

        if round_closed(&env) {
            return Err(SettleError::RoundClosed);
        }
        depositor.require_auth();
        set_deposit_total(&env, deposit_total(&env).checked_add(amount).ok_or(SettleError::Overflow)?);
        // Transfer USDC from depositor to contract
        let usdc = token::Client::new(&env, &config.usdc_sac);
        usdc.transfer(&depositor, &env.current_contract_address(), &amount);

        // Accumulate deposit tracking (checked_add for overflow safety)
        let key = DataKey::Deposit(depositor.clone());
        let existing: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        let new_total = existing.checked_add(amount).ok_or(SettleError::Overflow)?;
        env.storage().persistent().set(&key, &new_total);

        // Extend deposit TTL
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);

        emit(&env, symbol_short!("deposit"), amount);
        Ok(())
    }

    /// Atomically settle a batch of investors:
    ///   1. Validate: no duplicates, no phantom investors
    ///   2. USDC → each investor (payout)
    ///   3. Read on-chain token balance → clawback ALL (burn)
    ///   4. USDC → treasury (aggregated fee)
    ///
    /// TRUSTLESS: Clawback is automatic. The contract reads each investor's
    /// token balance from the chain and burns ALL of it. No backend input
    /// needed for burns — the public ledger is the source of truth.
    ///
    /// MULTI-BATCH: Can be called multiple times with different investor sets.
    /// Per-investor idempotency prevents double-payouts across batches.
    /// REENTRANCY SAFETY: Soroban's host is single-threaded and cross-contract
    /// calls cannot re-enter the caller. That — and ONLY that — is what makes
    /// this function safe today.
    ///
    /// §38.3 (09/09/2026): this comment used to claim the function "ALSO follows
    /// checks-effects-interactions — all state writes land before the external
    /// token transfers". It does NOT. The execution loop transfers the payout,
    /// reads the balance and claws back, and only THEN writes the `Settled`
    /// marker — the very "transfer, then mark" shape the old text forbade. The
    /// marker is what prevents paying the same investor twice.
    ///
    /// The text was corrected instead of the code because reordering changes the
    /// WASM, and this contract is DEPLOYED (v4, testnet + mainnet) — a new binary
    /// means a Ledger ceremony, which is Pedro's call, not a side effect of a
    /// comment fix. A comment that declares a defense the code does not implement
    /// is worse than no comment: the next reader "keeps the pattern" while
    /// preserving its absence.
    ///
    /// TODO (next ceremony): move the `Settled` write above the transfers, so the
    /// safety stops depending on a host property that may change.
    pub fn settle_batch(
        env: Env,
        items: Vec<SettleItem>,
        total_fee: i128,
    ) -> Result<(), SettleError> {
        let config = load_config(&env)?;

        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }

        let admin = get_active_admin(&env)?;
        admin.require_auth();

        if items.is_empty() {
            return Err(SettleError::EmptyBatch);
        }

        if items.len() > MAX_BATCH_SIZE {
            return Err(SettleError::BatchTooLarge);
        }

        if total_fee < 0 {
            return Err(SettleError::InvalidAmount);
        }
        if round_closed(&env) {
            return Err(SettleError::RoundClosed);
        }
        let round = current_round(&env);

        // ── Validation loop ──────────────────────────────────────────
        let mut seen: Map<Address, bool> = Map::new(&env);
        let sec_token = token::Client::new(&env, &config.token_sac);

        for i in 0..items.len() {
            let item = items.get(i).unwrap();

            if item.payout < 0 || item.tokens <= 0 {
                return Err(SettleError::InvalidAmount);
            }

            // Within-batch dedup
            if seen.contains_key(item.investor.clone()) {
                return Err(SettleError::DuplicateInvestor);
            }
            seen.set(item.investor.clone(), true);

            // Cross-batch dedup — v4: once per ROUND (persistent storage)
            let settled_key = DataKey::Settled(round, item.investor.clone());
            if env.storage().persistent().has(&settled_key) {
                return Err(SettleError::AlreadySettled);
            }

            // Phantom investor: can't pay someone with 0 tokens
            let balance = sec_token.balance(&item.investor);
            if balance == 0 && item.payout > 0 {
                return Err(SettleError::PhantomInvestor);
            }
            // v4: partial burn must be backed by the balance
            if item.tokens != i128::MAX && item.tokens > balance {
                return Err(SettleError::TokensExceedBalance);
            }
        }
        // ── v4 expected balance: every payout + fee must be covered BEFORE anything moves ──
        let mut sum_payouts: i128 = 0;
        for i in 0..items.len() {
            let item = items.get(i).unwrap();
            sum_payouts = sum_payouts.checked_add(item.payout).ok_or(SettleError::Overflow)?;
        }
        let needed = sum_payouts.checked_add(total_fee).ok_or(SettleError::Overflow)?;
        let usdc_probe = token::Client::new(&env, &config.usdc_sac);
        if usdc_probe.balance(&env.current_contract_address()) < needed {
            return Err(SettleError::InsufficientDeposit);
        }

        // ── Fee cap (CVM transparency) ───────────────────────────────
        // B1 (red-team 24/08): o teto vale SEMPRE que ha fee. As duas guardas
        // antigas (`max_fee_bps > 0` e `sum_payouts > 0`) abriam o mesmo buraco:
        // um lote de payout ZERO (maturidade em default gera isso legitimamente)
        // pulava a checagem e deixava total_fee = saldo inteiro -> admin drenava
        // o pool para a treasury. E max_fee_bps==0 significa "fee TEM que ser 0",
        // nunca "sem teto". payouts==0 -> max_allowed==0 -> qualquer fee reverte.
        if total_fee > 0 {
            let max_allowed = sum_payouts
                .checked_mul(config.max_fee_bps as i128)
                .ok_or(SettleError::Overflow)?
                / 10_000;
            if total_fee > max_allowed {
                return Err(SettleError::FeeTooHigh);
            }
        }

        // ── Execute atomically ────────────────────────────────────────
        let usdc = token::Client::new(&env, &config.usdc_sac);
        let sac = token::StellarAssetClient::new(&env, &config.token_sac);
        let contract = env.current_contract_address();

        for i in 0..items.len() {
            let item = items.get(i).unwrap();

            // Pay investor
            if item.payout > 0 {
                usdc.transfer(&contract, &item.investor, &item.payout);
            }

            // v4: burn `tokens` (MAX = the whole balance, the v3 bullet semantics)
            let balance = sec_token.balance(&item.investor);
            let burn = if item.tokens == i128::MAX { balance } else { item.tokens };
            if burn > 0 {
                sac.clawback(&item.investor, &burn);
            }
            emit(&env, symbol_short!("settle"), (round, item.investor.clone(), item.payout, burn));
            // Mark settled for THIS round (persistent — survives across batches)
            let settled_key = DataKey::Settled(round, item.investor.clone());
            env.storage().persistent().set(&settled_key, &true);
            env.storage()
                .persistent()
                .extend_ttl(&settled_key, TTL_THRESHOLD, TTL_EXTEND);
        }

        // Fee to treasury
        if total_fee > 0 {
            usdc.transfer(&contract, &config.treasury, &total_fee);
        }

        // Increment settled count (instance storage, fast read for refund guard)
        let prev_count: u32 = env.storage().instance().get(&DataKey::SettledCount).unwrap_or(0);
        env.storage().instance().set(&DataKey::SettledCount, &(prev_count + items.len()));

        // Extend TTL after state change
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);

        emit(&env, symbol_short!("settled"), items.len());
        Ok(())
    }

    /// Admin withdraws any token from the contract (emergency recovery).
    /// v4 (PEND 162): rescues a token sent here by mistake — never the USDC, and
    /// only to the admin. USDC leaves through settle_batch / refund / claim_residual.
    pub fn withdraw(
        env: Env,
        token: Address,
        amount: i128,
    ) -> Result<(), SettleError> {
        let config = load_config(&env)?;
        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        if amount <= 0 {
            return Err(SettleError::InvalidAmount);
        }
        if token == config.usdc_sac {
            return Err(SettleError::UnauthorizedToken);
        }
        let client = token::Client::new(&env, &token);
        client.transfer(&env.current_contract_address(), &admin, &amount);
        emit(&env, symbol_short!("withdraw"), amount);
        Ok(())
    }

    /// v4: closes the current round. What was not paid to investors becomes the
    /// RESIDUAL, owed back to the depositors pro-rata (see claim_residual). Idempotent.
    pub fn finalize_round(env: Env) -> Result<(), SettleError> {
        let config = load_config(&env)?;
        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        if round_closed(&env) {
            return Ok(());
        }
        let usdc = token::Client::new(&env, &config.usdc_sac);
        let residual = usdc.balance(&env.current_contract_address());
        env.storage().instance().set(&DataKey::RoundClosed, &true);
        env.storage().instance().set(&DataKey::ResidualSnapshot, &residual);
        env.storage().instance().set(&DataKey::DepositTotalSnapshot, &deposit_total(&env));
        emit(&env, symbol_short!("final"), (current_round(&env), residual));
        Ok(())
    }

    /// v4: a depositor takes back its share of the residual of a finalized round
    /// (deposit / deposit_total × residual). Once per depositor per round.
    pub fn claim_residual(env: Env, depositor: Address) -> Result<(), SettleError> {
        let config = load_config(&env)?;
        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }
        depositor.require_auth();
        if !round_closed(&env) {
            return Err(SettleError::RoundOpen);
        }
        let key = DataKey::Deposit(depositor.clone());
        let deposit: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if deposit <= 0 {
            return Err(SettleError::NoDeposit);
        }
        let residual: i128 = env.storage().instance().get(&DataKey::ResidualSnapshot).unwrap_or(0);
        let base: i128 = env.storage().instance().get(&DataKey::DepositTotalSnapshot).unwrap_or(0);
        let share = if base > 0 {
            deposit.checked_mul(residual).ok_or(SettleError::Overflow)? / base
        } else {
            0
        };
        // Effects before interaction.
        env.storage().persistent().set(&key, &0i128);
        set_deposit_total(&env, deposit_total(&env) - deposit);
        if share > 0 {
            token::Client::new(&env, &config.usdc_sac).transfer(&env.current_contract_address(), &depositor, &share);
        }
        emit(&env, symbol_short!("claim"), (depositor, share));
        Ok(())
    }

    /// v4: opens the next round once the current one is finalized and every
    /// depositor has claimed its residual.
    pub fn next_round(env: Env) -> Result<(), SettleError> {
        let _config = load_config(&env)?;
        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        if !round_closed(&env) {
            return Err(SettleError::RoundOpen);
        }
        if deposit_total(&env) > 0 {
            return Err(SettleError::ResidualPending);
        }
        let next = current_round(&env) + 1;
        env.storage().instance().set(&DataKey::Round, &next);
        env.storage().instance().set(&DataKey::RoundClosed, &false);
        env.storage().instance().set(&DataKey::SettledCount, &0u32);
        env.storage().instance().set(&DataKey::ResidualSnapshot, &0i128);
        env.storage().instance().set(&DataKey::DepositTotalSnapshot, &0i128);
        emit(&env, symbol_short!("round"), next);
        Ok(())
    }

    /// v4: (round, closed, deposit_total still owed to depositors, items settled this round)
    pub fn get_round(env: Env) -> (u32, bool, i128, u32) {
        let settled: u32 = env.storage().instance().get(&DataKey::SettledCount).unwrap_or(0);
        (current_round(&env), round_closed(&env), deposit_total(&env), settled)
    }

    /// Admin refunds a depositor's USDC. Blocked after settlement (V-1).
    /// Only callable by admin (not depositor) to prevent company self-refund attack.
    pub fn refund(
        env: Env,
        depositor: Address,
    ) -> Result<(), SettleError> {
        let config = load_config(&env)?;

        if is_paused(&env) {
            return Err(SettleError::ContractPaused);
        }

        let admin = get_active_admin(&env)?;
        admin.require_auth();

        if round_closed(&env) {
            return Err(SettleError::RoundClosed); // after finalize the way back is claim_residual()
        }
        // Block refunds after any settlement has started (in this round)
        let settled_count: u32 = env.storage().instance().get(&DataKey::SettledCount).unwrap_or(0);
        if settled_count > 0 {
            return Err(SettleError::AlreadySettled);
        }

        // Load deposit amount
        let key = DataKey::Deposit(depositor.clone());
        let amount: i128 = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(SettleError::NoDeposit)?;

        if amount == 0 {
            return Err(SettleError::NoDeposit);
        }

        // Transfer USDC back to depositor
        let usdc = token::Client::new(&env, &config.usdc_sac);
        usdc.transfer(&env.current_contract_address(), &depositor, &amount);

        // Clear deposit tracking
        env.storage().persistent().set(&key, &0i128);
        set_deposit_total(&env, deposit_total(&env) - amount);
        emit(&env, symbol_short!("refund"), amount);
        Ok(())
    }

    /// Upgrade contract WASM. Admin only (high-privilege).
    /// Intentionally NOT pause-gated — upgrade is the recovery path when a
    /// pause has been triggered to contain a bug.
    /// Upgrade do WASM — VALVULA DE RECUPERACAO (decisao do Pedro 24/08, pos red-team).
    ///
    /// NOTA (09/09): as duas linhas que estavam AQUI diziam o oposto do paragrafo
    /// abaixo — "the code does not change while an open round holds deposits or has
    /// settled items (same rule as token_sale v9.1). Finalize + claims first." Era a
    /// regra do v9.1, REVOGADA pelo Pedro no mesmo 24/08 depois do red-team achar o
    /// deadlock "fechavel mas infechavel". A correcao foi anexada e a frase original
    /// nunca apagada, entao o mesmo comentario afirmava e negava o lock em paragrafos
    /// consecutivos. O codigo (linhas abaixo) nao tem lock nenhum: require_auth do
    /// admin e update_current_contract_wasm, mais nada.
    /// So o admin, que na mainnet e a LEDGER (cold/2-de-3) — NUNCA uma hot wallet.
    /// Deliberadamente SEM lock de "dinheiro dentro": e exatamente quando ha deposito
    /// numa rodada aberta que um bug prenderia o USDC da empresa sem saida, e a Ledger
    /// tem que poder trocar o codigo para destravar. O risco (Ledger comprometida) e
    /// menor que o risco (dinheiro preso e intocavel) enquanto o admin e uma unica
    /// Ledger; multisig de verdade e o proximo passo. Ver memoria recovery-valve-ledger-only.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), SettleError> {
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash.clone());
        emit(&env, symbol_short!("upgrade"), new_wasm_hash);
        Ok(())
    }

    // ─── v2 additions: pause + 2-step admin rotation ─────────────

    /// Pause the contract. Admin-only. Blocks deposit/settle/withdraw/refund.
    /// Upgrade and accept_admin remain accessible (recovery paths).
    pub fn pause(env: Env) -> Result<(), SettleError> {
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("paused"), true);
        Ok(())
    }

    /// Resume the contract. Admin-only.
    pub fn resume(env: Env) -> Result<(), SettleError> {
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
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
    ) -> Result<(), SettleError> {
        let admin = get_active_admin(&env)?;
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
    pub fn renounce_emergency_pauser(env: Env) -> Result<(), SettleError> {
        let pauser: Address = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyPauser)
            .ok_or(SettleError::NoEmergencyPauser)?;
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
    /// `pause()` (mesmo efeito: ver o cabecalho v5). NAO despausa (`resume` e so do admin),
    /// NAO move dinheiro, NAO troca admin, NAO faz upgrade — esta funcao nao tem outro efeito.
    /// Idempotente: pausar contrato ja pausado nao falha.
    ///
    /// Custo assumido da pausa (inalterado desde v2): enquanto pausado, `refund` e
    /// `claim_residual` tambem ficam bloqueados — o dinheiro do depositante so sai depois do
    /// `resume` do admin. Um pauser hostil congela, nao subtrai.
    pub fn emergency_pause(env: Env) -> Result<(), SettleError> {
        let pauser: Address = env
            .storage()
            .instance()
            .get(&DataKey::EmergencyPauser)
            .ok_or(SettleError::NoEmergencyPauser)?;
        pauser.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("paused"), true);
        emit(&env, symbol_short!("emergpaus"), pauser);
        Ok(())
    }

    /// Step 1 of admin rotation: current admin proposes a new admin.
    /// The new admin must then call accept_admin() to take ownership.
    /// Overwrites any prior pending proposal.
    pub fn propose_admin(env: Env, new_admin: Address) -> Result<(), SettleError> {
        let admin = get_active_admin(&env)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::PendingAdmin, &new_admin);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("propadm"), new_admin);
        Ok(())
    }

    /// Step 2 of admin rotation: pending admin accepts ownership.
    /// The pending admin must sign — proves they hold the keypair (prevents
    /// transfer-to-typo'd-address footgun).
    pub fn accept_admin(env: Env) -> Result<(), SettleError> {
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(SettleError::NoPendingAdmin)?;
        pending.require_auth();
        env.storage().instance().set(&DataKey::Admin, &pending);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        emit(&env, symbol_short!("admchg"), pending);
        Ok(())
    }

    // ─── v2 readers ──────────────────────────────────────────────

    /// Returns whether the contract is paused.
    pub fn get_paused(env: Env) -> bool {
        is_paused(&env)
    }

    /// Returns the currently active admin.
    pub fn get_admin(env: Env) -> Result<Address, SettleError> {
        get_active_admin(&env)
    }

    /// v5 (F6) — O `emergency_pauser` configurado, ou None.
    pub fn emergency_pauser(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::EmergencyPauser)
    }

    /// Returns the pending admin if one has been proposed.
    pub fn get_pending_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::PendingAdmin)
    }

    /// Extend contract instance TTL. Anyone can call (allows cron jobs).
    pub fn extend_ttl(env: Env) {
        env.storage()
            .instance()
            .extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    }

    /// Read: contract's USDC balance.
    pub fn get_balance(env: Env) -> Result<i128, SettleError> {
        let config = load_config(&env)?;
        let usdc = token::Client::new(&env, &config.usdc_sac);
        Ok(usdc.balance(&env.current_contract_address()))
    }

    /// Read: deposit amount for a specific depositor.
    pub fn get_deposit(env: Env, depositor: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Deposit(depositor))
            .unwrap_or(0)
    }

    /// Returns the contract version.
    pub fn version(_env: Env) -> u32 {
        CONTRACT_VERSION
    }
}

fn load_config(env: &Env) -> Result<Config, SettleError> {
    env.storage()
        .instance()
        .get(&DataKey::Config)
        .ok_or(SettleError::NotInitialized)
}

mod test;
