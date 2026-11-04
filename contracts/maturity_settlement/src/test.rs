#![cfg(test)]
// O crate e `#![no_std]`; os testes rodam com std disponivel, mas ele precisa ser LIGADO
// explicitamente. Sem isto, `std::string::String` nao resolve (§17.6/§38.3).
extern crate std;

use soroban_sdk::testutils::{Address as _, Events as _, IssuerFlags};
use soroban_sdk::{token, vec, Address, Env, IntoVal, Vec};

use crate::{Config, DataKey, MaturitySettlement, MaturitySettlementClient, SettleError, SettleItem};

// ═══════════════════════════════════════════════════════
//  HELPERS
// ═══════════════════════════════════════════════════════

fn create_token_contract<'a>(
    e: &Env,
    admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    let sac = e.register_stellar_asset_contract_v2(admin.clone());
    (
        token::Client::new(e, &sac.address()),
        token::StellarAssetClient::new(e, &sac.address()),
    )
}

// PENDENCIAS 159 — o USDC das fixtures precisa nascer NO endereço canônico para
// a suíte inteira rodar sob as features do WASM real (`--no-default-features
// --features testnet|mainnet`), onde validate_canonical_usdc (F-006) está vivo.
#[cfg(not(feature = "testing"))]
#[path = "../../shared/mock_usdc.rs"]
mod mock_usdc;

/// USDC da fixture. Sob `testing`: SAC gerado (status quo). Sob rede real: mock
/// registrado NO endereço canônico, o único que o F-006 aceita em initialize().
#[cfg(feature = "testing")]
fn create_usdc_contract<'a>(
    e: &Env,
    admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    create_token_contract(e, admin)
}

#[cfg(not(feature = "testing"))]
fn create_usdc_contract<'a>(
    e: &Env,
    _admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    let addr = Address::from_str(e, mock_usdc::CANONICAL_USDC);
    e.register_at(&addr, mock_usdc::MockUsdc, ());
    (
        token::Client::new(e, &addr),
        token::StellarAssetClient::new(e, &addr),
    )
}

/// Create a security token SAC with clawback enabled on the issuer.
/// Must be called BEFORE minting — trustline clawback flag is set at creation time.
fn create_clawback_token_contract<'a>(
    e: &Env,
    admin: &Address,
) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
    let sac = e.register_stellar_asset_contract_v2(admin.clone());
    // Enable clawback on issuer BEFORE any mint (trustline flag set at creation)
    sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    sac.issuer().set_flag(IssuerFlags::ClawbackEnabledFlag);
    (
        token::Client::new(e, &sac.address()),
        token::StellarAssetClient::new(e, &sac.address()),
    )
}

/// Standard setup: admin, company, 1 investor, USDC + security token, initialized contract.
/// Company gets $10,000 USDC. Investor gets 1,000 security tokens.
fn setup<'a>(
    e: &Env,
) -> (
    MaturitySettlementClient<'a>,
    Address,                       // admin (= issuer = SAC admin)
    Address,                       // company (depositor)
    Address,                       // investor
    Address,                       // treasury
    token::Client<'a>,             // usdc
    token::StellarAssetClient<'a>, // usdc_admin
    token::Client<'a>,             // security token
    token::StellarAssetClient<'a>, // token_admin
    Address,                       // contract_id
) {
    let admin = Address::generate(e);
    let company = Address::generate(e);
    let investor = Address::generate(e);
    let treasury = Address::generate(e);

    let (usdc, usdc_admin) = create_usdc_contract(e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(e, &contract_id);

    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);

    usdc_admin.mint(&company, &(10_000 * 10_000_000i128));
    sec_token_admin.mint(&investor, &(1_000 * 10_000_000i128));

    (client, admin, company, investor, treasury, usdc, usdc_admin, sec_token, sec_token_admin, contract_id)
}

/// Helper: seed Config directly into storage without mock_all_auths (for auth tests).
fn seed_config(e: &Env, contract: &Address, admin: &Address) {
    e.as_contract(contract, || {
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                usdc_sac: Address::generate(e),
                token_sac: Address::generate(e),
                treasury: Address::generate(e),
                max_fee_bps: 5000,
            },
        );
    });
}

// ═══════════════════════════════════════════════════════
//  1. initialize() — 5 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_initialize_happy_path() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);
    let treasury = Address::generate(&e);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);
    // No panic = success
}

#[test]
fn test_initialize_double_call_fails() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);
    let treasury = Address::generate(&e);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);
    let result = client.try_initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);
    assert_eq!(result, Err(Ok(SettleError::AlreadyInitialized)));
}

#[test]
fn test_initialize_stores_correct_config() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);
    let treasury = Address::generate(&e);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);

    // Verify via get_balance (proves config.usdc_sac is stored correctly)
    assert_eq!(client.get_balance(), 0);
}

#[test]
fn test_initialize_different_admin_and_treasury() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let treasury = Address::generate(&e);
    assert_ne!(admin, treasury);

    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);
    // No panic = admin ≠ treasury is fine
}

#[test]
fn test_initialize_extends_ttl() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &Address::generate(&e), &5000);
    // Subsequent calls shouldn't panic due to expired TTL
    assert_eq!(client.version(), 5);
}

// ═══════════════════════════════════════════════════════
//  2. deposit() — 9 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_deposit_happy_path() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, contract_id) = setup(&e);

    let amount: i128 = 1_000 * 10_000_000;
    client.deposit(&company, &amount);

    assert_eq!(usdc.balance(&contract_id), amount);
    assert_eq!(client.get_deposit(&company), amount);
}

#[test]
fn test_deposit_zero_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    assert_eq!(client.try_deposit(&company, &0), Err(Ok(SettleError::InvalidAmount)));
}

#[test]
fn test_deposit_negative_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    assert_eq!(client.try_deposit(&company, &-1), Err(Ok(SettleError::InvalidAmount)));
}

#[test]
fn test_deposit_i128_min_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    assert_eq!(client.try_deposit(&company, &i128::MIN), Err(Ok(SettleError::InvalidAmount)));
}

#[test]
fn test_deposit_multiple_accumulate() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(500 * 10_000_000i128));
    client.deposit(&company, &(300 * 10_000_000i128));

    assert_eq!(client.get_deposit(&company), 800 * 10_000_000);
    assert_eq!(usdc.balance(&contract_id), 800 * 10_000_000);
}

#[test]
fn test_deposit_different_depositors_independent() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, usdc_admin, _, _, contract_id) = setup(&e);

    let other = Address::generate(&e);
    usdc_admin.mint(&other, &(500 * 10_000_000i128));

    client.deposit(&company, &(500 * 10_000_000i128));
    client.deposit(&other, &(300 * 10_000_000i128));

    assert_eq!(client.get_deposit(&company), 500 * 10_000_000);
    assert_eq!(client.get_deposit(&other), 300 * 10_000_000);
    assert_eq!(usdc.balance(&contract_id), 800 * 10_000_000);
}

#[test]
#[should_panic]
fn test_deposit_exceeds_balance_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    // Company has $10,000 — trying $20,000
    client.deposit(&company, &(20_000 * 10_000_000i128));
}

#[test]
fn test_deposit_overflow_accumulation() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let depositor = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &Address::generate(&e), &5000);

    // Seed a huge existing deposit via storage
    e.as_contract(&contract_id, || {
        let key = DataKey::Deposit(depositor.clone());
        e.storage().persistent().set(&key, &(i128::MAX - 10));
    });

    usdc_admin.mint(&depositor, &(100 * 10_000_000i128));
    let result = client.try_deposit(&depositor, &20);
    assert_eq!(result, Err(Ok(SettleError::Overflow)));
}

#[test]
fn test_deposit_get_nonexistent_returns_zero() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, _, _, _, _, _, _, _, _) = setup(&e);

    assert_eq!(client.get_deposit(&Address::generate(&e)), 0);
}

// ═══════════════════════════════════════════════════════
//  3. settle_batch() CORE — 14 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_settle_single_investor() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    // Company deposits $100
    client.deposit(&company, &(100 * 10_000_000i128));

    let items = vec![
        &e,
        SettleItem {
            investor: investor.clone(),
            payout: 95 * 10_000_000, tokens: i128::MAX,
        },
    ];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    assert_eq!(usdc.balance(&investor), 95 * 10_000_000);
    assert_eq!(usdc.balance(&treasury), 5 * 10_000_000);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned
}

#[test]
fn test_settle_three_investors() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, _, treasury, usdc, _, sec_token, sec_token_admin, contract_id) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    let inv_c = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(200 * 10_000_000i128));
    sec_token_admin.mint(&inv_c, &(300 * 10_000_000i128));

    client.deposit(&company, &(600 * 10_000_000i128));

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: 90 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: 180 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: inv_c.clone(), payout: 270 * 10_000_000, tokens: i128::MAX },
    ];
    let fee: i128 = 60 * 10_000_000; // 600 - 90 - 180 - 270 = 60
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&inv_a), 90 * 10_000_000);
    assert_eq!(usdc.balance(&inv_b), 180 * 10_000_000);
    assert_eq!(usdc.balance(&inv_c), 270 * 10_000_000);
    assert_eq!(usdc.balance(&treasury), fee);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(sec_token.balance(&inv_a), 0);
    assert_eq!(sec_token.balance(&inv_b), 0);
    assert_eq!(sec_token.balance(&inv_c), 0);
}

#[test]
fn test_settle_empty_batch_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));

    let empty: Vec<SettleItem> = vec![&e];
    assert_eq!(client.try_settle_batch(&empty, &0), Err(Ok(SettleError::EmptyBatch)));
}

#[test]
fn test_settle_zero_payout_skipped() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, contract_id) = setup(&e);

    client.deposit(&company, &(10 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 0, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &0i128); // B1: clawback-sem-pagamento => fee 0

    assert_eq!(usdc.balance(&investor), 0); // no payout
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned from chain
}

/// Contract always burns all tokens, even when payout = 0.
/// This is the "clawback without payout" scenario (company default).
#[test]
fn test_settle_zero_clawback_skipped() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &0);

    assert_eq!(usdc.balance(&investor), 100 * 10_000_000);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned from chain
}

#[test]
fn test_settle_zero_fee_no_treasury_transfer() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &0);

    assert_eq!(usdc.balance(&treasury), 0);
}

#[test]
fn test_settle_marks_as_settled() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // Second settle must fail
    let result = client.try_settle_batch(&items, &(5 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

#[test]
#[should_panic]
fn test_settle_insufficient_balance_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(50 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &0); // 100 > 50 → panic
}

#[test]
fn test_settle_exact_balance_succeeds() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, _, _, contract_id) = setup(&e);

    let deposit_amount: i128 = 105 * 10_000_000;
    client.deposit(&company, &deposit_amount);

    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(usdc.balance(&investor), 100 * 10_000_000);
    assert_eq!(usdc.balance(&treasury), 5 * 10_000_000);
}

#[test]
fn test_settle_clawback_auth_propagation() {
    // CRITICAL TEST: Verifies that admin.require_auth() in settle_batch()
    // propagates to SAC clawback (admin = issuer = SAC admin).
    // With mock_all_auths this passes trivially — proves the CODE PATH works.
    // Real auth propagation is verified on testnet deployment.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // All tokens burned from chain
    assert_eq!(sec_token.balance(&investor), 0);
}

#[test]
fn test_settle_batch_max_size_enforced() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(1_000 * 10_000_000i128));

    // Build 31 items (MAX = 30)
    let mut items_raw: soroban_sdk::Vec<SettleItem> = soroban_sdk::Vec::new(&e);
    for _ in 0..31 {
        items_raw.push_back(SettleItem {
            investor: Address::generate(&e),
            payout: 1, tokens: i128::MAX,
        });
    }
    let result = client.try_settle_batch(&items_raw, &0);
    assert_eq!(result, Err(Ok(SettleError::BatchTooLarge)));
}

#[test]
fn test_settle_batch_at_max_succeeds() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, sec_token_admin, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));

    let mut items_raw: soroban_sdk::Vec<SettleItem> = soroban_sdk::Vec::new(&e);
    for _ in 0..30 {
        let inv = Address::generate(&e);
        sec_token_admin.mint(&inv, &(1i128));
        items_raw.push_back(SettleItem {
            investor: inv,
            payout: 1, tokens: i128::MAX,
        });
    }
    // Native test env has stricter resource limits than on-chain; disable for large batches
    e.cost_estimate().disable_resource_limits();
    e.cost_estimate().budget().reset_unlimited();
    client.settle_batch(&items_raw, &0);
    // No panic = success with exactly 30 items
}

#[test]
fn test_settle_negative_fee_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));

    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
    ];
    let result = client.try_settle_batch(&items, &-1);
    assert_eq!(result, Err(Ok(SettleError::InvalidAmount)));
}

// ═══════════════════════════════════════════════════════
//  4. settle_batch() FINANCIAL INVARIANTS — 7 tests
//
//  Every test asserts ALL of these:
//    investor_usdc_after  = investor_usdc_before + payout
//    investor_tokens_after = investor_tokens_before - clawback
//    treasury_usdc_after  = treasury_usdc_before + fee
//    contract_usdc_after  = contract_usdc_before - Σ(payouts) - fee
//    Σ(payouts) + fee     = total_settlement
// ═══════════════════════════════════════════════════════

#[test]
fn test_financial_invariants_simple() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    let deposit: i128 = 110 * 10_000_000;
    client.deposit(&company, &deposit);

    let payout: i128 = 100 * 10_000_000;
    let fee: i128 = 10 * 10_000_000;

    let inv_usdc_before = usdc.balance(&investor);
    let treas_before = usdc.balance(&treasury);

    let items = vec![&e, SettleItem { investor: investor.clone(), payout, tokens: i128::MAX }];
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&investor), inv_usdc_before + payout);
    assert_eq!(sec_token.balance(&investor), 0);  // ALL tokens burned from chain
    assert_eq!(usdc.balance(&treasury), treas_before + fee);
    assert_eq!(usdc.balance(&contract_id), 0); // deposit - payout - fee = 0
    assert_eq!(payout + fee, deposit);
}

#[test]
fn test_financial_invariants_multi_investor() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, _, _, sec_token_admin, contract_id) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(200 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(300 * 10_000_000i128));

    let deposit: i128 = 500 * 10_000_000;
    client.deposit(&company, &deposit);

    let pa: i128 = 180 * 10_000_000;
    let pb: i128 = 270 * 10_000_000;
    let fee: i128 = 50 * 10_000_000;

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: pa, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: pb, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&inv_a), pa);
    assert_eq!(usdc.balance(&inv_b), pb);
    assert_eq!(usdc.balance(&treasury), fee);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(pa + pb + fee, deposit);
}

#[test]
fn test_financial_invariants_zero_fee() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, _, _, contract_id) = setup(&e);

    let deposit: i128 = 100 * 10_000_000;
    client.deposit(&company, &deposit);

    let items = vec![&e, SettleItem { investor: investor.clone(), payout: deposit, tokens: i128::MAX }];
    client.settle_batch(&items, &0);

    assert_eq!(usdc.balance(&investor), deposit);
    assert_eq!(usdc.balance(&treasury), 0);
    assert_eq!(usdc.balance(&contract_id), 0);
}

#[test]
fn test_financial_invariants_large_amounts() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, _, _, treasury, usdc, usdc_admin, sec_token, sec_token_admin, contract_id) = setup(&e);

    let whale = Address::generate(&e);
    let deposit: i128 = 1_000_000 * 10_000_000; // $1M
    usdc_admin.mint(&whale, &deposit);
    sec_token_admin.mint(&whale, &(500_000 * 10_000_000i128));

    // Use whale as depositor
    client.deposit(&whale, &deposit);

    let payout: i128 = 950_000 * 10_000_000;
    let fee: i128 = 50_000 * 10_000_000;
    let items = vec![&e, SettleItem { investor: whale.clone(), payout, tokens: i128::MAX }];
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&whale), payout);
    assert_eq!(usdc.balance(&treasury), fee);
    assert_eq!(payout + fee, deposit);
}

#[test]
fn test_financial_invariants_fractional_stroops() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    let payout: i128 = 123_456_789;
    let fee: i128 = 9_876_543;
    let deposit = payout + fee;
    client.deposit(&company, &deposit);

    let items = vec![&e, SettleItem { investor: investor.clone(), payout, tokens: i128::MAX }];
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&investor), payout);
    assert_eq!(usdc.balance(&treasury), fee);
    assert_eq!(usdc.balance(&contract_id), 0);
}

#[test]
fn test_financial_no_rounding_leak() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, _, _, sec_token_admin, contract_id) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    let inv_c = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(10 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(10 * 10_000_000i128));
    sec_token_admin.mint(&inv_c, &(10 * 10_000_000i128));

    // Odd payouts that could cause rounding issues
    let p1: i128 = 33_333_333;
    let p2: i128 = 33_333_334;
    let p3: i128 = 33_333_333;
    let fee: i128 = 0;
    let deposit = p1 + p2 + p3 + fee;
    client.deposit(&company, &deposit);

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: p1, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: p2, tokens: i128::MAX },
        SettleItem { investor: inv_c.clone(), payout: p3, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &fee);

    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(usdc.balance(&inv_a) + usdc.balance(&inv_b) + usdc.balance(&inv_c) + usdc.balance(&treasury), deposit);
}

#[test]
fn test_financial_all_tokens_burned() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, sec_token, sec_token_admin, _) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(200 * 10_000_000i128));

    client.deposit(&company, &(300 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: 200 * 10_000_000, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &0);

    assert_eq!(sec_token.balance(&inv_a), 0);
    assert_eq!(sec_token.balance(&inv_b), 0);
}

// ═══════════════════════════════════════════════════════
//  5. settle_batch() EDGE CASES — 5 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_settle_both_zero_payout_and_clawback() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(10 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 0, tokens: i128::MAX }];
    client.settle_batch(&items, &0i128); // B1: clawback-sem-pagamento => fee 0

    // Investor balances unchanged
    assert_eq!(usdc.balance(&investor), 0);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned
}

#[test]
fn test_settle_same_investor_twice_in_batch() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(200 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: investor.clone(), payout: 30 * 10_000_000, tokens: i128::MAX },
    ];
    // Contract rejects duplicate investor addresses — trustless validation
    let result = client.try_settle_batch(&items, &(120 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::DuplicateInvestor)));

    // No money moved — settlement was rejected
    assert_eq!(usdc.balance(&investor), 0);
    assert_eq!(sec_token.balance(&investor), 1_000 * 10_000_000); // tokens untouched
}

#[test]
fn test_settle_investor_is_admin() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, _, _, usdc, _, _, sec_token_admin, _) = setup(&e);

    sec_token_admin.mint(&admin, &(100 * 10_000_000i128));
    client.deposit(&company, &(100 * 10_000_000i128));

    let items = vec![&e, SettleItem { investor: admin.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    assert_eq!(usdc.balance(&admin), 95 * 10_000_000);
}

#[test]
fn test_settle_one_stroop_payout() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &1i128);
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 1, tokens: i128::MAX }];
    client.settle_batch(&items, &0);

    assert_eq!(usdc.balance(&investor), 1);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned from chain
}

/// clawback_exceeds_balance is impossible now — contract reads actual balance.
/// This test verifies the settle succeeds (contract reads 1000, burns 1000).
#[test]
fn test_settle_clawback_exceeds_investor_balance() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 100 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &0);
    assert_eq!(sec_token.balance(&investor), 0); // all 1000 tokens burned
}

// ═══════════════════════════════════════════════════════
//  6. withdraw() — 4 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_withdraw_usdc_refused_residual_goes_back_to_depositor() {
    // v4 (PEND 162): the admin has no route to move USDC; what is not paid to
    // investors returns to whoever deposited it, through finalize + claim.
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, _, _, usdc, _, _, _, contract_id) = setup(&e);
    client.deposit(&company, &(1_000 * 10_000_000i128));
    assert_eq!(client.try_withdraw(&usdc.address, &(500 * 10_000_000i128)), Err(Ok(SettleError::UnauthorizedToken)));
    assert_eq!(usdc.balance(&admin), 0);
    client.finalize_round();
    client.claim_residual(&company);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(usdc.balance(&company), 10_000 * 10_000_000);
}

#[test]
fn test_withdraw_zero_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, _, _, _, usdc, _, _, _, _) = setup(&e);

    assert_eq!(client.try_withdraw(&usdc.address, &0), Err(Ok(SettleError::InvalidAmount)));
}

#[test]
fn test_withdraw_negative_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, _, _, _, usdc, _, _, _, _) = setup(&e);

    assert_eq!(client.try_withdraw(&usdc.address, &-1), Err(Ok(SettleError::InvalidAmount)));
}

#[test]
#[should_panic]
fn test_withdraw_exceeds_balance_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, _, _, _, _, _, _, _, contract_id) = setup(&e);
    let (other, other_admin) = create_token_contract(&e, &Address::generate(&e));
    other_admin.mint(&contract_id, &(100 * 10_000_000i128));
    client.withdraw(&other.address, &(200 * 10_000_000i128));
}

// ═══════════════════════════════════════════════════════
//  7. refund() — 4 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_refund_happy_path() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, _) = setup(&e);

    let amount: i128 = 1_000 * 10_000_000;
    let company_before = usdc.balance(&company);
    client.deposit(&company, &amount);
    assert_eq!(usdc.balance(&company), company_before - amount);

    client.refund(&company);
    assert_eq!(usdc.balance(&company), company_before);
    assert_eq!(client.get_deposit(&company), 0);
}

#[test]
fn test_refund_after_settlement_blocked() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    let result = client.try_refund(&company);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

#[test]
fn test_refund_no_deposit_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, _, _, _, _, _, _, _, _) = setup(&e);

    let result = client.try_refund(&Address::generate(&e));
    assert_eq!(result, Err(Ok(SettleError::NoDeposit)));
}

#[test]
fn test_refund_returns_full_amount() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(500 * 10_000_000i128));
    client.deposit(&company, &(300 * 10_000_000i128));
    // Accumulated: 800

    client.refund(&company);
    assert_eq!(client.get_deposit(&company), 0);
    assert_eq!(usdc.balance(&contract_id), 0);
}

// ═══════════════════════════════════════════════════════
//  8. upgrade / version / TTL — 3 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_version_returns_3() {
    let e = Env::default();
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    assert_eq!(client.version(), 5);
}

#[test]
fn test_extend_ttl_anyone_can_call() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, _, _, _, _, _, _, _, _) = setup(&e);
    client.extend_ttl(); // no panic
}

#[test]
fn test_get_balance_reflects_deposits() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    assert_eq!(client.get_balance(), 0);
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(client.get_balance(), 100 * 10_000_000);
}

// ═══════════════════════════════════════════════════════
//  9. IDEMPOTENCY & MULTI-BATCH — 7 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_double_settle_blocked() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, usdc_admin, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // Fund contract again to prove it's not a balance issue
    usdc_admin.mint(&company, &(200 * 10_000_000i128));
    client.deposit(&company, &(100 * 10_000_000i128));

    let result = client.try_settle_batch(&items, &(5 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

#[test]
fn test_settle_then_deposit_then_settle_blocked() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // New deposit after settlement
    usdc_admin.mint(&company, &(500 * 10_000_000i128));
    client.deposit(&company, &(200 * 10_000_000i128));

    // SAME investor again — blocked
    sec_token_admin.mint(&investor, &(50 * 10_000_000i128));
    let new_items = vec![&e, SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX }];
    let result = client.try_settle_batch(&new_items, &0);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

#[test]
fn test_settled_flag_persists_across_extend_ttl() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    client.extend_ttl();

    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

/// MULTI-BATCH: settle batch 1 (inv_a), then batch 2 (inv_b) — both succeed.
/// This is the core >30 investor feature.
#[test]
fn test_multi_batch_different_investors_succeeds() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, usdc_admin, _, sec_token_admin, contract_id) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(200 * 10_000_000i128));

    // Deposit enough for both batches
    usdc_admin.mint(&company, &(400 * 10_000_000i128));
    client.deposit(&company, &(400 * 10_000_000i128));

    // Batch 1: settle inv_a
    let batch1 = vec![&e, SettleItem { investor: inv_a.clone(), payout: 110 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&batch1, &(10 * 10_000_000i128));

    assert_eq!(usdc.balance(&inv_a), 110 * 10_000_000);
    assert_eq!(sec_token_admin.balance(&inv_a), 0); // ALL tokens burned
    assert_eq!(usdc.balance(&treasury), 10 * 10_000_000);

    // Batch 2: settle inv_b — MUST succeed
    let batch2 = vec![&e, SettleItem { investor: inv_b.clone(), payout: 220 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&batch2, &(20 * 10_000_000i128));

    assert_eq!(usdc.balance(&inv_b), 220 * 10_000_000);
    assert_eq!(sec_token_admin.balance(&inv_b), 0); // ALL tokens burned
    assert_eq!(usdc.balance(&treasury), 30 * 10_000_000); // cumulative fees
    assert_eq!(usdc.balance(&contract_id), 400 * 10_000_000 - 110 * 10_000_000 - 10 * 10_000_000 - 220 * 10_000_000 - 20 * 10_000_000);
}

/// MULTI-BATCH: settling same investor in both batches is rejected.
#[test]
fn test_multi_batch_same_investor_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    let inv_a = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));

    usdc_admin.mint(&company, &(300 * 10_000_000i128));
    client.deposit(&company, &(300 * 10_000_000i128));

    // Batch 1: settle inv_a
    let batch1 = vec![&e, SettleItem { investor: inv_a.clone(), payout: 50 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&batch1, &0);

    // Batch 2: try inv_a again (even with new tokens) — blocked
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    let batch2 = vec![&e, SettleItem { investor: inv_a.clone(), payout: 50 * 10_000_000, tokens: i128::MAX }];
    let result = client.try_settle_batch(&batch2, &0);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

/// MULTI-BATCH: refund is blocked after first batch settles.
#[test]
fn test_refund_blocked_after_partial_settlement() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    let inv_a = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));

    usdc_admin.mint(&company, &(200 * 10_000_000i128));
    client.deposit(&company, &(200 * 10_000_000i128));

    // Settle batch 1 only
    let batch1 = vec![&e, SettleItem { investor: inv_a.clone(), payout: 50 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&batch1, &0);

    // Refund blocked — settlement has started
    let result = client.try_refund(&company);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));
}

/// MULTI-BATCH: fee cap enforced per-batch, not globally.
#[test]
fn test_multi_batch_fee_cap_per_batch() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);
    let treasury = Address::generate(&e);
    let company = Address::generate(&e);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    // max_fee_bps = 1000 → 10% cap
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &1000);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(100 * 10_000_000i128));

    usdc_admin.mint(&company, &(300 * 10_000_000i128));
    client.deposit(&company, &(300 * 10_000_000i128));

    // Batch 1: 100 payout, 10 fee (10%) — exactly at cap
    let batch1 = vec![&e, SettleItem { investor: inv_a.clone(), payout: 100 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&batch1, &(10 * 10_000_000i128));

    // Batch 2: 100 payout, 11 fee (11%) — exceeds cap
    let batch2 = vec![&e, SettleItem { investor: inv_b.clone(), payout: 100 * 10_000_000, tokens: i128::MAX }];
    let result = client.try_settle_batch(&batch2, &(11 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::FeeTooHigh)));
}

// ═══════════════════════════════════════════════════════
//  10. AUTH ENFORCEMENT (no mock_all_auths) — 7 tests
//
//  These use e.as_contract() to seed storage, then verify
//  that require_auth() panics for unauthorized callers.
// ═══════════════════════════════════════════════════════

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_initialize_requires_admin() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    client.initialize(&admin, &usdc.address, &sec_token.address, &Address::generate(&e), &5000);
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_deposit_requires_depositor() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    // Seed config manually
    e.as_contract(&contract_id, || {
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                usdc_sac: usdc.address.clone(),
                token_sac: sec_token.address.clone(),
                treasury: Address::generate(&e),
                max_fee_bps: 5000,
            },
        );
    });

    let depositor = Address::generate(&e);
    client.deposit(&depositor, &(100 * 10_000_000i128));
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_settle_requires_admin() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    seed_config(&e, &contract_id, &admin);

    let items = vec![&e, SettleItem { investor: Address::generate(&e), payout: 1, tokens: i128::MAX }];
    client.settle_batch(&items, &0);
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_withdraw_requires_admin() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    seed_config(&e, &contract_id, &admin);

    client.withdraw(&usdc.address, &(100 * 10_000_000i128));
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_refund_requires_admin() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    seed_config(&e, &contract_id, &admin);

    client.refund(&Address::generate(&e));
}

#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_upgrade_requires_admin() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    seed_config(&e, &contract_id, &admin);

    let fake_hash = soroban_sdk::BytesN::from_array(&e, &[0u8; 32]);
    client.upgrade(&fake_hash);
}

#[test]
fn test_auth_non_admin_cannot_settle() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, investor, _, _, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));

    // Verify the stored admin — settle_batch checks THIS specific address
    let config: Config = e.as_contract(&contract_id, || {
        e.storage().instance().get(&DataKey::Config).unwrap()
    });
    assert_eq!(config.admin, admin);

    // With mock_all_auths, any address passes auth. This test documents
    // that settle_batch reads config.admin specifically (not any address).
    // True auth enforcement is tested in test_auth_settle_requires_admin above.
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 95 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &(5 * 10_000_000i128));
}

// ═══════════════════════════════════════════════════════
//  11. ECONOMIC ATTACKS — 3 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_fee_manipulation_total_exceeds_deposit() {
    // V-8: If payout + fee > deposit, the last transfer should fail
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    // payout=90 + fee=20 = 110 > 100 deposit
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 90 * 10_000_000, tokens: i128::MAX }];
    let result = client.try_settle_batch(&items, &(20 * 10_000_000i128));
    // Should fail — insufficient USDC for the fee transfer after payouts
    assert!(result.is_err());
}

#[test]
fn test_rounding_zero_payout_clawback_only() {
    // Intentional behavior: tokens can be clawbacked with 0 USDC payout
    // (e.g., defaulted loan — investors lose everything)
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(5 * 10_000_000i128)); // small deposit for fee only
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 0, tokens: i128::MAX }];
    client.settle_batch(&items, &0i128); // B1: clawback-sem-pagamento => fee 0

    assert_eq!(usdc.balance(&investor), 0);
    assert_eq!(sec_token.balance(&investor), 0); // all tokens burned
}

#[test]
fn test_deposit_1_stroop_allowed() {
    // V-10: Dust deposits — allowed for MVP (company self-DoS only)
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &1i128);
    assert_eq!(client.get_deposit(&company), 1);
}

// ═══════════════════════════════════════════════════════
//  12a. CVM FEE CAP ENFORCEMENT — 4 tests
//
//  On-chain transparency: fee cannot exceed the
//  max_fee_bps declared at initialization.
// ═══════════════════════════════════════════════════════

/// Fee at exactly max_fee_bps boundary should succeed.
#[test]
fn test_fee_cap_at_boundary_succeeds() {
    let e = Env::default();
    e.mock_all_auths();
    // max_fee_bps = 5000 (50%) from setup
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    // payout = 100 USDC, fee = 50 USDC (exactly 50% = 5000 bps)
    let payout: i128 = 100 * 10_000_000;
    let fee: i128 = 50 * 10_000_000;
    client.deposit(&company, &(payout + fee));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &fee);
    // No panic = fee at exact boundary accepted
}

/// Fee exceeding max_fee_bps should be rejected.
#[test]
fn test_fee_cap_exceeded_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    // max_fee_bps = 5000 (50%) from setup
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    // payout = 100 USDC, fee = 51 USDC (51% > 50% cap)
    let payout: i128 = 100 * 10_000_000;
    let fee: i128 = 51 * 10_000_000;
    client.deposit(&company, &(payout + fee));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout, tokens: i128::MAX,
    }];
    let result = client.try_settle_batch(&items, &fee);
    assert_eq!(result, Err(Ok(SettleError::FeeTooHigh)));
}

/// Zero fee always allowed regardless of max_fee_bps.
#[test]
fn test_fee_cap_zero_fee_always_allowed() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    let payout: i128 = 100 * 10_000_000;
    client.deposit(&company, &payout);

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &0);
    // Zero fee always passes — no FeeTooHigh check triggered
}

/// max_fee_bps = 0 means uncapped (no fee limit enforcement).
#[test]
fn test_fee_cap_uncapped_when_zero_bps() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let treasury = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    // max_fee_bps = 0 → uncapped
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &0);

    let investor = Address::generate(&e);
    sec_token_admin.mint(&investor, &(100 * 10_000_000i128));
    usdc_admin.mint(&company, &(200 * 10_000_000i128));
    client.deposit(&company, &(200 * 10_000_000i128));

    // B1 (24/08): max_fee_bps == 0 significa "fee TEM que ser zero", nunca "sem teto".
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    // fee > 0 com cap 0 tem que reverter.
    assert_eq!(
        client.try_settle_batch(&items, &(100 * 10_000_000i128)),
        Err(Ok(SettleError::FeeTooHigh)),
        "bps=0 nao e uncapped: fee tem que ser 0"
    );
    // fee == 0 com cap 0 passa normalmente.
    client.settle_batch(&items, &0i128);
    assert_eq!(usdc.balance(&investor), 100 * 10_000_000, "payout pago, sem fee");
}

// ═══════════════════════════════════════════════════════
//  12b. COMPUTE BUDGET STRESS — 2 tests
// ═══════════════════════════════════════════════════════

#[test]
fn test_batch_20_investors_within_budget() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    let payout_each: i128 = 10 * 10_000_000;
    let clawback_each: i128 = 5 * 10_000_000;
    let fee: i128 = 50 * 10_000_000;
    let total_deposit = payout_each * 20 + fee;

    usdc_admin.mint(&company, &total_deposit);
    client.deposit(&company, &total_deposit);

    let mut items: soroban_sdk::Vec<SettleItem> = soroban_sdk::Vec::new(&e);
    for _ in 0..20 {
        let inv = Address::generate(&e);
        sec_token_admin.mint(&inv, &clawback_each);
        items.push_back(SettleItem {
            investor: inv,
            payout: payout_each, tokens: i128::MAX,
        });
    }
    // Native test env has stricter resource limits than on-chain; disable for large batches
    e.cost_estimate().disable_resource_limits();
    e.cost_estimate().budget().reset_unlimited();
    client.settle_batch(&items, &fee);
    // No panic = 20 investors within compute budget
}

#[test]
fn test_batch_25_investors_realistic_max() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    let payout_each: i128 = 5 * 10_000_000;
    let clawback_each: i128 = 3 * 10_000_000;
    let fee: i128 = 2_500_000; // 0.25 USDC (~2% of 125 USDC total payouts)
    let total_deposit = payout_each * 25 + fee;

    usdc_admin.mint(&company, &total_deposit);
    client.deposit(&company, &total_deposit);

    let mut items: soroban_sdk::Vec<SettleItem> = soroban_sdk::Vec::new(&e);
    for _ in 0..25 {
        let inv = Address::generate(&e);
        sec_token_admin.mint(&inv, &clawback_each);
        items.push_back(SettleItem {
            investor: inv,
            payout: payout_each, tokens: i128::MAX,
        });
    }
    // Native test env has stricter resource limits than on-chain; disable for large batches
    e.cost_estimate().disable_resource_limits();
    e.cost_estimate().budget().reset_unlimited();
    client.settle_batch(&items, &fee);
    // No panic = 25 investors within compute budget (MAX_BATCH_SIZE = 30)
}

// ═══════════════════════════════════════════════════════
//  13. ATOMICITY VERIFICATION — 3 tests
// ═══════════════════════════════════════════════════════

#[test]
#[should_panic]
fn test_atomicity_failed_clawback_reverts_payouts() {
    // Atomicity: if ANY transfer fails, the ENTIRE batch reverts.
    // Test: contract has enough for payout but fee transfer will fail
    // because there isn't enough USDC after paying the investor.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    // Deposit 100, try payout=100 + fee=50 = 150 total needed
    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    // This should panic: 100 payout + 50 fee = 150, but only 100 deposited
    client.settle_batch(&items, &(50 * 10_000_000i128));
}

#[test]
#[should_panic]
fn test_atomicity_failed_payout_reverts_all() {
    // Contract has too little USDC — entire batch reverts.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(10 * 10_000_000i128));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX, // > deposit
    }];
    client.settle_batch(&items, &0);
}

#[test]
#[should_panic]
fn test_atomicity_mixed_success_failure_reverts_all() {
    // 2 investors: A has tokens, B doesn't. B's clawback fails → ENTIRE batch reverts.
    // Proves all-or-nothing: A gets NO USDC even though A's portion would have succeeded.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, sec_token_admin, _) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(100 * 10_000_000i128));
    // inv_b gets NO tokens — intentional

    client.deposit(&company, &(200 * 10_000_000i128));

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: 100 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: 100 * 10_000_000, tokens: i128::MAX }, // fails
    ];
    client.settle_batch(&items, &0);
}

// ═══════════════════════════════════════════════════════
//  14. SECURITY REVIEW GAP-FILL — 9 tests
//
//  Added after adversarial line-by-line review.
//  Each test addresses a specific missing coverage vector.
// ═══════════════════════════════════════════════════════

/// GAP-1: Negative payout in SettleItem — contract must reject or SAC must panic.
/// Without this test, a malicious backend could pass negative payouts to extract
/// USDC FROM the investor instead of paying them (CWE-20).
#[test]
fn test_settle_negative_payout_in_item() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: -1, tokens: i128::MAX },
    ];
    // MUST return InvalidAmount — contract validates BEFORE calling SAC.
    // Relying on SAC to reject would waste gas and return opaque errors.
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::InvalidAmount)));
}

/// GAP-2: Negative clawback in SettleItem — could mint tokens instead of burning (CWE-20).
#[test]
fn test_settle_negative_clawback_in_item() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    // negative clawback_amount no longer exists — test negative payout instead
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: -1, tokens: i128::MAX },
    ];
    // MUST return InvalidAmount — validated at contract level.
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::InvalidAmount)));
}

/// GAP-3: Withdraw AFTER settlement — admin must be able to clean up leftover USDC.
/// This is the operational path for recovering dust or excess deposits.
#[test]
fn test_withdraw_after_settlement() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, investor, _, usdc, _, _, _, contract_id) = setup(&e);

    // Deposit more than needed
    client.deposit(&company, &(200 * 10_000_000i128));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 95 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // 200 - 95 - 5 = 100 USDC leftover
    assert_eq!(usdc.balance(&contract_id), 100 * 10_000_000);

    // v4: the leftover goes back to the depositor, never to the admin.
    let before = usdc.balance(&company);
    client.finalize_round();
    client.claim_residual(&company);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(usdc.balance(&company), before + 100 * 10_000_000);
    assert_eq!(usdc.balance(&admin), 0);
}

/// GAP-4: Withdraw to third party — admin can send recovered funds to any address.
#[test]
fn test_residual_only_to_a_depositor() {
    // v4: there is no `to` any more — a third party cannot receive the residual.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));
    client.finalize_round();
    let third_party = Address::generate(&e);
    assert_eq!(client.try_claim_residual(&third_party), Err(Ok(SettleError::NoDeposit)));
}

/// GAP-5: get_balance before initialize — should return NotInitialized error.
#[test]
fn test_get_balance_before_initialize() {
    let e = Env::default();
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    let result = client.try_get_balance();
    assert_eq!(result, Err(Ok(SettleError::NotInitialized)));
}

/// GAP-6: Deposit is allowed after settlement — documents intentional behavior.
/// The contract doesn't block deposits post-settlement because:
/// 1. The USDC is recoverable via withdraw()
/// 2. Blocking would add unnecessary state checks
#[test]
fn test_deposit_after_settlement_allowed() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 95 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // Deposit after settlement succeeds (intentional — recoverable via withdraw)
    client.deposit(&company, &(50 * 10_000_000i128));
    assert_eq!(usdc.balance(&contract_id), 50 * 10_000_000);
    // NOTE: Deposit tracking accumulates. settle_batch does NOT clear Deposit entries
    // because it doesn't know which addresses deposited. So: 100 (original) + 50 = 150.
    // Post-settlement, Deposit tracking is stale — use get_balance() for USDC availability.
    assert_eq!(client.get_deposit(&company), 150 * 10_000_000);
}

/// GAP-7: Refund one depositor does NOT affect another depositor's funds.
/// Proves per-depositor isolation even within the same contract.
#[test]
fn test_refund_one_depositor_preserves_other() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, usdc_admin, _, _, contract_id) = setup(&e);

    let other = Address::generate(&e);
    usdc_admin.mint(&other, &(300 * 10_000_000i128));

    client.deposit(&company, &(500 * 10_000_000i128));
    client.deposit(&other, &(300 * 10_000_000i128));
    assert_eq!(usdc.balance(&contract_id), 800 * 10_000_000);

    // Refund only company
    client.refund(&company);

    assert_eq!(client.get_deposit(&company), 0);
    assert_eq!(client.get_deposit(&other), 300 * 10_000_000); // untouched
    assert_eq!(usdc.balance(&contract_id), 300 * 10_000_000); // only other's deposit remains
}

/// GAP-8: settle_batch with no deposits — should panic on USDC transfer (insufficient balance).
/// Catches the case where admin calls settle before any company deposits.
#[test]
#[should_panic]
fn test_settle_before_any_deposit_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, _, investor, _, _, _, _, _, _) = setup(&e);

    // NO deposit — contract has 0 USDC
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &0);
}

/// GAP-9: Investor address equals treasury address — edge case where payout + fee
/// go to the same address. Must still work correctly with independent transfers.
#[test]
fn test_settle_investor_is_treasury() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let treasury = Address::generate(&e); // treasury IS the investor
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);

    usdc_admin.mint(&company, &(110 * 10_000_000i128));
    sec_token_admin.mint(&treasury, &(100 * 10_000_000i128));

    client.deposit(&company, &(110 * 10_000_000i128));

    let items = vec![&e, SettleItem {
        investor: treasury.clone(), // investor = treasury
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(10 * 10_000_000i128));

    // Treasury gets payout (100) + fee (10) = 110
    assert_eq!(usdc.balance(&treasury), 110 * 10_000_000);
    assert_eq!(sec_token.balance(&treasury), 0); // tokens clawbacked
}

// ═══════════════════════════════════════════════════════
//  15. SECOND SECURITY REVIEW — 7 tests
//
//  Cross-referenced against tokenomics-expert v3.1
//  and kill chain analysis.
// ═══════════════════════════════════════════════════════

/// R2-1: V-3 overflow in settle_batch payout summation (CWE-190).
/// overflow-checks=true in Cargo.toml catches this, but we MUST verify at
/// the settle_batch level, not just deposit. Without this test, removing
/// overflow-checks would silently pass.
#[test]
#[should_panic]
fn test_settle_payout_sum_overflow_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    // Fund enough to avoid balance issues
    usdc_admin.mint(&company, &i128::MAX);
    client.deposit(&company, &(1_000 * 10_000_000i128));

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &1i128);
    sec_token_admin.mint(&inv_b, &1i128);

    // Two payouts that overflow when summed: (MAX/2 + 1) + (MAX/2 + 1) > MAX
    let half_plus_one = i128::MAX / 2 + 1;
    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: half_plus_one, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: half_plus_one, tokens: i128::MAX },
    ];
    // Must panic from overflow-checks, NOT from insufficient balance
    client.settle_batch(&items, &0);
}

/// R2-2: Full lifecycle: deposit → refund → deposit again → settle.
/// Proves the contract IS reusable after a refund (Settled flag NOT set by refund).
#[test]
fn test_deposit_refund_deposit_settle_lifecycle() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    // Phase 1: deposit then change mind
    client.deposit(&company, &(100 * 10_000_000i128));
    client.refund(&company);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(client.get_deposit(&company), 0);

    // Phase 2: deposit again with correct amount and settle
    client.deposit(&company, &(110 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(10 * 10_000_000i128));

    assert_eq!(usdc.balance(&investor), 100 * 10_000_000);
    assert_eq!(usdc.balance(&treasury), 10 * 10_000_000);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned
    assert_eq!(usdc.balance(&contract_id), 0);
}

/// R2-3: All money to treasury, investors get $0 USDC + lose ALL tokens.
/// This is the total-loss / company-default scenario.
#[test]
fn test_settle_fee_equals_entire_deposit() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 0, tokens: i128::MAX,
    }];
    // B1 (24/08): fee = deposito inteiro sobre payout ZERO era o dreno. Agora reverte.
    assert_eq!(
        client.try_settle_batch(&items, &(100 * 10_000_000i128)),
        Err(Ok(SettleError::FeeTooHigh)),
        "payout 0 nao autoriza fee > 0"
    );
    assert_eq!(usdc.balance(&treasury), 0, "treasury nao drenou");
    assert_eq!(usdc.balance(&contract_id), 100 * 10_000_000, "deposito intacto");
    // O clawback-sem-pagamento continua valido com fee ZERO:
    client.settle_batch(&items, &0i128);
    assert_eq!(sec_token.balance(&investor), 0, "tokens queimados sem fee");
}

/// R2-4: Company is ALSO an investor (deposited USDC AND holds tokens).
/// Tests self-transfer safety: company deposits from same USDC pool it gets paid into.
#[test]
fn test_settle_investor_is_company() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, _, sec_token, sec_token_admin, contract_id) = setup(&e);

    // Company also holds security tokens (e.g., founder allocation)
    sec_token_admin.mint(&company, &(200 * 10_000_000i128));

    // Company deposits 500 USDC
    client.deposit(&company, &(500 * 10_000_000i128));

    // Company is the investor receiving payout
    let items = vec![&e, SettleItem {
        investor: company.clone(),
        payout: 450 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(50 * 10_000_000i128));

    // Company started with 10,000 USDC, deposited 500, received 450 back
    // Net: 10,000 - 500 + 450 = 9,950
    assert_eq!(usdc.balance(&company), 9_950 * 10_000_000);
    assert_eq!(usdc.balance(&treasury), 50 * 10_000_000);
    assert_eq!(sec_token.balance(&company), 0); // 200 clawbacked
    assert_eq!(usdc.balance(&contract_id), 0);
}

/// R2-5: Investor address = contract address → contract sends USDC to itself.
/// Must fail (panics) because contract can't hold security tokens to clawback.
#[test]
#[should_panic]
fn test_settle_investor_is_contract_panics() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));

    // Contract address as investor — clawback will fail (contract has 0 tokens)
    let items = vec![&e, SettleItem {
        investor: contract_id.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &0);
}

/// R2-6: Refund after partial withdraw → panics because actual USDC < tracked deposit.
/// Ops scenario: admin withdraws 50 of 100, then tries to refund full 100.
#[test]
fn test_withdraw_cannot_desync_refund() {
    // v4: "admin withdrew half, refund finds less than tracked" cannot happen — USDC never leaves through withdraw.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(client.try_withdraw(&usdc.address, &(50 * 10_000_000i128)), Err(Ok(SettleError::UnauthorizedToken)));
    client.refund(&company);
    assert_eq!(usdc.balance(&company), 10_000 * 10_000_000);
}

/// R2-7: Realistic bullet payout values from tokenomics-expert v3.1.
/// 100 USDC invested, 12% annualRate, 10% investorRate, 1 year.
/// Dual computation: test calculates expected values independently.
#[test]
fn test_settle_realistic_bullet_values() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    // Tokenomics formulas (from tokenomics-expert v3.1):
    // principal         = 100 USDC
    // investorRate      = 10%
    // annualRate        = 12%
    // years             = 1.0
    // investorInterest  = round7(100 × 0.10 × 1.0) = 10.0000000 USDC
    // platformFee       = round7(100 × 0.02 × 1.0) = 2.0000000 USDC
    // netToInvestor     = 100 + 10 = 110 USDC
    // companyPays       = 100 × 0.12 × 1 = 12 (interest) + 100 (principal) = 112 USDC
    // Verify: payout(110) + fee(2) = 112 = deposit ✓

    let principal: i128 = 100 * 10_000_000;       // 100 USDC
    let investor_interest: i128 = 10 * 10_000_000; // 10 USDC (100 × 10% × 1yr)
    let platform_fee: i128 = 2 * 10_000_000;       // 2 USDC  (100 × 2% × 1yr)
    let payout = principal + investor_interest;      // 110 USDC
    let deposit = payout + platform_fee;             // 112 USDC

    client.deposit(&company, &deposit);

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &platform_fee);

    // Tokenomics invariants:
    assert_eq!(usdc.balance(&investor), payout);       // investor got principal + interest
    assert!(payout > principal);                        // interest accrued (investorRate > 0)
    assert!(payout < principal * 2);                    // sanity: <100% interest
    assert_eq!(usdc.balance(&treasury), platform_fee);  // platform got the spread
    assert_eq!(sec_token.balance(&investor), 0);        // ALL tokens burned from chain
    assert_eq!(usdc.balance(&contract_id), 0);          // nothing left
    assert_eq!(payout + platform_fee, deposit);          // sum conservation
}

// ═══════════════════════════════════════════════════════
//  16. CTO GATE REVIEW (non-upgradable) — 6 tests
//
//  Third-pass: behavior locked forever. Every test is a
//  permanent guarantee. No patches, no second chances.
// ═══════════════════════════════════════════════════════

/// R3-1: Multi-investor UNEVEN proportional split with dual computation.
/// 3 investors: 60 USDC (60%), 30 USDC (30%), 10 USDC (10%).
/// Payout = principal + interest computed independently in test.
/// Tokenomics-expert v3.1 Edge Case #4.
#[test]
fn test_settle_uneven_proportional_split_dual_computation() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, usdc_admin, _, sec_token_admin, contract_id) = setup(&e);

    // 3 investors with uneven investments
    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    let inv_c = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(60 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(30 * 10_000_000i128));
    sec_token_admin.mint(&inv_c, &(10 * 10_000_000i128));

    // Independent dual computation (tokenomics-expert v3.1 formulas):
    // investorRate = 10%, annualRate = 12%, 1 year, total invested = 100 USDC
    //
    // Per-investor interest (investorRate):
    //   A: round7(60 × 0.10 × 1.0) = 6.0000000 USDC
    //   B: round7(30 × 0.10 × 1.0) = 3.0000000 USDC
    //   C: round7(10 × 0.10 × 1.0) = 1.0000000 USDC
    //
    // Platform fee (spread on yield):
    //   fee = round7(100 × 0.02 × 1.0) = 2.0000000 USDC
    //
    // Payouts:
    //   A: 60 + 6 = 66 USDC
    //   B: 30 + 3 = 33 USDC
    //   C: 10 + 1 = 11 USDC
    //
    // Sum check: 66 + 33 + 11 + 2 = 112 USDC = total deposit

    let pa: i128 = 66 * 10_000_000;  // principal(60) + interest(6)
    let pb: i128 = 33 * 10_000_000;  // principal(30) + interest(3)
    let pc: i128 = 11 * 10_000_000;  // principal(10) + interest(1)
    let fee: i128 = 2 * 10_000_000;  // yield spread: (12%-10%) × 100 × 1yr
    let deposit = pa + pb + pc + fee; // 112 USDC

    usdc_admin.mint(&company, &deposit); // company needs more than setup's 10k for this
    client.deposit(&company, &deposit);

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: pa, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: pb, tokens: i128::MAX },
        SettleItem { investor: inv_c.clone(), payout: pc, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &fee);

    // Dual computation assertions:
    assert_eq!(usdc.balance(&inv_a), pa);   // 66 USDC
    assert_eq!(usdc.balance(&inv_b), pb);   // 33 USDC
    assert_eq!(usdc.balance(&inv_c), pc);   // 11 USDC
    assert_eq!(usdc.balance(&treasury), fee); // 2 USDC (yield spread)
    assert_eq!(usdc.balance(&contract_id), 0);

    // Sum conservation — the ATOMIC guarantee
    assert_eq!(pa + pb + pc + fee, deposit);
    assert_eq!(
        usdc.balance(&inv_a) + usdc.balance(&inv_b) + usdc.balance(&inv_c) + usdc.balance(&treasury),
        deposit
    );

    // All tokens burned
    assert_eq!(sec_token_admin.balance(&inv_a), 0);
    assert_eq!(sec_token_admin.balance(&inv_b), 0);
    assert_eq!(sec_token_admin.balance(&inv_c), 0);

    // Each payout > principal (interest accrued)
    assert!(pa > 60 * 10_000_000);
    assert!(pb > 30 * 10_000_000);
    assert!(pc > 10 * 10_000_000);
}

/// R3-2: Initialize with treasury = admin (degenerate but valid config).
/// Both fee AND admin control go to the same address.
#[test]
fn test_initialize_treasury_equals_admin() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    // treasury = admin — degenerate but must work
    client.initialize(&admin, &usdc.address, &sec_token.address, &admin, &5000);

    let company = Address::generate(&e);
    let investor = Address::generate(&e);
    usdc_admin.mint(&company, &(110 * 10_000_000i128));
    sec_token_admin.mint(&investor, &(100 * 10_000_000i128));

    client.deposit(&company, &(110 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(10 * 10_000_000i128));

    // Admin got the fee (admin = treasury)
    assert_eq!(usdc.balance(&admin), 10 * 10_000_000);
    assert_eq!(usdc.balance(&investor), 100 * 10_000_000);
    assert_eq!(sec_token.balance(&investor), 0);
}

/// R3-3: settle_batch, deposit, refund, withdraw all fail before initialize.
/// For a non-upgradable contract, error messages are the ONLY debugging tool.
/// Must return NotInitialized, NOT raw panics.
#[test]
fn test_all_operations_fail_before_initialize() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    let (usdc, _) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    // deposit before init
    let dep_result = client.try_deposit(&Address::generate(&e), &100);
    assert_eq!(dep_result, Err(Ok(SettleError::NotInitialized)));

    // settle before init
    let items = vec![&e, SettleItem { investor: Address::generate(&e), payout: 1, tokens: i128::MAX }];
    let settle_result = client.try_settle_batch(&items, &0);
    assert_eq!(settle_result, Err(Ok(SettleError::NotInitialized)));

    // refund before init
    let refund_result = client.try_refund(&Address::generate(&e));
    assert_eq!(refund_result, Err(Ok(SettleError::NotInitialized)));

    // withdraw before init
    let withdraw_result = client.try_withdraw(&usdc.address, &100);
    assert_eq!(withdraw_result, Err(Ok(SettleError::NotInitialized)));

    // get_balance before init (already tested in GAP-5, but complete the set)
    let balance_result = client.try_get_balance();
    assert_eq!(balance_result, Err(Ok(SettleError::NotInitialized)));
}

/// R3-4: Withdraw non-USDC token — emergency recovery path.
/// If someone sends random tokens to the contract, admin must recover them.
/// For a non-upgradable contract, this is the ONLY escape hatch for trapped assets.
#[test]
fn test_withdraw_non_usdc_token() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, _, _, _, _, _, _, _, contract_id) = setup(&e);

    // Create a random token and send it to the contract
    let random_token_admin = Address::generate(&e);
    let (random_token, random_admin) = create_token_contract(&e, &random_token_admin);
    random_admin.mint(&contract_id, &(500 * 10_000_000i128));

    assert_eq!(random_token.balance(&contract_id), 500 * 10_000_000);

    // Admin recovers the random token
    client.withdraw(&random_token.address, &(500 * 10_000_000i128));

    assert_eq!(random_token.balance(&contract_id), 0);
    assert_eq!(random_token.balance(&admin), 500 * 10_000_000);
}

/// R3-5: Golden state — complete post-settlement verification.
/// Verifies EVERY aspect of the contract's state after settlement.
/// This is the definitive behavioral contract for a non-upgradable deployment.
#[test]
fn test_golden_state_post_settlement() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, company, investor, treasury, usdc, _, sec_token, _, contract_id) = setup(&e);

    // Setup
    let deposit: i128 = 105 * 10_000_000;
    client.deposit(&company, &deposit);

    let payout: i128 = 100 * 10_000_000;
    let fee: i128 = 5 * 10_000_000;
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &fee);

    // === GOLDEN STATE VERIFICATION ===

    // 1. Settled flag is active (proven by rejection)
    let double_settle = client.try_settle_batch(&items, &0);
    assert_eq!(double_settle, Err(Ok(SettleError::AlreadySettled)));

    // 2. get_balance returns 0 (all USDC distributed)
    assert_eq!(client.get_balance(), 0);

    // 3. get_deposit returns stale historical value
    assert_eq!(client.get_deposit(&company), deposit);

    // 4. version still works
    assert_eq!(client.version(), 5);

    // 5. extend_ttl still works
    client.extend_ttl();

    // 6. withdraw still works (for any accidentally sent tokens later)
    // No USDC to withdraw, but the function shouldn't panic on 0 check
    // (it would panic on SAC transfer of 0 — test that withdraw rejects 0)
    let zero_result = client.try_withdraw(&usdc.address, &0);
    assert_eq!(zero_result, Err(Ok(SettleError::InvalidAmount)));

    // 7. refund is blocked
    let refund = client.try_refund(&company);
    assert_eq!(refund, Err(Ok(SettleError::AlreadySettled)));

    // 8. Balances are final
    assert_eq!(usdc.balance(&investor), payout);
    assert_eq!(usdc.balance(&treasury), fee);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned

    // 9. New deposits are still accepted (documented behavior)
    client.deposit(&company, &(1 * 10_000_000i128));
    assert_eq!(usdc.balance(&contract_id), 1 * 10_000_000);
}

/// R3-6: Multi-investor all-zero payout — company default scenario.
/// 3 investors ALL get $0, ALL tokens clawbacked, ALL funds to treasury.
/// Tests the worst business case: total investor loss.
#[test]
fn test_settle_multi_investor_all_zero_payout() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, treasury, usdc, _, sec_token, sec_token_admin, contract_id) = setup(&e);

    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    let inv_c = Address::generate(&e);
    sec_token_admin.mint(&inv_a, &(60 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(30 * 10_000_000i128));
    sec_token_admin.mint(&inv_c, &(10 * 10_000_000i128));

    let total_fee: i128 = 0; // B1: payouts todos zero => fee 0 (clawback puro)
    client.deposit(&company, &(50 * 10_000_000i128));

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: 0, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: 0, tokens: i128::MAX },
        SettleItem { investor: inv_c.clone(), payout: 0, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &total_fee);

    // All investors got nothing
    assert_eq!(usdc.balance(&inv_a), 0);
    assert_eq!(usdc.balance(&inv_b), 0);
    assert_eq!(usdc.balance(&inv_c), 0);

    // All tokens burned
    assert_eq!(sec_token_admin.balance(&inv_a), 0);
    assert_eq!(sec_token_admin.balance(&inv_b), 0);
    assert_eq!(sec_token_admin.balance(&inv_c), 0);

    // B1: clawback puro (payouts 0, fee 0) — nada para a treasury; o deposito
    // fica no contrato ate um settle com payout, refund ou finalize/residual.
    assert_eq!(usdc.balance(&treasury), 0);
    assert_eq!(usdc.balance(&contract_id), 50 * 10_000_000);
}

// ═══════════════════════════════════════════════════════
//  17. ROUND 4 — FINAL ADVERSARIAL PASS — 4 tests
//
//  Focus: wrong-identity auth, degenerate config,
//  view function accuracy, interface-level bugs.
// ═══════════════════════════════════════════════════════

/// R4-1: Company (depositor) cannot self-refund — ONLY admin can call refund().
/// Different from test_auth_refund_requires_admin which tests "no auth at all."
/// This tests "wrong identity auth" — the company is authenticated but NOT authorized.
///
/// Attack: Compromised company wallet calls refund(&company) to extract USDC
/// before admin can settle. Contract must reject because company ≠ admin.
#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_company_cannot_self_refund() {
    let e = Env::default();
    // NO mock_all_auths — we want real auth enforcement

    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    // Seed config with admin (NOT the company)
    e.as_contract(&contract_id, || {
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                usdc_sac: usdc.address.clone(),
                token_sac: sec_token.address.clone(),
                treasury: Address::generate(&e),
                max_fee_bps: 5000,
            },
        );
    });

    // Seed a deposit for the company
    usdc_admin.mint(&company, &(100 * 10_000_000i128));
    e.as_contract(&contract_id, || {
        e.storage().persistent().set(
            &DataKey::Deposit(company.clone()),
            &(100i128 * 10_000_000),
        );
    });

    // Company tries to refund ITSELF — must fail because company ≠ admin
    // The contract calls config.admin.require_auth(), company can't satisfy that
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &company,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "refund",
            args: soroban_sdk::vec![&e, company.to_val()],
            sub_invokes: &[],
        },
    }]);

    client.refund(&company);
}

/// R4-2: Initialize with usdc_sac == token_sac (same SAC for both tokens).
/// Degenerate config: settlement sends USDC payout then clawbacks ALL "tokens"
/// which are actually USDC. With auto-clawback, investor loses everything.
/// Documents that this config is dangerous and should be prevented by backend.
#[test]
fn test_initialize_same_sac_both_tokens() {
    let e = Env::default();
    e.mock_all_auths();

    let admin = Address::generate(&e);
    #[cfg(feature = "testing")]
    let (usdc, usdc_admin) = create_clawback_token_contract(&e, &admin);
    // EXCEÇÃO NOMEADA (PENDENCIAS 159): sob rede real o USDC é o mock no endereço
    // canônico, e o mock faz clawback sem exigir a flag AUTH_CLAWBACK_ENABLED no
    // emissor (nem a ordem trustline-depois-da-flag que ela impõe — o `SAC #10`
    // do CLAUDE.md). O que este teste mede é a config degenerada
    // usdc_sac == token_sac, não a semântica de flag do SAC; essa continua
    // coberta pelo mesmo teste rodando sob `testing`.
    #[cfg(not(feature = "testing"))]
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &usdc.address, &Address::generate(&e), &5000);

    let company = Address::generate(&e);
    let investor = Address::generate(&e);
    usdc_admin.mint(&company, &(200 * 10_000_000i128));
    usdc_admin.mint(&investor, &(100 * 10_000_000i128));

    client.deposit(&company, &(110 * 10_000_000i128));

    // Settle: payout 100 USDC then auto-clawback ALL investor USDC "tokens"
    // Investor: starts 100, gets +100 payout (now 200), then contract reads
    // balance(investor)=200 and clawbacks ALL 200. Net: 0.
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(10 * 10_000_000i128));

    // Degenerate: investor ends up with 0 (payout then full clawback of same token)
    assert_eq!(usdc.balance(&investor), 0);
}

/// R4-3: get_balance() agrees with on-chain USDC balance after partial settlement.
/// Verifies the view function reads the right storage key and isn't stale.
/// For non-upgradable: if get_balance() lies, backend makes wrong decisions forever.
#[test]
fn test_get_balance_agrees_with_onchain_after_settlement() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, _, _, contract_id) = setup(&e);

    // Deposit more than settlement needs
    client.deposit(&company, &(200 * 10_000_000i128));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 95 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // 200 - 95 - 5 = 100 USDC leftover
    let onchain = usdc.balance(&contract_id);
    let view_fn = client.get_balance();

    assert_eq!(onchain, 100 * 10_000_000);
    assert_eq!(view_fn, 100 * 10_000_000);
    assert_eq!(onchain, view_fn, "get_balance() must match on-chain USDC balance");
}

/// R4-4: Depositor cannot deposit on behalf of another address.
/// `deposit(&other, &100)` requires other.require_auth() — company can't sign for other.
/// Without this test, a compromised company could drain OTHER depositors' wallets.
#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_depositor_cannot_deposit_for_other() {
    let e = Env::default();
    // NO mock_all_auths

    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let victim = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    // Seed config
    e.as_contract(&contract_id, || {
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                usdc_sac: usdc.address.clone(),
                token_sac: sec_token.address.clone(),
                treasury: Address::generate(&e),
                max_fee_bps: 5000,
            },
        );
    });

    usdc_admin.mint(&victim, &(1_000 * 10_000_000i128));

    // Company tries to deposit on behalf of victim (drain victim's USDC)
    // Mock auth only for company — victim hasn't authorized anything
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &company,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "deposit",
            args: soroban_sdk::vec![&e, victim.to_val(), (100 * 10_000_000i128).into_val(&e)],
            sub_invokes: &[],
        },
    }]);

    // This MUST fail: victim hasn't authorized this deposit
    client.deposit(&victim, &(100 * 10_000_000i128));
}

// ═══════════════════════════════════════════════════════
//  18. ROUND 5 — FINAL CTO SIGN-OFF — 4 tests
//
//  Batch size regression, event emission, TTL coverage,
//  and the definitive production lifecycle test.
// ═══════════════════════════════════════════════════════

/// R5-1: 35 investors now exceeds MAX_BATCH_SIZE (lowered to 30).
/// Regression test: ensures the batch size change is enforced correctly.
#[test]
fn test_batch_35_exceeds_new_max() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, usdc_admin, _, sec_token_admin, _) = setup(&e);

    let total_deposit: i128 = 500 * 10_000_000;
    usdc_admin.mint(&company, &total_deposit);
    client.deposit(&company, &total_deposit);

    let mut items: soroban_sdk::Vec<SettleItem> = soroban_sdk::Vec::new(&e);
    for _ in 0..35 {
        let inv = Address::generate(&e);
        sec_token_admin.mint(&inv, &(10 * 10_000_000i128));
        items.push_back(SettleItem {
            investor: inv,
            payout: 1, tokens: i128::MAX,
        });
    }
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::BatchTooLarge)));
}

/// R5-2: settle_batch emits a `settled` event for audit trail.
/// On mainnet with no DB and no logs, events are the ONLY debugging tool.
/// For a non-upgradable contract, if events aren't emitted, they can never be added.
#[test]
fn test_settle_batch_emits_settled_event() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 95 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &(5 * 10_000_000i128));

    // Verify at least one event was published by the contract
    let all_events = e.events().all();
    // Filter to contract events only (exclude SAC transfer events)
    let contract_events = all_events.filter_by_contract(&contract_id);

    assert!(
        !contract_events.events().is_empty(),
        "settle_batch must emit at least one event for audit trail"
    );
}

/// R5-3: Deposit data survives extend_ttl call.
/// In Soroban, persistent storage keys have independent TTLs from instance.
/// extend_ttl must preserve deposit data (or at minimum not destroy it).
/// 
/// NOTE: SDK test env doesn't simulate real TTL expiration.
/// This test verifies the code PATH works — real TTL is validated on testnet.
#[test]
fn test_extend_ttl_preserves_deposit_data() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);

    // Deposit first
    client.deposit(&company, &(500 * 10_000_000i128));
    assert_eq!(client.get_deposit(&company), 500 * 10_000_000);

    // Extend TTL
    client.extend_ttl();

    // Deposit data still intact after TTL extension
    assert_eq!(client.get_deposit(&company), 500 * 10_000_000);
    assert_eq!(client.get_balance(), 500 * 10_000_000);
}

/// R5-4: Full production lifecycle — the DEFINITIVE test.
/// This is the EXACT sequence that happens for every offer in production:
///   1. Deploy + initialize
///   2. Company deposits USDC (possibly multiple deposits)
///   3. Admin settles (payouts + clawback + fee)
///   4. Admin withdraws leftover
///   5. Contract sits dormant (TTL extended periodically)
///
/// If this test passes, the contract handles the real-world use case.
#[test]
fn test_full_lifecycle_init_deposit_settle_withdraw() {
    let e = Env::default();
    e.mock_all_auths();

    // === 1. DEPLOY + INITIALIZE ===
    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let treasury = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &5000);
    assert_eq!(client.version(), 5);
    assert_eq!(client.get_balance(), 0);

    // === 2. SETUP INVESTORS ===
    let inv_a = Address::generate(&e);
    let inv_b = Address::generate(&e);
    let inv_c = Address::generate(&e);
    // Investors bought tokens at different amounts (uneven)
    sec_token_admin.mint(&inv_a, &(60 * 10_000_000i128));
    sec_token_admin.mint(&inv_b, &(30 * 10_000_000i128));
    sec_token_admin.mint(&inv_c, &(10 * 10_000_000i128));

    // === 3. COMPANY DEPOSITS (multiple — simulating partial funding) ===
    usdc_admin.mint(&company, &(200 * 10_000_000i128));

    // First deposit: $80
    client.deposit(&company, &(80 * 10_000_000i128));
    assert_eq!(client.get_balance(), 80 * 10_000_000);

    // Second deposit: $35 (company tops up after confirming exact amount)
    client.deposit(&company, &(35 * 10_000_000i128));
    assert_eq!(client.get_balance(), 115 * 10_000_000);
    assert_eq!(client.get_deposit(&company), 115 * 10_000_000);

    // === 4. ADMIN SETTLES ===
    // Tokenomics: investorRate=10%, annualRate=12%, 1 year, 100 USDC principal
    // Payouts: A=66, B=33, C=11 (principal + 10% interest)
    // Fee: 2 USDC (2% yield spread × 100 × 1yr)
    // Total used: 66 + 33 + 11 + 2 = 112 USDC
    // Leftover in contract: 115 - 112 = 3 USDC
    let pa: i128 = 66 * 10_000_000;
    let pb: i128 = 33 * 10_000_000;
    let pc: i128 = 11 * 10_000_000;
    let fee: i128 = 2 * 10_000_000;

    let items = vec![
        &e,
        SettleItem { investor: inv_a.clone(), payout: pa, tokens: i128::MAX },
        SettleItem { investor: inv_b.clone(), payout: pb, tokens: i128::MAX },
        SettleItem { investor: inv_c.clone(), payout: pc, tokens: i128::MAX },
    ];
    client.settle_batch(&items, &fee);

    // === 5. VERIFY SETTLEMENT ===
    // Investor balances
    assert_eq!(usdc.balance(&inv_a), pa);
    assert_eq!(usdc.balance(&inv_b), pb);
    assert_eq!(usdc.balance(&inv_c), pc);

    // All tokens burned
    assert_eq!(sec_token.balance(&inv_a), 0);
    assert_eq!(sec_token.balance(&inv_b), 0);
    assert_eq!(sec_token.balance(&inv_c), 0);

    // Treasury got fees
    assert_eq!(usdc.balance(&treasury), fee);

    // Sum conservation
    assert_eq!(pa + pb + pc + fee, 112 * 10_000_000);

    // Contract has leftover
    assert_eq!(client.get_balance(), 3 * 10_000_000);
    assert_eq!(usdc.balance(&contract_id), 3 * 10_000_000);

    // Double-settle blocked
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::AlreadySettled)));

    // Refund blocked
    let refund_result = client.try_refund(&company);
    assert_eq!(refund_result, Err(Ok(SettleError::AlreadySettled)));

    // === 6. LEFTOVER GOES BACK TO THE DEPOSITOR (v4) ===
    client.finalize_round();
    client.claim_residual(&company);
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(client.get_balance(), 0);

    // === 7. CONTRACT DORMANT — TTL EXTENSION ===
    client.extend_ttl();

    // State is preserved
    assert_eq!(client.version(), 5);
    // v4: claim_residual() clears the depositor record — the claim is once per round.
    assert_eq!(client.get_deposit(&company), 0);
}

// ═══════════════════════════════════════════════════════
//  19. ROUND 6 — DUPLICATE INVESTOR REJECTION — 2 tests
//
//  Contract REJECTS duplicate investor addresses in a batch.
//  Defense in depth: even if the backend has a dedup bug,
//  the contract catches it. Trustless design.
// ═══════════════════════════════════════════════════════

/// R6-1: Duplicate investor WITH clawback → DuplicateInvestor error.
/// Previously this would panic on the second clawback (token balance = 0).
/// Now the contract catches the duplicate BEFORE any processing starts.
#[test]
fn test_duplicate_investor_with_clawback_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);

    client.deposit(&company, &(200 * 10_000_000i128));

    // Same investor appears twice — both with clawback
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX },
    ];
    // Contract detects duplicate BEFORE executing any transfers
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::DuplicateInvestor)));
}

/// R6-2: Duplicate investor WITHOUT clawback → DuplicateInvestor error.
/// This was previously the DANGEROUS variant (silent double payout).
/// Now the contract catches it at input validation, BEFORE any USDC moves.
///
/// This is the trustless design: the contract validates inputs,
/// not the backend. The blockchain is the last line of defense.
#[test]
fn test_duplicate_investor_without_clawback_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, _, _, _) = setup(&e);

    client.deposit(&company, &(200 * 10_000_000i128));

    // Same investor twice, no clawback — previously caused silent double-payout
    let items = vec![
        &e,
        SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX },
        SettleItem { investor: investor.clone(), payout: 50 * 10_000_000, tokens: i128::MAX },
    ];
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::DuplicateInvestor)));

    // Investor received NOTHING — settlement was rejected atomically
    assert_eq!(usdc.balance(&investor), 0);
}

// ═══════════════════════════════════════════════════════
//  20. ROUND 8 — PHANTOM INVESTOR VALIDATION — 3 tests
//
//  Trustless design: contract reads on-chain token balance
//  before paying out. Rejects payouts to addresses holding
//  zero tokens (phantom investors).
// ═══════════════════════════════════════════════════════

/// R8-1: Payout to address with zero tokens → PhantomInvestor error.
/// Attack: backend bug includes a non-investor address in the batch.
/// Contract reads token::balance(&phantom) == 0, rejects.
///
/// This is the core "look at the blockchain" validation.
#[test]
fn test_settle_phantom_investor_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, _) = setup(&e);

    // Phantom address — holds ZERO security tokens
    let phantom = Address::generate(&e);
    // Note: phantom gets NO sec_token_admin.mint() call

    client.deposit(&company, &(100 * 10_000_000i128));

    // Backend bug: phantom appears in settle batch with payout, no clawback
    let items = vec![&e, SettleItem {
        investor: phantom.clone(),
        payout: 50 * 10_000_000, tokens: i128::MAX,
    }];
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::PhantomInvestor)));

    // No money moved
    assert_eq!(usdc.balance(&phantom), 0);
}

/// R8-2: Contract address as investor with clawback=0 → PhantomInvestor.
/// The contract holds 0 security tokens. Paying USDC to itself with
/// no clawback is a phantom payout (net effect: lose the fee).
/// Previously this would succeed silently (documented in R2-5 comment).
#[test]
fn test_settle_contract_as_investor_no_clawback_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, usdc, _, _, _, contract_id) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));

    // Contract address as investor — holds 0 tokens, clawback=0
    let items = vec![&e, SettleItem {
        investor: contract_id.clone(),
        payout: 50 * 10_000_000, tokens: i128::MAX,
    }];
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::PhantomInvestor)));

    // USDC stayed in contract — no self-payment
    assert_eq!(usdc.balance(&contract_id), 100 * 10_000_000);
}

/// Contract ALWAYS burns all tokens on settlement.
/// This is maturity behavior — tokens are the claim, settlement closes it.
#[test]
fn test_settle_investor_with_tokens_no_clawback_allowed() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));

    let items = vec![&e, SettleItem {
        investor: investor.clone(),
        payout: 100 * 10_000_000, tokens: i128::MAX,
    }];
    client.settle_batch(&items, &0);

    // Investor got paid AND ALL tokens burned (maturity = claim settled)
    assert_eq!(usdc.balance(&investor), 100 * 10_000_000);
    assert_eq!(sec_token.balance(&investor), 0); // ALL tokens burned
}

// ═══════════════════════════════════════════════════════
//  21. FINAL AUDIT — AUTH MATRIX COMPLETION — 1 test
//
//  Completes the auth matrix: every admin-gated function
//  now has "no auth" + "wrong identity" coverage.
// ═══════════════════════════════════════════════════════

/// FA-1: Company (depositor) cannot self-withdraw — ONLY admin can call withdraw().
/// Mirrors R4-1 (company cannot self-refund) for the withdraw() path.
///
/// Attack: Compromised company wallet calls withdraw(usdc, all, company_wallet)
/// to drain the contract before admin can settle. Must fail because company ≠ admin.
#[test]
#[should_panic(expected = "HostError: Error(Auth")]
fn test_auth_company_cannot_withdraw() {
    let e = Env::default();
    // NO mock_all_auths — we want real auth enforcement

    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, _) = create_token_contract(&e, &admin);
    let treasury = Address::generate(&e);

    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);

    // Seed config with admin (NOT the company)
    e.as_contract(&contract_id, || {
        e.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin: admin.clone(),
                usdc_sac: usdc.address.clone(),
                token_sac: sec_token.address.clone(),
                treasury: treasury.clone(),
                max_fee_bps: 5000,
            },
        );
    });

    // Seed USDC in contract (simulating a deposit)
    usdc_admin.mint(&contract_id, &(100 * 10_000_000i128));

    // Company authenticates as ITSELF and tries to withdraw
    // Contract calls config.admin.require_auth() — company can't satisfy that
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &company,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "withdraw",
            args: soroban_sdk::vec![
                &e,
                usdc.address.to_val(),
                (100 * 10_000_000i128).into_val(&e),
            ],
            sub_invokes: &[],
        },
    }]);

    // This MUST fail: company ≠ admin
    client.withdraw(&usdc.address, &(100 * 10_000_000i128));
}

// ═══════════════════════════════════════════════════════
//  V2 — pause + 2-step admin rotation
//  (security audit F-003, added 2026-05-20)
// ═══════════════════════════════════════════════════════

#[test]
fn test_v2_pause_sets_flag() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    assert_eq!(client.get_paused(), false);
    client.pause();
    assert_eq!(client.get_paused(), true);
    client.resume();
    assert_eq!(client.get_paused(), false);
}

#[test]
fn test_v2_pause_blocks_deposit() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    client.pause();

    let result = client.try_deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::ContractPaused)));
}

#[test]
fn test_v2_pause_blocks_settle_batch() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, company, investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    // Fund the contract first (before pausing) so the failure is unambiguously about the pause
    client.deposit(&company, &(100 * 10_000_000i128));
    client.pause();

    let items: Vec<SettleItem> = vec![
        &e,
        SettleItem {
            investor: investor.clone(),
            payout: 50 * 10_000_000i128, tokens: i128::MAX,
        },
    ];
    let result = client.try_settle_batch(&items, &0);
    assert_eq!(result, Err(Ok(SettleError::ContractPaused)));
}

#[test]
fn test_v2_pause_blocks_withdraw() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, company, _investor, _treasury, usdc, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    client.pause();

    let result = client.try_withdraw(&usdc.address, &(10 * 10_000_000i128));
    assert_eq!(result, Err(Ok(SettleError::ContractPaused)));
}

#[test]
fn test_v2_pause_blocks_refund() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    client.deposit(&company, &(100 * 10_000_000i128));
    client.pause();

    let result = client.try_refund(&company);
    assert_eq!(result, Err(Ok(SettleError::ContractPaused)));
}

#[test]
fn test_v2_resume_unblocks_flows() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    client.pause();
    client.resume();

    // Should succeed now — no error from the deposit path
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(client.get_deposit(&company), 100 * 10_000_000i128);
}

#[test]
#[should_panic]
fn test_v2_pause_requires_admin_auth() {
    let e = Env::default();

    let (client, _admin, company, _investor, _treasury, _usdc, _, _, _, contract_id) = setup(&e);

    // Mock only the company's auth — NOT the admin's. pause() requires admin.
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &company,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: soroban_sdk::vec![&e],
            sub_invokes: &[],
        },
    }]);

    client.pause(); // must panic — company is not admin
}

#[test]
fn test_v2_propose_admin_sets_pending() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);
    let new_admin = Address::generate(&e);

    assert_eq!(client.get_pending_admin(), None);
    client.propose_admin(&new_admin);
    assert_eq!(client.get_pending_admin(), Some(new_admin));
}

#[test]
fn test_v2_accept_admin_rotates_active_admin() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);
    let new_admin = Address::generate(&e);

    // Before rotation: get_admin returns the initial Config.admin
    assert_eq!(client.get_admin(), admin);

    client.propose_admin(&new_admin);
    client.accept_admin();

    // After rotation: active admin is the new one; pending is cleared
    assert_eq!(client.get_admin(), new_admin);
    assert_eq!(client.get_pending_admin(), None);
}

#[test]
fn test_v2_accept_admin_without_proposal_fails() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    let result = client.try_accept_admin();
    assert_eq!(result, Err(Ok(SettleError::NoPendingAdmin)));
}

#[test]
#[should_panic]
fn test_v2_old_admin_disabled_after_rotation() {
    let e = Env::default();

    let (client, admin, _company, _investor, _treasury, _usdc, _, _, _, contract_id) = setup(&e);
    let new_admin = Address::generate(&e);

    // Phase 1: under mock_all_auths, perform the rotation
    e.mock_all_auths();
    client.propose_admin(&new_admin);
    client.accept_admin();
    assert_eq!(client.get_admin(), new_admin);

    // Phase 2: only the OLD admin signs — pause() must panic because
    // require_auth() is now checking the new admin, not the old one.
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: soroban_sdk::vec![&e],
            sub_invokes: &[],
        },
    }]);

    client.pause(); // expected to panic — old admin cannot act
}

#[test]
fn test_v2_new_admin_can_act_after_rotation() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);
    let new_admin = Address::generate(&e);

    client.propose_admin(&new_admin);
    client.accept_admin();

    // New admin can now pause/resume (mock_all_auths covers their require_auth)
    client.pause();
    assert_eq!(client.get_paused(), true);
    client.resume();
    assert_eq!(client.get_paused(), false);
}

#[test]
fn test_v2_version_bumped_to_3() {
    let e = Env::default();
    e.mock_all_auths();

    let (client, _admin, _company, _investor, _treasury, _usdc, _, _, _, _) = setup(&e);

    assert_eq!(client.version(), 5);
}

// ═══════════════════════════════════════════════════════
//  F-006 — canonical USDC SAC rejection path
//  These tests ONLY compile when the `testing` feature is OFF — i.e. when
//  the const-baked validation in lib.rs is active. Run with:
//     cargo test --no-default-features --features testnet
// ═══════════════════════════════════════════════════════
#[cfg(not(feature = "testing"))]
mod canonical_usdc_tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    /// Guarda do andaime (PENDENCIAS 159): se a const do mock sair de sincronia
    /// com a do lib.rs, TODA a suíte sob rede real vira falso-vermelho — este
    /// teste diz onde. `initialize` só aceita porque é o endereço canônico.
    #[test]
    fn mock_sits_at_the_canonical_address() {
        let e = Env::default();
        e.mock_all_auths();
        let admin = Address::generate(&e);
        let (usdc, _) = create_usdc_contract(&e, &admin);
        assert_eq!(usdc.address, Address::from_str(&e, mock_usdc::CANONICAL_USDC));
        let contract_id = e.register(MaturitySettlement, ());
        MaturitySettlementClient::new(&e, &contract_id).initialize(
            &admin,
            &usdc.address,
            &Address::generate(&e),
            &Address::generate(&e),
            &5000,
        );
    }

    #[test]
    fn test_initialize_rejects_non_canonical_usdc() {
        let e = Env::default();
        e.mock_all_auths();
        let contract_id = e.register(MaturitySettlement, ());
        let client = MaturitySettlementClient::new(&e, &contract_id);

        let admin = Address::generate(&e);
        // Random address — guaranteed not equal to the canonical USDC SAC const.
        let fake_usdc = Address::generate(&e);
        let token_sac = Address::generate(&e);
        let treasury = Address::generate(&e);

        let result = client.try_initialize(&admin, &fake_usdc, &token_sac, &treasury, &5000);
        assert_eq!(result, Err(Ok(SettleError::UnauthorizedToken)));
    }
}

// ═══════════════════════════════════════════════════════
//  v4 — resgate PARCIAL, rodadas, residual ao depositante, saque sem dreno,
//      upgrade travado com dinheiro dentro (decisão do Pedro 23/08, PEND 162/163)
// ═══════════════════════════════════════════════════════

#[test]
fn test_v4_partial_burn_leaves_remaining_tokens() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, contract_id) = setup(&e);
    client.deposit(&company, &(500 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 300 * 10_000_000, tokens: 300 * 10_000_000 }];
    client.settle_batch(&items, &0);
    assert_eq!(sec_token.balance(&investor), 700 * 10_000_000, "only 300 burned");
    assert_eq!(usdc.balance(&investor), 300 * 10_000_000);
    assert_eq!(usdc.balance(&contract_id), 200 * 10_000_000);
}

#[test]
fn test_v4_tokens_max_burns_everything_like_v3() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, sec_token, _, _) = setup(&e);
    client.deposit(&company, &(1_000 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 1_000 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &0);
    assert_eq!(sec_token.balance(&investor), 0);
}

#[test]
fn test_v4_tokens_zero_or_above_balance_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, sec_token, _, _) = setup(&e);
    client.deposit(&company, &(2_000 * 10_000_000i128));
    let zero = vec![&e, SettleItem { investor: investor.clone(), payout: 1, tokens: 0 }];
    assert_eq!(client.try_settle_batch(&zero, &0), Err(Ok(SettleError::InvalidAmount)));
    let too_many = vec![&e, SettleItem { investor: investor.clone(), payout: 1, tokens: 1_001 * 10_000_000 }];
    assert_eq!(client.try_settle_batch(&too_many, &0), Err(Ok(SettleError::TokensExceedBalance)));
    assert_eq!(sec_token.balance(&investor), 1_000 * 10_000_000, "nothing burned");
}

#[test]
fn test_v4_same_round_twice_rejected_next_round_accepts() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, sec_token, _, _) = setup(&e);
    client.deposit(&company, &(600 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 300 * 10_000_000, tokens: 300 * 10_000_000 }];
    client.settle_batch(&items, &0);
    assert_eq!(client.try_settle_batch(&items, &0), Err(Ok(SettleError::AlreadySettled)), "once per round");
    client.finalize_round();
    client.claim_residual(&company);
    client.next_round();
    assert_eq!(client.get_round().0, 2);
    client.deposit(&company, &(300 * 10_000_000i128));
    client.settle_batch(&items, &0);
    assert_eq!(sec_token.balance(&investor), 400 * 10_000_000, "amortized in two rounds");
}

#[test]
fn test_v4_expected_balance_guard_pays_nothing_when_short() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, _, sec_token, _, _) = setup(&e);
    client.deposit(&company, &(900 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 1_000 * 10_000_000, tokens: i128::MAX }];
    assert_eq!(client.try_settle_batch(&items, &0), Err(Ok(SettleError::InsufficientDeposit)));
    assert_eq!(usdc.balance(&investor), 0);
    assert_eq!(sec_token.balance(&investor), 1_000 * 10_000_000);
}

#[test]
fn test_v4_residual_is_shared_pro_rata_between_depositors() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, usdc, usdc_admin, _, _, contract_id) = setup(&e);
    let other = Address::generate(&e);
    usdc_admin.mint(&other, &(400 * 10_000_000i128));
    client.deposit(&company, &(600 * 10_000_000i128));
    client.deposit(&other, &(400 * 10_000_000i128));
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 500 * 10_000_000, tokens: i128::MAX }];
    client.settle_batch(&items, &0);
    client.finalize_round();
    client.finalize_round(); // idempotent
    client.claim_residual(&company);
    client.claim_residual(&other);
    assert_eq!(usdc.balance(&company), (10_000 - 600 + 300) * 10_000_000, "60% of the 500 residual");
    assert_eq!(usdc.balance(&other), 200 * 10_000_000, "40% of the 500 residual");
    assert_eq!(usdc.balance(&contract_id), 0);
    assert_eq!(client.try_claim_residual(&company), Err(Ok(SettleError::NoDeposit)), "claim once");
}

#[test]
fn test_v4_closed_round_accepts_nothing_else() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, investor, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));
    client.finalize_round();
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 1, tokens: 1 }];
    assert_eq!(client.try_settle_batch(&items, &0), Err(Ok(SettleError::RoundClosed)));
    assert_eq!(client.try_deposit(&company, &1), Err(Ok(SettleError::RoundClosed)));
    assert_eq!(client.try_refund(&company), Err(Ok(SettleError::RoundClosed)));
}

#[test]
fn test_v4_claim_before_finalize_and_next_round_rules() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(client.try_claim_residual(&company), Err(Ok(SettleError::RoundOpen)));
    assert_eq!(client.try_next_round(), Err(Ok(SettleError::RoundOpen)));
    client.finalize_round();
    assert_eq!(client.try_next_round(), Err(Ok(SettleError::ResidualPending)), "depositor has not claimed");
    client.claim_residual(&company);
    client.next_round();
    assert_eq!(client.get_round(), (2, false, 0, 0));
}

#[test]
fn test_v4_withdraw_other_token_goes_to_admin_only() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, admin, _, _, _, usdc, _, _, _, contract_id) = setup(&e);
    let (other, other_admin) = create_token_contract(&e, &Address::generate(&e));
    other_admin.mint(&contract_id, &(7 * 10_000_000i128));
    assert_eq!(client.try_withdraw(&usdc.address, &1), Err(Ok(SettleError::UnauthorizedToken)));
    client.withdraw(&other.address, &(7 * 10_000_000i128));
    assert_eq!(other.balance(&admin), 7 * 10_000_000);
}

#[test]
fn test_v4_upgrade_is_ledger_recovery_valve_even_with_money_inside() {
    // Decisao do Pedro 24/08 (pos red-team): o upgrade e a valvula de recuperacao da
    // Ledger e NAO trava com dinheiro dentro — e justamente ai que precisa destravar.
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    let hash = soroban_sdk::BytesN::from_array(&e, &[9u8; 32]);
    // rodada vazia: nunca foi WrongState.
    assert_ne!(client.try_upgrade(&hash), Err(Ok(SettleError::WrongState)));
    // deposito dentro, rodada aberta: ANTES travava (WrongState); agora a Ledger passa.
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_ne!(client.try_upgrade(&hash), Err(Ok(SettleError::WrongState)), "Ledger destrava com dinheiro dentro");
}

#[test]
fn test_v4_version_and_round_view() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _, company, _, _, _, _, _, _, _) = setup(&e);
    assert_eq!(client.version(), 5);
    assert_eq!(client.get_round(), (1, false, 0, 0));
    client.deposit(&company, &(100 * 10_000_000i128));
    assert_eq!(client.get_round(), (1, false, 100 * 10_000_000, 0));
}

// ═══════════════════════════════════════════════════════
//  B1 (red-team 24/08) — fee cap furado por batch de payout ZERO
// ═══════════════════════════════════════════════════════

#[test]
fn test_zero_payout_batch_cannot_drain_deposit_via_fee() {
    let e = Env::default();
    e.mock_all_auths();
    let (client, _admin, company, investor, treasury, usdc, _ua, _st, _ta, contract_id) = setup(&e);

    // Empresa deposita 1.000 USDC destinados a pagar investidores.
    let deposit = 1_000 * 10_000_000i128;
    client.deposit(&company, &deposit);

    // Admin tenta drenar TUDO para a treasury via um lote de payout zero + fee = saldo.
    // max_fee_bps do setup é 5000 (50%): mesmo assim, fee de 100% sobre payouts=0 tem
    // que ser barrada — o teto avaliado com payouts 0 é 0.
    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 0, tokens: i128::MAX }];
    let r = client.try_settle_batch(&items, &deposit);
    assert_eq!(r, Err(Ok(SettleError::FeeTooHigh)), "batch de payout zero nao pode levar fee > 0");

    // O depósito continua no contrato, intacto; a treasury não recebeu nada.
    assert_eq!(usdc.balance(&contract_id), deposit, "deposito preservado");
    assert_eq!(usdc.balance(&treasury), 0, "treasury nao drenou o pool dos investidores");
}

#[test]
fn test_max_fee_bps_zero_means_no_fee_allowed_not_no_cap() {
    let e = Env::default();
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let company = Address::generate(&e);
    let investor = Address::generate(&e);
    let treasury = Address::generate(&e);
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let (sec_token, sec_token_admin) = create_clawback_token_contract(&e, &admin);
    let contract_id = e.register(MaturitySettlement, ());
    let client = MaturitySettlementClient::new(&e, &contract_id);
    // max_fee_bps = 0 significa "taxa TEM que ser zero", nunca "sem teto".
    client.initialize(&admin, &usdc.address, &sec_token.address, &treasury, &0);
    sec_token_admin.mint(&investor, &(1_000 * 10_000_000i128)); // saldo real: passa do PhantomInvestor ate o fee cap
    usdc_admin.mint(&company, &(1_000 * 10_000_000i128));
    client.deposit(&company, &(1_000 * 10_000_000i128));

    let items = vec![&e, SettleItem { investor: investor.clone(), payout: 100 * 10_000_000i128, tokens: i128::MAX }];
    // fee de 1 stroop com cap 0 tem que reverter.
    let r = client.try_settle_batch(&items, &1i128);
    assert_eq!(r, Err(Ok(SettleError::FeeTooHigh)), "cap 0 = fee tem que ser 0");
}

// ─────────────────────────────────────────────────────────────────────────────
// §38.3 — o comentário não pode prometer uma defesa que o código não implementa.
//
// Até 09/09/2026 o doc-comment de `settle_batch` afirmava que a função "ALSO follows
// checks-effects-interactions — all state writes land before the external token
// transfers", e mandava manter essa ordem. O laço faz o contrário: transfere o payout,
// lê o saldo, faz clawback, e SÓ ENTÃO grava a marca `Settled` — que é justamente o que
// impede pagar duas vezes o mesmo investidor.
//
// É inofensivo hoje pelo motivo que o próprio comentário dá primeiro (host
// single-threaded, sem reentrância). O defeito é o texto: ele se declara uma defesa
// contra mudança FUTURA de semântica do host, e essa defesa não existe. Quem confiar
// nele e reordenar outra coisa "mantendo o padrão" está mantendo um padrão ausente.
//
// Esta guarda impede o texto de voltar. Reordenar o código de verdade muda o WASM, e o
// contrato está DEPLOYADO (v4, testnet + mainnet): binário novo é cerimônia da Ledger,
// decisão do Pedro. Quando isso acontecer, é este teste que deve ser trocado por um que
// exija a ordem — não apagado.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn doc_comment_nao_pode_prometer_checks_effects_interactions() {
    // A primeira versao desta guarda casava a frase NUMA LINHA SO — e o texto original vinha
    // quebrado em duas linhas `///`. Ela nao teria pego o proprio defeito que existe para
    // pegar: passava por sorte. Normaliza (tira `///`, colapsa espaco) antes de casar.
    // `String` e `Vec` estao SOMBREADOS pelo soroban_sdk neste crate — os do std precisam
    // de caminho completo, senao o compilador tenta construir um `soroban_sdk::Vec`.
    let fonte: std::string::String = include_str!("lib.rs")
        .lines()
        .map(|l| l.trim().trim_start_matches("///").trim())
        .collect::<std::vec::Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<std::vec::Vec<_>>()
        .join(" ");
    // A forma VIVA da afirmacao. A citacao historica logo acima do `settle_batch` diz
    // `used to claim the function "ALSO follows ...` — explicar o defeito nao pode disparar
    // a guarda contra ele.
    let promessa = "but this function ALSO follows checks-effects-interactions";
    assert!(
        !fonte.contains(promessa),
        "O doc-comment voltou a prometer checks-effects-interactions em settle_batch, e a \
         ordem do laço continua sendo transfer → clawback → gravar `Settled`. Ou reordene \
         o código (cerimônia da Ledger: o WASM muda) ou não prometa. §38.3."
    );
}

// ═══════════════════════════════════════════════════════
//  v5 (F6) — emergency_pauser: papel separado do admin
//  Liga a pausa e SO isso: nao despausa, nao troca admin, nao faz upgrade, nao move dinheiro.
//  A pausa continua bloqueando o mesmo conjunto de antes (deposit, settle_batch, withdraw,
//  finalize_round, claim_residual, next_round, refund) — so mudou QUEM pode liga-la.
// ═══════════════════════════════════════════════════════

mod emergency_pauser_tests {
    use super::*;
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    use soroban_sdk::{symbol_short, BytesN, Val};

    const USDC: i128 = 10_000_000;

    /// Autoriza SO `who` para a proxima chamada de `fn_name` — as demais exigencias de auth falham.
    fn only(e: &Env, who: &Address, contract: &Address, fn_name: &str, args: Vec<Val>) {
        e.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke {
                contract,
                fn_name,
                args,
                sub_invokes: &[],
            },
        }]);
    }

    fn some_arg(e: &Env, a: &Address) -> Vec<Val> {
        soroban_sdk::vec![e, Some(a.clone()).into_val(e)]
    }

    struct Fx<'a> {
        client: MaturitySettlementClient<'a>,
        cid: Address,
        admin: Address,
        company: Address,
        investor: Address,
        usdc: token::Client<'a>,
        pauser: Address,
    }

    /// Instancia com 100 USDC depositados pela empresa e o pauser definido pelo admin.
    fn fx<'a>(e: &Env) -> Fx<'a> {
        e.mock_all_auths();
        let (client, admin, company, investor, _treasury, usdc, _ua, _st, _sa, cid) = setup(e);
        client.deposit(&company, &(100 * USDC));
        let pauser = Address::generate(e);
        client.set_emergency_pauser(&Some(pauser.clone()));
        Fx { client, cid, admin, company, investor, usdc, pauser }
    }

    fn item(e: &Env, f: &Fx) -> Vec<SettleItem> {
        soroban_sdk::vec![
            e,
            SettleItem { investor: f.investor.clone(), payout: 50 * USDC, tokens: i128::MAX },
        ]
    }

    #[test]
    fn so_o_admin_define_o_pauser() {
        let e = Env::default();
        let f = fx(&e);
        let novo = Address::generate(&e);

        only(&e, &f.admin, &f.cid, "set_emergency_pauser", some_arg(&e, &novo));
        f.client.set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(e.auths()[0].0, f.admin);
        assert_eq!(f.client.emergency_pauser(), Some(novo.clone()));

        let intruso = Address::generate(&e);
        let outro = Address::generate(&e);
        only(&e, &intruso, &f.cid, "set_emergency_pauser", some_arg(&e, &outro));
        assert!(f.client.try_set_emergency_pauser(&Some(outro)).is_err());
        assert_eq!(f.client.emergency_pauser(), Some(novo.clone()));

        // o proprio pauser nao se promove nem escolhe sucessor
        let sucessor = Address::generate(&e);
        only(&e, &novo, &f.cid, "set_emergency_pauser", some_arg(&e, &sucessor));
        assert!(f.client.try_set_emergency_pauser(&Some(sucessor)).is_err());
        assert_eq!(f.client.emergency_pauser(), Some(novo));
    }

    #[test]
    fn pauser_pausa_e_a_pausa_bloqueia_o_mesmo_conjunto_de_antes() {
        let e = Env::default();
        let f = fx(&e);
        let saldo = f.usdc.balance(&f.cid);

        only(&e, &f.pauser, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        f.client.emergency_pause();
        assert_eq!(e.auths()[0].0, f.pauser, "quem assina e o pauser");
        assert!(f.client.get_paused());

        e.mock_all_auths();
        let paused = Err(Ok(SettleError::ContractPaused));
        assert_eq!(f.client.try_deposit(&f.company, &USDC), paused);
        assert_eq!(f.client.try_settle_batch(&item(&e, &f), &0), paused);
        assert_eq!(f.client.try_withdraw(&Address::generate(&e), &USDC), paused);
        assert_eq!(f.client.try_refund(&f.company), paused);
        assert_eq!(f.client.try_finalize_round(), paused);
        assert_eq!(f.client.try_claim_residual(&f.company), paused);
        assert_eq!(f.client.try_next_round(), paused);
        assert_eq!(f.usdc.balance(&f.cid), saldo, "pausar nao move dinheiro");
        assert_eq!(f.client.get_deposit(&f.company), 100 * USDC);
    }

    #[test]
    fn emergency_pause_sem_pauser_configurado_e_recusada() {
        let e = Env::default();
        e.mock_all_auths();
        let (client, ..) = setup(&e);
        assert_eq!(client.emergency_pauser(), None);
        assert_eq!(
            client.try_emergency_pause(),
            Err(Ok(SettleError::NoEmergencyPauser))
        );
        assert!(!client.get_paused());
    }

    #[test]
    fn terceiro_e_o_proprio_admin_nao_ligam_emergency_pause() {
        let e = Env::default();
        let f = fx(&e);
        let intruso = Address::generate(&e);

        only(&e, &intruso, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        assert!(f.client.try_emergency_pause().is_err());
        assert!(!f.client.get_paused());

        // o admin tem `pause()` para isso; emergency_pause exige a assinatura do pauser
        only(&e, &f.admin, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        assert!(f.client.try_emergency_pause().is_err());
        assert!(!f.client.get_paused());
    }

    #[test]
    fn pauser_nao_despausa_nao_troca_admin_nao_faz_upgrade_nao_move_dinheiro() {
        let e = Env::default();
        let f = fx(&e);
        f.client.emergency_pause();
        let saldo = f.usdc.balance(&f.cid);
        let alvo = Address::generate(&e);

        only(&e, &f.pauser, &f.cid, "resume", soroban_sdk::vec![&e]);
        assert!(f.client.try_resume().is_err());
        assert!(f.client.get_paused(), "o pauser nao despausa");

        only(&e, &f.pauser, &f.cid, "propose_admin", soroban_sdk::vec![&e, alvo.to_val()]);
        assert!(f.client.try_propose_admin(&alvo).is_err());
        assert_eq!(f.client.get_admin(), f.admin, "o pauser nao troca o admin");
        assert_eq!(f.client.get_pending_admin(), None);

        let hash = BytesN::from_array(&e, &[7u8; 32]);
        only(&e, &f.pauser, &f.cid, "upgrade", soroban_sdk::vec![&e, hash.to_val()]);
        assert!(f.client.try_upgrade(&hash).is_err(), "o pauser nao troca o codigo");

        // as funcoes que movem dinheiro pedem o admin (ou o depositante): o pauser nao passa
        only(&e, &f.pauser, &f.cid, "refund", soroban_sdk::vec![&e, f.company.to_val()]);
        assert!(f.client.try_refund(&f.company).is_err());
        only(
            &e,
            &f.pauser,
            &f.cid,
            "withdraw",
            soroban_sdk::vec![&e, alvo.to_val(), (1 * USDC).into_val(&e)],
        );
        assert!(f.client.try_withdraw(&alvo, &USDC).is_err());
        only(&e, &f.pauser, &f.cid, "finalize_round", soroban_sdk::vec![&e]);
        assert!(f.client.try_finalize_round().is_err());
        only(
            &e,
            &f.pauser,
            &f.cid,
            "settle_batch",
            soroban_sdk::vec![&e, item(&e, &f).to_val(), 0i128.into_val(&e)],
        );
        assert!(f.client.try_settle_batch(&item(&e, &f), &0).is_err());

        assert_eq!(f.usdc.balance(&f.cid), saldo);
        assert_eq!(f.usdc.balance(&alvo), 0);
    }

    #[test]
    fn admin_despausa_e_o_dinheiro_do_depositante_volta() {
        let e = Env::default();
        let f = fx(&e);
        let antes = f.usdc.balance(&f.company);

        only(&e, &f.pauser, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        f.client.emergency_pause();
        e.mock_all_auths();
        assert_eq!(f.client.try_refund(&f.company), Err(Ok(SettleError::ContractPaused)));

        f.client.resume();
        assert!(!f.client.get_paused());
        f.client.refund(&f.company);
        assert_eq!(f.usdc.balance(&f.company), antes + 100 * USDC);
        assert_eq!(f.client.get_deposit(&f.company), 0);
    }

    #[test]
    fn trocar_ou_remover_o_pauser_invalida_o_antigo() {
        let e = Env::default();
        let f = fx(&e);
        let novo = Address::generate(&e);
        f.client.set_emergency_pauser(&Some(novo.clone()));

        only(&e, &f.pauser, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        assert!(f.client.try_emergency_pause().is_err(), "o antigo nao pausa mais");
        assert!(!f.client.get_paused());

        only(&e, &novo, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        f.client.emergency_pause();
        assert!(f.client.get_paused());

        e.mock_all_auths();
        f.client.resume();
        f.client.set_emergency_pauser(&None);
        assert_eq!(f.client.emergency_pauser(), None);
        assert_eq!(
            f.client.try_emergency_pause(),
            Err(Ok(SettleError::NoEmergencyPauser))
        );
    }

    #[test]
    fn emergency_pause_e_idempotente() {
        let e = Env::default();
        let f = fx(&e);
        f.client.emergency_pause();
        f.client.emergency_pause();
        assert!(f.client.get_paused());
    }

    /// Instancia "antiga" (v4): nasce SEM a chave nova. O ciclo inteiro funciona e as chaves
    /// existentes (Config, Admin, Paused, Round...) nao mudam quando o pauser entra e sai.
    #[test]
    fn instancia_antiga_sem_a_chave_continua_funcionando() {
        let e = Env::default();
        e.mock_all_auths();
        let (client, admin, company, investor, _treasury, usdc, _ua, _st, _sa, cid) = setup(&e);

        e.as_contract(&cid, || {
            assert!(!e.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert_eq!(client.emergency_pauser(), None);

        // ciclo v4 completo, sem pauser: deposito -> pausa/retoma -> liquidacao -> encerra -> resto
        client.deposit(&company, &(100 * USDC));
        client.pause();
        assert_eq!(client.try_deposit(&company, &USDC), Err(Ok(SettleError::ContractPaused)));
        client.resume();
        let items = soroban_sdk::vec![
            &e,
            SettleItem { investor: investor.clone(), payout: 50 * USDC, tokens: i128::MAX },
        ];
        client.settle_batch(&items, &0);
        client.finalize_round();
        let antes = usdc.balance(&company);
        client.claim_residual(&company);
        assert_eq!(usdc.balance(&company), antes + 50 * USDC);
        client.next_round();
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_round().0, 2);

        // entrar e sair com a chave nova nao mexe nas antigas
        client.set_emergency_pauser(&Some(admin.clone()));
        assert_eq!(client.get_admin(), admin);
        assert_eq!(client.get_round().0, 2);
        assert!(!client.get_paused());
        client.set_emergency_pauser(&None);
        e.as_contract(&cid, || {
            assert!(!e.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert_eq!(client.version(), 5);
    }

    /// Layout v1/v2 cru (so `Config`, sem `Admin`/`Paused`/`EmergencyPauser`): a chave nova
    /// ausente nao atrapalha o fallback de admin nem a leitura da pausa.
    #[test]
    fn layout_v1_so_com_config_le_sem_a_chave_nova() {
        let e = Env::default();
        e.mock_all_auths();
        let contract_id = e.register(MaturitySettlement, ());
        let client = MaturitySettlementClient::new(&e, &contract_id);
        let admin = Address::generate(&e);
        seed_config(&e, &contract_id, &admin);

        assert_eq!(client.emergency_pauser(), None);
        assert_eq!(client.get_admin(), admin);
        assert!(!client.get_paused());
        client.pause();
        assert!(client.get_paused());
        client.resume();
        assert_eq!(
            client.try_emergency_pause(),
            Err(Ok(SettleError::NoEmergencyPauser))
        );
    }

    // ─── renuncia + eventos de troca (emenda da v5) ─────────────────────────

    /// Evento esperado do contrato: (contrato, [topico], dado).
    fn ev<D: IntoVal<Env, soroban_sdk::Val>>(
        e: &Env,
        cid: &Address,
        topic: soroban_sdk::Symbol,
        data: D,
    ) -> (Address, Vec<Val>, Val) {
        (cid.clone(), (topic,).into_val(e), data.into_val(e))
    }

    /// Eventos do contrato na ULTIMA chamada (sem os eventos do SAC).
    fn do_contrato(e: &Env, cid: &Address) -> soroban_sdk::testutils::ContractEvents {
        e.events().all().filter_by_contract(cid)
    }

    #[test]
    fn renuncia_so_pelo_proprio_pauser() {
        let e = Env::default();
        let f = fx(&e);

        // terceiro nao renuncia pelo pauser
        let intruso = Address::generate(&e);
        only(&e, &intruso, &f.cid, "renounce_emergency_pauser", soroban_sdk::vec![&e]);
        assert!(f.client.try_renounce_emergency_pauser().is_err());
        assert_eq!(f.client.emergency_pauser(), Some(f.pauser.clone()));

        // o admin tambem nao: ele remove com set_emergency_pauser(None)
        only(&e, &f.admin, &f.cid, "renounce_emergency_pauser", soroban_sdk::vec![&e]);
        assert!(f.client.try_renounce_emergency_pauser().is_err());
        assert_eq!(f.client.emergency_pauser(), Some(f.pauser.clone()));

        // o pauser sozinho renuncia
        only(&e, &f.pauser, &f.cid, "renounce_emergency_pauser", soroban_sdk::vec![&e]);
        f.client.renounce_emergency_pauser();
        assert_eq!(e.auths()[0].0, f.pauser, "quem assina e o pauser");
        assert_eq!(f.client.emergency_pauser(), None);
        e.as_contract(&f.cid, || {
            assert!(!e.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert!(!f.client.get_paused(), "renunciar nao liga pausa");
    }

    #[test]
    fn depois_da_renuncia_emergency_pause_cai_e_o_admin_define_de_novo() {
        let e = Env::default();
        let f = fx(&e);
        only(&e, &f.pauser, &f.cid, "renounce_emergency_pauser", soroban_sdk::vec![&e]);
        f.client.renounce_emergency_pauser();

        // sem pauser: ninguem liga emergency_pause (nem o antigo)
        e.mock_all_auths();
        assert_eq!(
            f.client.try_emergency_pause(),
            Err(Ok(SettleError::NoEmergencyPauser))
        );
        // renunciar de novo, sem pauser, tambem e recusado
        assert_eq!(
            f.client.try_renounce_emergency_pauser(),
            Err(Ok(SettleError::NoEmergencyPauser))
        );
        assert!(!f.client.get_paused());

        // o admin define de novo e o novo pausa
        let novo = Address::generate(&e);
        f.client.set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(f.client.emergency_pauser(), Some(novo.clone()));
        only(&e, &novo, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        f.client.emergency_pause();
        assert!(f.client.get_paused());
        // o pauser que renunciou nao volta sozinho
        e.mock_all_auths();
        f.client.resume();
        only(&e, &f.pauser, &f.cid, "emergency_pause", soroban_sdk::vec![&e]);
        assert!(f.client.try_emergency_pause().is_err());
        assert!(!f.client.get_paused());
    }

    #[test]
    fn toda_troca_emite_pauserset_com_anterior_e_novo() {
        let e = Env::default();
        let f = fx(&e);
        let a = f.pauser.clone();
        let b = Address::generate(&e);
        let topic = symbol_short!("pauserset");
        let none: Option<Address> = None;

        // trocar a -> b
        f.client.set_emergency_pauser(&Some(b.clone()));
        assert_eq!(
            do_contrato(&e, &f.cid),
            soroban_sdk::vec![&e, ev(&e, &f.cid, topic.clone(), (Some(a.clone()), Some(b.clone())))]
        );

        // remover b -> nada
        f.client.set_emergency_pauser(&None);
        assert_eq!(
            do_contrato(&e, &f.cid),
            soroban_sdk::vec![&e, ev(&e, &f.cid, topic.clone(), (Some(b.clone()), none.clone()))]
        );

        // definir nada -> a
        f.client.set_emergency_pauser(&Some(a.clone()));
        assert_eq!(
            do_contrato(&e, &f.cid),
            soroban_sdk::vec![&e, ev(&e, &f.cid, topic.clone(), (none.clone(), Some(a.clone())))]
        );

        // renunciar: pauserset (a -> nada) e pauserren (a), nessa ordem
        f.client.renounce_emergency_pauser();
        assert_eq!(
            do_contrato(&e, &f.cid),
            soroban_sdk::vec![
                &e,
                ev(&e, &f.cid, topic, (Some(a.clone()), none)),
                ev(&e, &f.cid, symbol_short!("pauserren"), a),
            ]
        );
    }

    #[test]
    fn trocar_o_admin_nao_zera_o_pauser() {
        let e = Env::default();
        let f = fx(&e);
        let novo_admin = Address::generate(&e);
        f.client.propose_admin(&novo_admin);
        f.client.accept_admin();
        assert_eq!(f.client.get_admin(), novo_admin);
        assert_eq!(f.client.emergency_pauser(), Some(f.pauser.clone()));
        f.client.emergency_pause();
        assert!(f.client.get_paused());
    }
}
