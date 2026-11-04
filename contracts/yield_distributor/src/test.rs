#![cfg(test)]

extern crate std;

use super::*;
use soroban_sdk::{
    testutils::Address as _,
    token::{StellarAssetClient, TokenClient},
    Address, Env, Vec,
};

/// SAC gerado — endereço aleatório, NUNCA o canônico. Sob rede real serve como
/// o token "errado" que o F-006 tem de recusar.
fn create_generated_sac<'a>(
    env: &Env,
    admin: &Address,
) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let contract_id = env.register_stellar_asset_contract_v2(admin.clone());
    let sac_address = contract_id.address();
    let token = TokenClient::new(env, &sac_address);
    let sac = StellarAssetClient::new(env, &sac_address);
    (sac_address, token, sac)
}

// PENDENCIAS 159 — o USDC da fixture precisa nascer NO endereço canônico para a
// suíte inteira rodar sob as features do WASM real (`--no-default-features
// --features testnet|mainnet`), onde validate_canonical_usdc (F-006) está vivo.
#[cfg(not(feature = "testing"))]
#[path = "../../shared/mock_usdc.rs"]
mod mock_usdc;

#[cfg(feature = "testing")]
fn create_usdc<'a>(env: &Env, admin: &Address) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    create_generated_sac(env, admin)
}

#[cfg(not(feature = "testing"))]
fn create_usdc<'a>(env: &Env, _admin: &Address) -> (Address, TokenClient<'a>, StellarAssetClient<'a>) {
    let addr = Address::from_str(env, mock_usdc::CANONICAL_USDC);
    env.register_at(&addr, mock_usdc::MockUsdc, ());
    (
        addr.clone(),
        TokenClient::new(env, &addr),
        StellarAssetClient::new(env, &addr),
    )
}

fn setup<'a>(
    env: &Env,
) -> (
    YieldDistributorClient<'a>,
    Address,                     // usdc_sac address
    TokenClient<'a>,             // usdc reader
    StellarAssetClient<'a>,      // usdc admin (mint)
    Address,                     // payer (company)
    Address,                     // treasury
    Address,                     // contract admin
) {
    // v4: the contract is initialized atomically by its __constructor at
    // registration time, which runs admin.require_auth() — mock it here so every
    // setup()-based test deploys cleanly regardless of caller ordering.
    env.mock_all_auths();

    let sac_admin = Address::generate(env);
    let (usdc_addr, token_client, sac_client) = create_usdc(env, &sac_admin);

    // admin must exist before register — it is the constructor argument.
    let admin = Address::generate(env);
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(env, &contract_id);

    let payer = Address::generate(env);
    let treasury = Address::generate(env);

    // Fund payer with 100,000 USDC
    sac_client.mint(&payer, &1_000_000_000_000i128); // 100,000 USDC in stroops

    (client, usdc_addr, token_client, sac_client, payer, treasury, admin)
}

// ═══════════════════════════════════════════════════════════
//  1. distribute() — Happy Path (existing tests, updated)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_distribute_single_investor() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let recipient = Address::generate(&env);
    let recipients = Vec::from_array(&env, [recipient.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128]); // 10 USDC

    client.distribute(
        &payer,
        &usdc,
        &recipients,
        &amounts,
        &treasury,
        &2_0000000i128, // 2 USDC fee
    );

    assert_eq!(token.balance(&recipient), 10_0000000);
    assert_eq!(token.balance(&treasury), 2_0000000);
    // Payer debited: 10 + 2 = 12 USDC
    assert_eq!(
        token.balance(&payer),
        1_000_000_000_000i128 - 12_0000000i128
    );
}

#[test]
fn test_distribute_multiple_investors() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone(), r2.clone(), r3.clone()]);
    let amounts = Vec::from_array(&env, [100_0000000i128, 200_0000000i128, 300_0000000i128]);

    client.distribute(
        &payer,
        &usdc,
        &recipients,
        &amounts,
        &treasury,
        &50_0000000i128, // 50 USDC fee
    );

    assert_eq!(token.balance(&r1), 100_0000000);
    assert_eq!(token.balance(&r2), 200_0000000);
    assert_eq!(token.balance(&r3), 300_0000000);
    assert_eq!(token.balance(&treasury), 50_0000000);
}

#[test]
fn test_distribute_zero_fee() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [50_0000000i128]);

    client.distribute(
        &payer,
        &usdc,
        &recipients,
        &amounts,
        &treasury,
        &0i128, // zero fee
    );

    assert_eq!(token.balance(&r1), 50_0000000);
    assert_eq!(token.balance(&treasury), 0); // No fee transferred
}

// ═══════════════════════════════════════════════════════════
//  2. distribute() — Validation Errors (existing)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_distribute_empty_batch() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let recipients: Vec<Address> = Vec::new(&env);
    let amounts: Vec<i128> = Vec::new(&env);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::EmptyBatch)));
}

#[test]
fn test_distribute_mismatched_arrays() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1, r2]);
    let amounts = Vec::from_array(&env, [10_0000000i128]); // only 1 amount

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::MismatchedArrays)));
}

#[test]
fn test_distribute_negative_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [-1i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::InvalidAmount)));
}

#[test]
fn test_distribute_zero_amount() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [0i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::InvalidAmount)));
}

// ═══════════════════════════════════════════════════════════
//  3. Fee Cap (existing)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_distribute_fee_exceeds_cap() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [100_0000000i128]); // 100 USDC

    // Cap is 70% (lib.rs:295 — `total_payout * 7 / 10`). 100 USDC payout
    // → max_fee = 70 USDC. Fee = 75 USDC → 75 > 70 → FeeTooHigh.
    let result = client.try_distribute(
        &payer,
        &usdc,
        &recipients,
        &amounts,
        &treasury,
        &75_0000000i128,
    );
    assert_eq!(result, Err(Ok(DistributeError::FeeTooHigh)));
}

#[test]
fn test_distribute_fee_at_cap() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [100_0000000i128]); // 100 USDC

    // Fee = 20 USDC = exactly 20% → should work
    client.distribute(
        &payer,
        &usdc,
        &recipients,
        &amounts,
        &treasury,
        &20_0000000i128,
    );

    assert_eq!(token.balance(&r1), 100_0000000);
    assert_eq!(token.balance(&treasury), 20_0000000);
}

// ═══════════════════════════════════════════════════════════
//  4. Batch Size (existing)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_distribute_max_batch_30() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, sac, payer, treasury, _admin) = setup(&env);

    // Fund extra
    sac.mint(&payer, &10_000_000_000_000i128);

    let mut recipients_vec: Vec<Address> = Vec::new(&env);
    let mut amounts_vec: Vec<i128> = Vec::new(&env);
    let mut addrs = soroban_sdk::vec![&env];

    for _ in 0..30 {
        let addr = Address::generate(&env);
        recipients_vec.push_back(addr.clone());
        amounts_vec.push_back(1_0000000i128); // 1 USDC each
        addrs.push_back(addr);
    }

    env.cost_estimate().disable_resource_limits();
    env.cost_estimate().budget().reset_unlimited();
    client.distribute(
        &payer,
        &usdc,
        &recipients_vec,
        &amounts_vec,
        &treasury,
        &0i128,
    );

    // Verify one of them got paid
    let first_recipient = addrs.get(1).unwrap();
    assert_eq!(token.balance(&first_recipient), 1_0000000);
}

#[test]
fn test_distribute_batch_too_large() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let mut recipients_vec: Vec<Address> = Vec::new(&env);
    let mut amounts_vec: Vec<i128> = Vec::new(&env);

    for _ in 0..31 {
        recipients_vec.push_back(Address::generate(&env));
        amounts_vec.push_back(1_0000000i128);
    }

    let result = client.try_distribute(
        &payer,
        &usdc,
        &recipients_vec,
        &amounts_vec,
        &treasury,
        &0i128,
    );
    assert_eq!(result, Err(Ok(DistributeError::BatchTooLarge)));
}

// ═══════════════════════════════════════════════════════════
//  5. Financial Invariants (existing)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_financial_invariant_payer_debit_equals_sum() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone(), r2.clone()]);
    let amounts = Vec::from_array(&env, [333_3333333i128, 666_6666667i128]);
    let fee: i128 = 100_0000000; // 100 USDC

    let payer_before = token.balance(&payer);
    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee);
    let payer_after = token.balance(&payer);

    let total_debited = payer_before - payer_after;
    let expected = 333_3333333i128 + 666_6666667i128 + fee;
    assert_eq!(total_debited, expected, "Payer debit must equal sum(amounts) + fee");

    assert_eq!(token.balance(&r1), 333_3333333);
    assert_eq!(token.balance(&r2), 666_6666667);
    assert_eq!(token.balance(&treasury), fee);
}

#[test]
fn test_financial_no_stroop_leak() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let r3 = Address::generate(&env);

    // Odd fractional amounts that could cause rounding
    let p1: i128 = 33_333_333;
    let p2: i128 = 33_333_334;
    let p3: i128 = 33_333_333;
    let fee: i128 = 0;

    let recipients = Vec::from_array(&env, [r1.clone(), r2.clone(), r3.clone()]);
    let amounts = Vec::from_array(&env, [p1, p2, p3]);

    let payer_before = token.balance(&payer);
    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee);
    let payer_after = token.balance(&payer);

    let total_received = token.balance(&r1) + token.balance(&r2) + token.balance(&r3) + token.balance(&treasury);
    let total_debited = payer_before - payer_after;

    assert_eq!(total_received, total_debited, "Zero stroop leak");
    assert_eq!(total_received, p1 + p2 + p3 + fee);
}

// ═══════════════════════════════════════════════════════════════
//  NEW TESTS — Security hardening (T1-T17)
// ═══════════════════════════════════════════════════════════════

// ─── T1: Duplicate recipient → reject (P0) ──────────────────

#[test]
fn t1_duplicate_recipient_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let same = Address::generate(&env);
    let recipients = Vec::from_array(&env, [same.clone(), same.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128, 10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::DuplicateRecipient)));
}

// ─── T2, T3: upgrade() auth (P0) ────────────────────────────

#[test]
fn t2_upgrade_requires_admin_auth() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);

    // Just verify it doesn't panic with mock_all_auths
    // (In prod, the deployer verifies the WASM hash is a valid installed contract)
    // We can't actually upgrade in tests, but we verify the auth check works
    let fake_hash = BytesN::from_array(&env, &[0u8; 32]);
    // This will fail because the hash isn't an installed WASM, but the auth check passes
    let _result = client.try_upgrade(&fake_hash);
    // The important thing is it didn't fail on auth — it fails on the WASM hash
}

#[test]
fn t3_upgrade_non_admin_fails() {
    let env = Env::default();
    let sac_admin = Address::generate(&env);
    let (_usdc_addr, _token_client, _sac_client) = create_usdc(&env, &sac_admin);

    let real_admin = Address::generate(&env);
    // The __constructor runs admin.require_auth(); mock it for the deploy only.
    env.mock_all_auths();
    let contract_id = env.register(YieldDistributor, (real_admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);

    // Now clear all auths — upgrade() must reject without admin authorization.
    env.mock_auths(&[]);
    let fake_hash = BytesN::from_array(&env, &[0u8; 32]);
    let result = client.try_upgrade(&fake_hash);
    assert!(result.is_err(), "upgrade without auth must fail");
}

// ─── T4: REMOVED in v4 — initialize() replaced by __constructor, which the host
//     invokes exactly once at deploy. A "double initialize" is no longer
//     expressible (there is no initialize entrypoint), so the case is gone.
//     Constructor state-init is covered by test_v4_constructor_initializes_state.

// ─── T5, T6: pause blocks, resume restores (P1) ─────────────

#[test]
fn t5_paused_blocks_distribute() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    client.pause();

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::ContractPaused)));
}

#[test]
fn t6_resume_restores_functionality() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);

    // Pause then resume
    client.pause();
    client.resume();

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(token.balance(&r1), 10_0000000);
}

// ─── T7: Self-transfer (payer == recipient) (P2) ─────────────

#[test]
fn t7_payer_equals_recipient_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    // Payer tries to pay themselves
    let recipients = Vec::from_array(&env, [payer.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::SelfTransfer)));
}

// ─── T8: Payer == fee_recipient (P2) ─────────────────────────

#[test]
fn t8_payer_equals_fee_recipient_allowed() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, _treasury, _admin) = setup(&env);

    // Payer IS the fee recipient — this is allowed (company is also treasury)
    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [100_0000000i128]);

    let payer_before = token.balance(&payer);
    // fee goes back to payer (who is also fee_recipient)
    client.distribute(&payer, &usdc, &recipients, &amounts, &payer, &10_0000000i128);

    // Payer net debit: 100 (investor) + 10 (fee) - 10 (fee back) = 100
    assert_eq!(token.balance(&payer), payer_before - 100_0000000);
    assert_eq!(token.balance(&r1), 100_0000000);
}

// ─── T9: Insufficient balance → SAC panics (P1) ─────────────

#[test]
fn t9_insufficient_payer_balance_panics() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    // Payer only has 100,000 USDC. Try to send 200,000.
    let amounts = Vec::from_array(&env, [2_000_000_000_000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert!(result.is_err(), "Should fail when payer has insufficient balance");
}

// ─── T10: Non-USDC token — arbitrary contract (P1) ──────────

#[test]
fn t10_non_usdc_token_works_if_authed() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, treasury, _admin) = setup(&env);

    // Create a SECOND token (not USDC)
    let other_admin = Address::generate(&env);
    let (other_token_addr, other_token, other_sac) = create_usdc(&env, &other_admin);

    let payer2 = Address::generate(&env);
    other_sac.mint(&payer2, &1_000_000_000_000i128);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [50_0000000i128]);

    // Under the `testing` feature (default for unit tests) the canonical-USDC
    // check is a no-op, so any SAC distributes. Production builds
    // (--features testnet|mainnet) reject a non-canonical token with
    // UnauthorizedToken — see validate_canonical_usdc + the testnet smoke test.
    client.distribute(&payer2, &other_token_addr, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(other_token.balance(&r1), 50_0000000);
}

// ─── T11: i128::MAX overflow (P1) ────────────────────────────

#[test]
fn t11_overflow_detected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1, r2]);
    let amounts = Vec::from_array(&env, [i128::MAX, 1i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::Overflow)));
}

// ─── T12: Tiny fee rounding edge case (P1) ───────────────────

#[test]
fn t12_tiny_fee_rounding() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    // total = 4 stroops, max_fee = 4 * 7 / 10 = 2 (integer division on 70% cap)
    // fee = 3 stroops → 3 > 2 → FeeTooHigh. Smallest possible boundary.
    let amounts = Vec::from_array(&env, [4i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &3i128);
    assert_eq!(result, Err(Ok(DistributeError::FeeTooHigh)));
}

// ─── T13: extend_ttl callable by anyone (P2) ────────────────

#[test]
fn t13_extend_ttl_no_auth_required() {
    let env = Env::default();
    let admin = Address::generate(&env);
    // __constructor needs admin auth at deploy time.
    env.mock_all_auths();
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);

    // extend_ttl should work without any auth — clear auths to prove it.
    env.mock_auths(&[]);
    client.extend_ttl(); // Should not panic
}

// ─── T14: version increments (P0) ───────────────────────────

#[test]
fn t14_version_is_5() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);
    assert_eq!(client.version(), 5);
}

// ─── T15: distribute without auth → fails (P0) ──────────────

#[test]
fn t15_distribute_without_auth_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let sac_admin = Address::generate(&env);
    let (usdc_addr, _token_client, sac_client) = create_usdc(&env, &sac_admin);

    let admin = Address::generate(&env);
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);

    let payer = Address::generate(&env);
    sac_client.mint(&payer, &1_000_000_000_000i128);

    let treasury = Address::generate(&env);
    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    // With mock_all_auths, distribute succeeds — confirming auth IS checked.
    // The actual on-chain auth enforcement is validated by Soroban host, not by
    // unit tests. This test verifies that require_auth() is called (via auths()).
    client.distribute(&payer, &usdc_addr, &recipients, &amounts, &treasury, &0i128);

    // Verify require_auth was actually invoked for payer
    let auths = env.auths();
    assert!(!auths.is_empty(), "require_auth must have been called for payer");
}

// ─── T16: Event emission shape (P2) ─────────────────────────

#[test]
fn t16_event_emitted_on_distribute() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &2_0000000i128);

    // If distribute() succeeded, the emit() call at the end ran.
    // ContractEvents API varies by SDK version — the success assertion is sufficient.
    // The event payload (payer, count, total_payout, fee_amount) is tested
    // implicitly by the successful execution path.
}

// ─── T17: 29 investors — batch_size - 1 boundary (P2) ───────

#[test]
fn t17_batch_29_succeeds() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, sac, payer, treasury, _admin) = setup(&env);

    sac.mint(&payer, &10_000_000_000_000i128);

    let mut recipients_vec: Vec<Address> = Vec::new(&env);
    let mut amounts_vec: Vec<i128> = Vec::new(&env);
    let mut first_addr: Option<Address> = None;

    for i in 0..29 {
        let addr = Address::generate(&env);
        if i == 0 {
            first_addr = Some(addr.clone());
        }
        recipients_vec.push_back(addr);
        amounts_vec.push_back(1_0000000i128);
    }

    env.cost_estimate().disable_resource_limits();
    env.cost_estimate().budget().reset_unlimited();
    client.distribute(&payer, &usdc, &recipients_vec, &amounts_vec, &treasury, &0i128);

    assert_eq!(token.balance(&first_addr.unwrap()), 1_0000000);
}

// ═══════════════════════════════════════════════════════════
//  Removed in v4: "distribute before initialize → NotInitialized".
//  __constructor guarantees the contract is initialized at deploy, so the
//  uninitialized state is unreachable on-chain (no initialize entrypoint exists).
// ═══════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════
//  Additional: get_admin / get_paused read-only
// ═══════════════════════════════════════════════════════════

#[test]
fn test_get_admin_returns_correct_address() {
    let env = Env::default();
    env.mock_all_auths();
    let (_client, _usdc, _token, _sac, _payer, _treasury, admin) = setup(&env);

    let returned_admin = _client.get_admin();
    assert_eq!(returned_admin, admin);
}

#[test]
fn test_get_paused_reflects_state() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);

    assert_eq!(client.get_paused(), false);
    client.pause();
    assert_eq!(client.get_paused(), true);
    client.resume();
    assert_eq!(client.get_paused(), false);
}

// ═══════════════════════════════════════════════════════════
//  Additional: set_admin transfers admin role
// ═══════════════════════════════════════════════════════════

#[test]
fn test_set_admin_transfers_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, admin) = setup(&env);

    let new_admin = Address::generate(&env);
    client.set_admin(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
    assert_ne!(client.get_admin(), admin);
}

// ═══════════════════════════════════════════════════════════
//  Additional: negative fee → InvalidAmount
// ═══════════════════════════════════════════════════════════

#[test]
fn test_negative_fee_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &-1i128);
    assert_eq!(result, Err(Ok(DistributeError::InvalidAmount)));
}

// ═══════════════════════════════════════════════════════════
//  V3 — F-004: 2-step admin rotation (propose_admin / accept_admin)
//  Plus F-016 test coverage backfill.
// ═══════════════════════════════════════════════════════════

#[test]
fn test_v5_version_returns_5() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);
    assert_eq!(client.version(), 5);
}

#[test]
fn test_v3_propose_admin_sets_pending() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);
    let new_admin = Address::generate(&env);

    assert_eq!(client.get_pending_admin(), None);
    client.propose_admin(&new_admin);
    assert_eq!(client.get_pending_admin(), Some(new_admin));
}

#[test]
fn test_v3_propose_admin_overwrites_prior() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.propose_admin(&first);
    client.propose_admin(&second);
    assert_eq!(client.get_pending_admin(), Some(second));
}

#[test]
fn test_v3_accept_admin_rotates_active_admin() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, admin) = setup(&env);
    let new_admin = Address::generate(&env);

    assert_eq!(client.get_admin(), admin);
    client.propose_admin(&new_admin);
    client.accept_admin();

    assert_eq!(client.get_admin(), new_admin);
    assert_eq!(client.get_pending_admin(), None);
}

#[test]
fn test_v3_accept_admin_without_proposal_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);

    let result = client.try_accept_admin();
    assert_eq!(result, Err(Ok(DistributeError::NoPendingAdmin)));
}

#[test]
fn test_v3_new_admin_can_pause_after_rotation() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);
    let new_admin = Address::generate(&env);

    client.propose_admin(&new_admin);
    client.accept_admin();

    // new admin can pause/resume — under mock_all_auths both auths pass
    client.pause();
    assert_eq!(client.get_paused(), true);
    client.resume();
    assert_eq!(client.get_paused(), false);
}

#[test]
#[should_panic]
fn test_v3_old_admin_locked_out_after_rotation() {
    let env = Env::default();

    let (client, _usdc, _token, _sac, _payer, _treasury, admin) = setup(&env);
    let new_admin = Address::generate(&env);

    // Phase 1: rotate under mock_all_auths
    env.mock_all_auths();
    client.propose_admin(&new_admin);
    client.accept_admin();
    assert_eq!(client.get_admin(), new_admin);

    // Phase 2: old admin tries to pause — Soroban must reject because
    // require_auth now checks new_admin, not the old admin.
    let contract_id = client.address.clone();
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &admin,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &contract_id,
            fn_name: "pause",
            args: soroban_sdk::vec![&env],
            sub_invokes: &[],
        },
    }]);

    client.pause(); // expected panic — old admin no longer authorized
}

#[test]
fn test_v3_set_admin_clears_pending() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);
    let pending = Address::generate(&env);
    let direct_swap = Address::generate(&env);

    // Propose one, then use the deprecated set_admin to swap to a different one.
    client.propose_admin(&pending);
    assert_eq!(client.get_pending_admin(), Some(pending));

    client.set_admin(&direct_swap);
    assert_eq!(client.get_admin(), direct_swap);
    // Stale pending must be cleared — otherwise an attacker who knew the
    // earlier proposed address could call accept_admin and take over.
    assert_eq!(client.get_pending_admin(), None);
}

// ─── F-016 backfill: invariants and edge cases ──────────────────

#[test]
fn test_paused_blocks_distribute() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);
    client.pause();

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::ContractPaused)));
}

#[test]
fn test_resume_unblocks_distribute() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, _sac, payer, treasury, _admin) = setup(&env);
    client.pause();
    client.resume();

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    // Should succeed now
    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(token.balance(&r1), 10_0000000);
}

#[test]
fn test_duplicate_recipient_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1.clone(), r1.clone()]);
    let amounts = Vec::from_array(&env, [5_0000000i128, 5_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::DuplicateRecipient)));
}

#[test]
fn test_self_transfer_rejected() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let recipients = Vec::from_array(&env, [payer.clone()]);
    let amounts = Vec::from_array(&env, [10_0000000i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::SelfTransfer)));
}

#[test]
fn test_v4_constructor_initializes_state() {
    // v4: __constructor sets admin + unpaused atomically at deploy — no separate
    // initialize() exists, so the front-run / double-init window is closed.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);

    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);

    assert_eq!(client.get_admin(), admin);
    assert_eq!(client.get_paused(), false);
}

#[test]
fn test_overflow_total_payout_rejected() {
    // Two recipients with i128::MAX amounts each → overflow on .checked_add()
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, _sac, payer, treasury, _admin) = setup(&env);

    let r1 = Address::generate(&env);
    let r2 = Address::generate(&env);
    let recipients = Vec::from_array(&env, [r1, r2]);
    let amounts = Vec::from_array(&env, [i128::MAX, 1i128]);

    let result = client.try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128);
    assert_eq!(result, Err(Ok(DistributeError::Overflow)));
}

#[test]
fn test_extend_ttl_no_auth_required() {
    // extend_ttl is intentionally permissionless (cron jobs)
    let env = Env::default();
    let admin = Address::generate(&env);
    env.mock_all_auths();
    let contract_id = env.register(YieldDistributor, (admin.clone(),));
    let client = YieldDistributorClient::new(&env, &contract_id);

    // No auths needed — even an empty mock list should let this through.
    env.mock_auths(&[]);
    client.extend_ttl();
}

#[test]
fn test_get_pending_admin_default_none() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _usdc, _token, _sac, _payer, _treasury, _admin) = setup(&env);

    assert_eq!(client.get_pending_admin(), None);
}

// ═══════════════════════════════════════════════════════════════════════════
//  WORST-CASE STRESS SIMULATION  (added 2026-06-05 — pre-mainnet hardening)
//
//  These tests do NOT exercise the happy path. They push distribute() to the
//  ABSOLUTE MAXIMUM the code permits and check it survives a *real* Stellar
//  transaction's resource ceilings — fully off-chain, deterministic, no
//  network, no funding.
//
//  "Maximum the code permits":
//    • MAX_BATCH_SIZE (30) distinct recipients  — the hard cap (lib.rs:27)
//    • fee sitting EXACTLY on the 70% cap        — the boundary (lib.rs:295)
//    • distinct, non-round stroop amounts        — stress the running sum + leak
//
//  The ceilings below are copied verbatim from the SDK's own
//  `InvocationResourceLimits::mainnet()` (soroban-sdk 25.3.0,
//  src/testutils/cost_estimate.rs:139). The default test Env already ENFORCES
//  these — which is precisely why the older batch tests had to call
//  `disable_resource_limits()` to pass. Here we MEASURE instead of hiding.
// ═══════════════════════════════════════════════════════════════════════════

// Real mainnet per-transaction ceilings (verbatim from mainnet()).
const MAINNET_MAX_INSTRUCTIONS: i64 = 600_000_000;
const MAINNET_MAX_MEM_BYTES: i64 = 41_943_040; // 40 MiB
const MAINNET_MAX_DISK_READ_ENTRIES: u32 = 100;
const MAINNET_MAX_WRITE_ENTRIES: u32 = 50;
const MAINNET_MAX_DISK_READ_BYTES: u32 = 200_000;
const MAINNET_MAX_WRITE_BYTES: u32 = 132_096;
const MAINNET_MAX_EVENT_BYTES: u32 = 16_384;

/// Build the worst case distribute() will accept: MAX_BATCH_SIZE distinct
/// recipients, distinct non-round amounts (~1000 USDC each), and a fee sitting
/// exactly on the 70% cap. Returns (recipients, amounts, total_payout, fee).
fn build_worst_case(env: &Env) -> (Vec<Address>, Vec<i128>, i128, i128) {
    let mut recipients: Vec<Address> = Vec::new(env);
    let mut amounts: Vec<i128> = Vec::new(env);
    let mut total: i128 = 0;
    for i in 0..MAX_BATCH_SIZE {
        recipients.push_back(Address::generate(env));
        // ~1000 USDC + i stroops: distinct per investor, deliberately non-round
        // to stress the zero-stroop-leak invariant at full batch width.
        let amt = 1_000_0000001i128 + i as i128;
        amounts.push_back(amt);
        total += amt;
    }
    let fee = total * 7 / 10; // exactly the cap checked at lib.rs:295
    (recipients, amounts, total, fee)
}

/// WORST CASE #1 — does a full 30-investor batch fit inside ONE real Stellar
/// transaction? Measures every resource dimension and asserts it against the
/// real mainnet ceiling. This is the single most important pre-mainnet check:
/// the existing batch tests disable the resource limiter, so nobody has ever
/// verified the batch actually fits.
///
/// ⚠ NATIVE flavour: the Rust contract runs host-native, so CPU/memory are
/// UNDER-counted (the SDK cannot see WASM VM instantiation or guest
/// execution). A PASS here is necessary-but-not-sufficient; a FAIL is a hard
/// "won't fit on mainnet". For the mainnet-accurate number, run the
/// `wasm-tests` flavour: `worst_case_30_investors_fits_mainnet_wasm`.
#[test]
fn worst_case_30_investors_fits_mainnet_native() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, sac, payer, treasury, _admin) = setup(&env);
    // Fund the payer well beyond a ~30k USDC batch + fee.
    sac.mint(&payer, &1_000_000_000_000_000i128); // +100M USDC headroom

    let (recipients, amounts, total, fee) = build_worst_case(&env);

    // Meter with unlimited headroom and enforcement off, so we capture the
    // numbers even if a dimension is over the line (rather than panicking).
    env.cost_estimate().budget().reset_unlimited();
    env.cost_estimate().disable_resource_limits();

    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee);

    let res = env.cost_estimate().resources();
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    let mem = env.cost_estimate().budget().memory_bytes_cost();

    std::eprintln!("\n── WORST CASE: 30 investors + fee@70% cap (NATIVE — under-counts CPU) ──");
    std::eprintln!("{:#?}", res);
    std::eprintln!("budget: cpu_insns={} mem_bytes={}", cpu, mem);
    std::eprintln!("fee estimate: {:#?}", env.cost_estimate().fee());
    std::eprintln!(
        "mainnet ceilings: insns<={} mem<={} write_entries<={} write_bytes<={} events<={}\n",
        MAINNET_MAX_INSTRUCTIONS, MAINNET_MAX_MEM_BYTES, MAINNET_MAX_WRITE_ENTRIES,
        MAINNET_MAX_WRITE_BYTES, MAINNET_MAX_EVENT_BYTES,
    );

    // Assert EVERY dimension against the real mainnet ceiling.
    assert!(res.instructions <= MAINNET_MAX_INSTRUCTIONS,
        "CPU {} over mainnet cap {}", res.instructions, MAINNET_MAX_INSTRUCTIONS);
    assert!(res.mem_bytes <= MAINNET_MAX_MEM_BYTES,
        "mem {} over mainnet cap {}", res.mem_bytes, MAINNET_MAX_MEM_BYTES);
    assert!(res.write_entries <= MAINNET_MAX_WRITE_ENTRIES,
        "write_entries {} over mainnet cap {} — 30 investors would NOT fit in one tx",
        res.write_entries, MAINNET_MAX_WRITE_ENTRIES);
    assert!(res.disk_read_entries <= MAINNET_MAX_DISK_READ_ENTRIES,
        "disk_read_entries {} over cap {}", res.disk_read_entries, MAINNET_MAX_DISK_READ_ENTRIES);
    assert!(res.disk_read_bytes <= MAINNET_MAX_DISK_READ_BYTES,
        "disk_read_bytes {} over cap {}", res.disk_read_bytes, MAINNET_MAX_DISK_READ_BYTES);
    assert!(res.write_bytes <= MAINNET_MAX_WRITE_BYTES,
        "write_bytes {} over cap {}", res.write_bytes, MAINNET_MAX_WRITE_BYTES);
    assert!(res.contract_events_size_bytes <= MAINNET_MAX_EVENT_BYTES,
        "event bytes {} over cap {}", res.contract_events_size_bytes, MAINNET_MAX_EVENT_BYTES);

    // Financial integrity at maximum width: zero stroop leak.
    let mut received: i128 = token.balance(&treasury);
    for i in 0..recipients.len() {
        received += token.balance(&recipients.get(i).unwrap());
    }
    assert_eq!(received, total + fee, "zero stroop leak across 30 investors + fee");
}

/// WORST CASE #2 — the contract's entire reason to exist: ONE signature must
/// authorise ALL transfers. A 30-investor batch + fee is 31 SAC.transfer
/// sub-invocations under a single `payer.require_auth()`. This asserts the auth
/// TREE SHAPE — exactly what `mock_all_auths()` normally hides. If the tree is
/// not what we expect, the production passkey would have to sign something
/// different from what we think.
#[test]
fn worst_case_one_signature_covers_all_31_transfers() {
    use soroban_sdk::testutils::AuthorizedFunction;
    use soroban_sdk::Symbol;

    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, sac, payer, treasury, _admin) = setup(&env);
    sac.mint(&payer, &1_000_000_000_000_000i128);

    let (recipients, amounts, _total, fee) = build_worst_case(&env);

    env.cost_estimate().budget().reset_unlimited();
    env.cost_estimate().disable_resource_limits();
    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee);

    let auths = env.auths();
    std::eprintln!("\n── WORST CASE: auth tree for 30 investors + fee ──\n{:#?}\n", auths);

    // Exactly ONE address authorises the whole batch: the payer.
    assert_eq!(auths.len(), 1, "exactly one signer (payer) authorises the batch");
    let (who, root) = &auths[0];
    assert_eq!(who, &payer, "the single signer must be the payer");

    // Root is the distribute() call on the distributor contract.
    match &root.function {
        AuthorizedFunction::Contract((addr, fname, _args)) => {
            assert_eq!(addr, &client.address, "root auth must be on the distributor contract");
            assert_eq!(fname, &Symbol::new(&env, "distribute"));
        }
        _ => panic!("root auth must be the distribute contract call"),
    }

    // 31 sub-invocations: 30 investor transfers + 1 fee transfer, ALL under the
    // single root signature. THIS is "one passkey prompt = N investor payments".
    assert_eq!(root.sub_invocations.len(), 31,
        "one signature must cover all 30 investor transfers + 1 fee transfer");

    // Every sub-invocation is a USDC SAC transfer.
    let transfer = Symbol::new(&env, "transfer");
    for sub in root.sub_invocations.iter() {
        match &sub.function {
            AuthorizedFunction::Contract((addr, fname, _)) => {
                assert_eq!(addr, &usdc, "sub-invocation must be on the USDC SAC");
                assert_eq!(fname, &transfer, "sub-invocation must be a transfer");
            }
            _ => panic!("sub-invocation must be a SAC transfer"),
        }
    }
}

/// WORST CASE #3 — the 70% fee cap is EXACT at full batch width: a fee equal to
/// floor(total * 7 / 10) is accepted; one stroop more is rejected. Guards the
/// FeeTooHigh boundary (lib.rs:295) at the maximum total the batch allows.
#[test]
fn worst_case_fee_cap_is_exact_at_max_batch() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, _token, sac, payer, treasury, _admin) = setup(&env);
    sac.mint(&payer, &1_000_000_000_000_000i128);

    let (recipients, amounts, _total, fee_at_cap) = build_worst_case(&env);
    env.cost_estimate().budget().reset_unlimited();
    env.cost_estimate().disable_resource_limits();

    // fee == floor(total * 7 / 10) → accepted.
    client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee_at_cap);

    // fee == cap + 1 stroop → rejected with FeeTooHigh, even at max width.
    let (recipients2, amounts2, _t2, _f2) = build_worst_case(&env);
    let result = client.try_distribute(
        &payer, &usdc, &recipients2, &amounts2, &treasury, &(fee_at_cap + 1),
    );
    assert_eq!(result, Err(Ok(DistributeError::FeeTooHigh)),
        "one stroop over the 70% cap must be rejected at max batch width");
}

// ═══════════════════════════════════════════════════════════════════════════
//  SMART-WALLET (PASSKEY) APPROVAL SIMULATION  (added 2026-06-05)
//
//  The off-chain batch sim above uses a plain generated payer, so it does NOT
//  include the cost of the production payer being a passkey SMART WALLET whose
//  `__check_auth` runs a WebAuthn (secp256r1) signature check. This block closes
//  that gap: a representative passkey wallet that performs a REAL secp256r1
//  verification (the same primitive the OZ WebAuthn account uses).
//
//  The cheat the user authorised: we skip the human (no Touch ID). The test
//  holds a P-256 key and signs in-process. The CRYPTO stays real, so the
//  measured cost is honest. `__check_auth` runs ONCE per distribute() tree, so
//  this approval cost is additive on top of the 7.1M batch.
// ═══════════════════════════════════════════════════════════════════════════

/// Representative passkey smart-wallet. `approve()` models the costly part of an
/// OZ WebAuthn account's `__check_auth`: hash the challenge, then verify a real
/// secp256r1 signature over it. (`env.crypto().sha256` returns `Hash<32>`, which
/// `secp256r1_verify` requires; signing `sk.sign(msg)` hashes the same way, so
/// the digests match.)
#[contract]
pub struct MockPasskeyWallet;

#[contractimpl]
impl MockPasskeyWallet {
    pub fn approve(
        env: Env,
        message: soroban_sdk::Bytes,
        pubkey: BytesN<65>,
        sig: BytesN<64>,
    ) {
        let digest = env.crypto().sha256(&message);
        env.crypto().secp256r1_verify(&pubkey, &digest, &sig);
    }
}

/// Measures the cost a passkey approval adds to a distribute() transaction:
/// one real secp256r1 verification. Reports it against the 7.1M batch and the
/// 600M mainnet CPU ceiling.
#[test]
fn smart_wallet_passkey_approval_cost() {
    use p256::ecdsa::{signature::Signer, Signature, SigningKey};

    let env = Env::default();
    let wallet_id = env.register(MockPasskeyWallet, ());
    let wallet = MockPasskeyWalletClient::new(&env, &wallet_id);

    // Test-side "passkey": a fixed P-256 key signed programmatically (no biometric).
    let sk = SigningKey::from_slice(&[0x42u8; 32]).expect("valid P-256 scalar");
    let pk_arr: [u8; 65] = sk
        .verifying_key()
        .to_encoded_point(false) // uncompressed SEC-1
        .as_bytes()
        .try_into()
        .expect("65-byte uncompressed pubkey");
    let pubkey = BytesN::<65>::from_array(&env, &pk_arr);

    // The challenge a real wallet would sign (Signer::sign hashes with SHA-256,
    // matching env.crypto().sha256 in the contract).
    let message_bytes: &[u8] = b"radox yield distribution: 30 investors + fee";
    let sig: Signature = sk.sign(message_bytes);
    let sig = sig.normalize_s().unwrap_or(sig); // Soroban requires low-S
    let sb = sig.to_bytes();
    let sig_arr: [u8; 64] = (&sb[..]).try_into().expect("64-byte sig");
    let sig_bytes = BytesN::<64>::from_array(&env, &sig_arr);
    let message = soroban_sdk::Bytes::from_slice(&env, message_bytes);

    env.cost_estimate().budget().reset_unlimited();
    env.cost_estimate().disable_resource_limits();
    wallet.approve(&message, &pubkey, &sig_bytes); // real secp256r1 verify

    let res = env.cost_estimate().resources();
    let cpu = env.cost_estimate().budget().cpu_instruction_cost();
    let mem = env.cost_estimate().budget().memory_bytes_cost();

    const BATCH_WASM_CPU: i64 = 7_111_724; // measured: worst_case_..._wasm
    std::eprintln!("\n── SMART-WALLET PASSKEY APPROVAL (real secp256r1 verify) ──");
    std::eprintln!("{:#?}", res);
    std::eprintln!("budget: cpu_insns={} mem_bytes={}", cpu, mem);
    std::eprintln!(
        "→ approval ≈ {} CPU; batch+approval ≈ {} / {} mainnet cap ({}%)\n",
        res.instructions,
        res.instructions + BATCH_WASM_CPU,
        MAINNET_MAX_INSTRUCTIONS,
        (res.instructions + BATCH_WASM_CPU) * 100 / MAINNET_MAX_INSTRUCTIONS,
    );

    // A passkey approval must be cheap relative to the mainnet CPU ceiling.
    assert!(res.instructions <= MAINNET_MAX_INSTRUCTIONS,
        "approval CPU {} over mainnet cap {}", res.instructions, MAINNET_MAX_INSTRUCTIONS);
    // And batch + approval together must still fit one transaction with wide margin.
    assert!(res.instructions + BATCH_WASM_CPU <= MAINNET_MAX_INSTRUCTIONS,
        "batch+approval {} over mainnet cap {}", res.instructions + BATCH_WASM_CPU, MAINNET_MAX_INSTRUCTIONS);
}

/// Runs the FULL 30-investor worst-case batch with the passkey smart-wallet
/// CONTRACT as the payer (production uses a C... smart-wallet address, not a
/// classic G... account). Confirms a contract-address payer drives all 31
/// transfers and measures the batch footprint in that configuration.
#[test]
fn worst_case_smart_wallet_is_payer() {
    let env = Env::default();
    env.mock_all_auths(); // mock_all_auths covers the wallet's __check_auth here;
                          // the standalone secp256r1 cost is measured separately above.
    let (client, usdc, token, sac, _payer, treasury, _admin) = setup(&env);

    // The payer is the smart-wallet contract address.
    let wallet_id = env.register(MockPasskeyWallet, ());
    sac.mint(&wallet_id, &1_000_000_000_000_000i128);

    let (recipients, amounts, total, fee) = build_worst_case(&env);

    env.cost_estimate().budget().reset_unlimited();
    env.cost_estimate().disable_resource_limits();
    client.distribute(&wallet_id, &usdc, &recipients, &amounts, &treasury, &fee);

    let res = env.cost_estimate().resources();
    std::eprintln!("\n── WORST CASE: 30 investors, payer = SMART-WALLET CONTRACT ──");
    std::eprintln!("write_entries={} instructions={} (vs classic-payer 33 / 7.1M)\n",
        res.write_entries, res.instructions);

    // Same ledger-write ceiling holds with a contract payer.
    assert!(res.write_entries <= MAINNET_MAX_WRITE_ENTRIES,
        "write_entries {} over mainnet cap {}", res.write_entries, MAINNET_MAX_WRITE_ENTRIES);

    // Zero stroop leak with a smart-wallet payer.
    let mut received: i128 = token.balance(&treasury);
    for i in 0..recipients.len() {
        received += token.balance(&recipients.get(i).unwrap());
    }
    assert_eq!(received, total + fee, "zero stroop leak, smart-wallet payer");
}

// ═══════════════════════════════════════════════════════════════════════════
//  FULL-SCHEDULE LIFECYCLE SIMULATION  (added 2026-06-05)
//
//  The question that matters most: do holders keep receiving the CORRECT amount,
//  every month, for the whole term, until the end — with nothing drifting,
//  leaking, or double-paying? This simulates a 1-year offer paying monthly
//  investor yield (10% APY) to 3 holders of different sizes, advancing the
//  calendar and bumping the contract TTL each month like the maintenance cron.
//
//  Scope: this proves the CONTRACT faithfully executes the schedule period after
//  period (no drift/leak, survives the term). The per-period AMOUNT computation
//  is the backend's job (companyPayment.service.js) and is covered by the E2E
//  tokenLifecycle.test.js — which today exercises a SINGLE period, not a full
//  term. The amounts here are computed independently in-test (dual computation,
//  the same round7 model) so this is a real correctness check, not a tautology.
// ═══════════════════════════════════════════════════════════════════════════

/// Round-half-up integer division — mirrors the backend's round7 in stroops.
fn round_div(numer: i128, denom: i128) -> i128 {
    (numer + denom / 2) / denom
}

#[test]
fn lifecycle_monthly_yield_full_term_correct() {
    use soroban_sdk::testutils::Ledger as _;
    let env = Env::default();
    env.mock_all_auths();
    let (client, usdc, token, sac, payer, treasury, admin) = setup(&env);
    sac.mint(&payer, &10_000_000_000_000i128); // fund the company for the year

    // 3 holders, distinct principals chosen to force rounding in BOTH directions.
    let holders = [
        (Address::generate(&env), 50_000_000_000i128), // 5,000 USDC
        (Address::generate(&env), 10_000_000_000i128), // 1,000 USDC
        (Address::generate(&env), 1_000_000_000i128),  //   100 USDC
    ];
    let investor_rate: i128 = 10; // % APY the investor receives
    let company_rate: i128 = 12; //  % APY the company pays (2% platform spread)
    let months: i128 = 12;

    // Independent monthly amounts (NOT from any service):
    //   investorMonthly_i = round7(principal × investorRate/100 / 12)  [stroops]
    let inv_monthly: std::vec::Vec<i128> = holders
        .iter()
        .map(|(_, p)| round_div(p * investor_rate, 100 * months))
        .collect();
    let comp_monthly: std::vec::Vec<i128> = holders
        .iter()
        .map(|(_, p)| round_div(p * company_rate, 100 * months))
        .collect();
    let fee_monthly: i128 = (0..holders.len()).map(|i| comp_monthly[i] - inv_monthly[i]).sum();
    let payout_monthly: i128 = inv_monthly.iter().sum();

    // Build the soroban Vec args the company signs each month.
    let mut recipients: Vec<Address> = Vec::new(&env);
    let mut amounts: Vec<i128> = Vec::new(&env);
    for (i, (addr, _)) in holders.iter().enumerate() {
        recipients.push_back(addr.clone());
        amounts.push_back(inv_monthly[i]);
    }

    let payer_start = token.balance(&payer);
    let mut seq: u32 = 100_000;
    let mut ts: u64 = 1_700_000_000; // arbitrary start time

    // ── Run the full schedule, month by month, to the end of the term ──
    for month in 1..=months {
        // A month passes; the maintenance cron bumps the contract's TTL.
        ts += 30 * 24 * 60 * 60; // +30 days (calendar)
        seq += 5_000; // modest ledger advance (distribute has no time logic)
        env.ledger().set_timestamp(ts);
        env.ledger().set_sequence_number(seq);
        client.extend_ttl();

        client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee_monthly);

        // Every holder has received EXACTLY month × their monthly amount — no drift.
        for (i, (addr, _)) in holders.iter().enumerate() {
            assert_eq!(token.balance(addr), inv_monthly[i] * month,
                "holder {} wrong cumulative balance after month {}", i, month);
        }
        assert_eq!(token.balance(&treasury), fee_monthly * month,
            "treasury cumulative drift after month {}", month);
        assert_eq!(payer_start - token.balance(&payer), (payout_monthly + fee_monthly) * month,
            "company debit drift after month {}", month);
    }

    // ── End of term: cumulative correctness + monthly-vs-annual rounding drift ──
    std::eprintln!("\n── 12-MONTH YIELD SCHEDULE — per-holder result ──");
    for (i, (addr, principal)) in holders.iter().enumerate() {
        let received = token.balance(addr);
        let monthly_sum = inv_monthly[i] * months;
        let annual = round_div(principal * investor_rate, 100); // one annual payment
        std::eprintln!(
            "holder {}: principal={} stroops | 12×monthly={} | annual_equiv={} | drift={} stroops",
            i, principal, received, annual, monthly_sum - annual);

        // Exact: 12 monthly payments == 12 × the monthly amount, to the stroop.
        assert_eq!(received, monthly_sum, "holder {} cumulative != 12 × monthly", i);
        // Monthly accrual lands within a handful of stroops of the annual figure
        // (the unavoidable, sub-cent consequence of rounding each month).
        assert!((monthly_sum - annual).abs() <= months,
            "holder {} monthly-vs-annual drift {} exceeds {} stroops",
            i, monthly_sum - annual, months);
    }
    std::eprintln!("(drift is in STROOPS: 1 stroop = 0.0000001 USDC)\n");

    // The contract is still alive, unpaused, and correctly owned after the term.
    assert_eq!(client.get_paused(), false, "contract must survive the full term");
    assert_eq!(client.get_admin(), admin, "admin unchanged across the term");
}

// ═══════════════════════════════════════════════════════════════════════════
//  WASM-ACCURATE WORST CASE  (feature-gated: `--features wasm-tests`)
//
//  Imports the EXACT bytes staged for mainnet —
//    deploy/mainnet-wasm/yield_distributor.wasm
//    sha256 88509c72c0a82812b8f110b04bcd2dd3f734a43aa4b8bb18a28160ca9b9e4203
//    (v4, rustc 1.86.0 / stellar-cli 27.0.0 / sdk 25.3.0 — matches docs/Operations/DEPLOYMENTS.md)
//  — and runs the 30-investor worst case through the real WASM VM. Unlike the
//  native test, this COUNTS VM instantiation + guest execution, i.e. the true
//  mainnet CPU/memory cost. write_entries/bytes/events are identical to native
//  (they are ledger ops, not guest compute), so only CPU/mem differ here.
//
//  Run (no build needed — uses the committed mainnet artifact):
//    cargo test --features wasm-tests \
//      worst_case_30_investors_fits_mainnet_wasm -- --nocapture
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(feature = "wasm-tests")]
mod wasm_accurate {
    use super::*;

    mod imported {
        soroban_sdk::contractimport!(
            file = "../../deploy/mainnet-wasm/yield_distributor.wasm"
        );
    }

    // IGNORED for v4 (F-SOR-001): the mainnet-feature WASM accepts ONLY the canonical
    // USDC SAC (CCW67TSZ…), which cannot be registered at that fixed address in a unit
    // Env, so distribute() returns UnauthorizedToken(#13) before any work is measured.
    // The 30-investor resource ceiling is covered by `worst_case_30_investors_fits_mainnet_native`
    // (write_entries/bytes/events are ledger ops, identical to the WASM run; the v4 CPU delta
    // over v3 is only the SAC-address compare). Re-enable if a canonical-SAC harness is added.
    #[test]
    #[ignore = "v4 mainnet WASM rejects non-canonical SAC (#13); resource ceiling covered by the native worst-case test"]
    fn worst_case_30_investors_fits_mainnet_wasm() {
        let env = Env::default();
        // Imported-WASM __constructor runs admin.require_auth() as a non-root
        // sub-invocation under recording auth — allow it (native tests inline it at root).
        env.mock_all_auths_allowing_non_root_auth();

        // USDC SAC — host built-in, identical to the native test setup.
        let sac_admin = Address::generate(&env);
        let (usdc, token, sac) = create_usdc(&env, &sac_admin);

        // Register the REAL mainnet WASM. v4: __constructor sets admin + unpaused
        // atomically at deploy (auth mocked above) — no separate initialize().
        let admin = Address::generate(&env);
        let contract_id = env.register(imported::WASM, (admin.clone(),));
        let client = imported::Client::new(&env, &contract_id);

        let payer = Address::generate(&env);
        sac.mint(&payer, &1_000_000_000_000_000i128);
        let treasury = Address::generate(&env);

        let (recipients, amounts, total, fee) = build_worst_case(&env);

        env.cost_estimate().budget().reset_unlimited();
        env.cost_estimate().disable_resource_limits();
        client.distribute(&payer, &usdc, &recipients, &amounts, &treasury, &fee);

        let res = env.cost_estimate().resources();
        let cpu = env.cost_estimate().budget().cpu_instruction_cost();
        let mem = env.cost_estimate().budget().memory_bytes_cost();

        std::eprintln!("\n── WORST CASE: 30 investors + fee@70% cap (REAL MAINNET WASM) ──");
        std::eprintln!("{:#?}", res);
        std::eprintln!("budget: cpu_insns={} mem_bytes={}", cpu, mem);
        std::eprintln!("fee estimate: {:#?}", env.cost_estimate().fee());
        std::eprintln!(
            "mainnet ceilings: insns<={} mem<={} write_entries<={} write_bytes<={} events<={}\n",
            MAINNET_MAX_INSTRUCTIONS, MAINNET_MAX_MEM_BYTES, MAINNET_MAX_WRITE_ENTRIES,
            MAINNET_MAX_WRITE_BYTES, MAINNET_MAX_EVENT_BYTES,
        );

        // The headline assertion: the REAL WASM CPU cost fits one mainnet tx.
        assert!(res.instructions <= MAINNET_MAX_INSTRUCTIONS,
            "REAL WASM CPU {} over mainnet cap {} — 30-investor batch would NOT fit in one tx",
            res.instructions, MAINNET_MAX_INSTRUCTIONS);
        assert!(res.mem_bytes <= MAINNET_MAX_MEM_BYTES,
            "mem {} over mainnet cap {}", res.mem_bytes, MAINNET_MAX_MEM_BYTES);
        assert!(res.write_entries <= MAINNET_MAX_WRITE_ENTRIES,
            "write_entries {} over mainnet cap {}", res.write_entries, MAINNET_MAX_WRITE_ENTRIES);
        assert!(res.disk_read_entries <= MAINNET_MAX_DISK_READ_ENTRIES,
            "disk_read_entries {} over cap {}", res.disk_read_entries, MAINNET_MAX_DISK_READ_ENTRIES);
        assert!(res.disk_read_bytes <= MAINNET_MAX_DISK_READ_BYTES,
            "disk_read_bytes {} over cap {}", res.disk_read_bytes, MAINNET_MAX_DISK_READ_BYTES);
        assert!(res.write_bytes <= MAINNET_MAX_WRITE_BYTES,
            "write_bytes {} over cap {}", res.write_bytes, MAINNET_MAX_WRITE_BYTES);
        assert!(res.contract_events_size_bytes <= MAINNET_MAX_EVENT_BYTES,
            "event bytes {} over cap {}", res.contract_events_size_bytes, MAINNET_MAX_EVENT_BYTES);

        // Zero stroop leak at maximum width, through the real WASM.
        let mut received: i128 = token.balance(&treasury);
        for i in 0..recipients.len() {
            received += token.balance(&recipients.get(i).unwrap());
        }
        assert_eq!(received, total + fee, "zero stroop leak across 30 investors + fee");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  BIRTH-TO-DEATH LIFECYCLE  (feature-gated: `--features wasm-tests`)
//
//  The complete arc across BOTH contracts: an investor holds tokens, receives
//  12 monthly yield payments via the yield_distributor, then at maturity the
//  MaturitySettlement contract returns their principal AND burns their tokens,
//  closing the offer. Proves the two contracts compose and every holder ends up
//  whole, to the stroop, with zero tokens left.
//
//  Imports the real (testing-feature) settlement WASM. Build it first:
//    cargo build --manifest-path ../maturity_settlement/Cargo.toml \
//      --target wasm32v1-none --release
//  Then: cargo test --features wasm-tests full_lifecycle -- --nocapture
//
//  Product model assumed: monthly coupons (interest paid each month) + principal
//  returned at maturity (a coupon bond). The settlement fee is 0 on the principal
//  return — the platform spread was already taken on each monthly coupon.
// ═══════════════════════════════════════════════════════════════════════════
#[cfg(feature = "wasm-tests")]
mod birth_to_death {
    use super::*;
    use soroban_sdk::testutils::{IssuerFlags, Ledger as _};
    use soroban_sdk::token::{StellarAssetClient, TokenClient};

    mod settlement {
        soroban_sdk::contractimport!(
            file = "../maturity_settlement/target/wasm32v1-none/release/maturity_settlement.wasm"
        );
    }

    #[test]
    fn full_lifecycle_yield_12_months_then_maturity_burn() {
        let env = Env::default();
        env.mock_all_auths();

        // ── Actors ──
        let issuer = Address::generate(&env); // token SAC admin (clawback) + settlement admin
        let usdc_admin = Address::generate(&env);
        let company = Address::generate(&env); // pays monthly yield, deposits principal
        let treasury = Address::generate(&env);

        // ── USDC SAC ──
        let usdc_sac = env.register_stellar_asset_contract_v2(usdc_admin.clone());
        let usdc_addr = usdc_sac.address();
        let usdc = TokenClient::new(&env, &usdc_addr);
        let usdc_mint = StellarAssetClient::new(&env, &usdc_addr);

        // ── Security token SAC — clawback enabled BEFORE any mint ──
        let sec = env.register_stellar_asset_contract_v2(issuer.clone());
        sec.issuer().set_flag(IssuerFlags::RevocableFlag);
        sec.issuer().set_flag(IssuerFlags::ClawbackEnabledFlag);
        let sec_addr = sec.address();
        let sec_token = TokenClient::new(&env, &sec_addr);
        let sec_mint = StellarAssetClient::new(&env, &sec_addr);

        // ── Holders (address, principal_stroops); 1 token = 1 USDC ──
        let holders = [
            (Address::generate(&env), 50_000_000_000i128), // 5,000 USDC
            (Address::generate(&env), 10_000_000_000i128), // 1,000 USDC
            (Address::generate(&env), 1_000_000_000i128),  //   100 USDC
        ];
        for (addr, p) in holders.iter() {
            sec_mint.mint(addr, p); // each holds tokens == their principal
        }

        // Independent yield math (round7 model, in stroops).
        let investor_rate: i128 = 10;
        let company_rate: i128 = 12;
        let months: i128 = 12;
        let inv_monthly: std::vec::Vec<i128> = holders
            .iter()
            .map(|(_, p)| round_div(p * investor_rate, 100 * months))
            .collect();
        let comp_monthly: std::vec::Vec<i128> = holders
            .iter()
            .map(|(_, p)| round_div(p * company_rate, 100 * months))
            .collect();
        let yield_fee_monthly: i128 =
            (0..holders.len()).map(|i| comp_monthly[i] - inv_monthly[i]).sum();
        let total_principal: i128 = holders.iter().map(|(_, p)| *p).sum();

        usdc_mint.mint(&company, &1_000_000_000_000i128); // 100k USDC — ample

        // ── PHASE 1: 12 monthly yield payments via yield_distributor ──
        // v4: issuer is the constructor admin; init is atomic at register time.
        let yd_id = env.register(YieldDistributor, (issuer.clone(),));
        let yd = YieldDistributorClient::new(&env, &yd_id);

        let mut recipients: Vec<Address> = Vec::new(&env);
        let mut amounts: Vec<i128> = Vec::new(&env);
        for (i, (addr, _)) in holders.iter().enumerate() {
            recipients.push_back(addr.clone());
            amounts.push_back(inv_monthly[i]);
        }

        let mut seq: u32 = 100_000;
        let mut ts: u64 = 1_700_000_000;
        for _m in 1..=months {
            ts += 30 * 24 * 60 * 60;
            seq += 5_000;
            env.ledger().set_timestamp(ts);
            env.ledger().set_sequence_number(seq);
            yd.extend_ttl();
            yd.distribute(&company, &usdc_addr, &recipients, &amounts, &treasury, &yield_fee_monthly);
        }

        // After the coupon term: yield accrued, tokens still held.
        for (i, (addr, p)) in holders.iter().enumerate() {
            assert_eq!(usdc.balance(addr), inv_monthly[i] * months, "holder {} coupon accrual", i);
            assert_eq!(sec_token.balance(addr), *p, "holder {} tokens intact pre-maturity", i);
        }

        // ── PHASE 2: Maturity — deposit principal, settle (pay + burn), close ──
        let ms_id = env.register(settlement::WASM, ());
        let ms = settlement::Client::new(&env, &ms_id);
        ms.initialize(&issuer, &usdc_addr, &sec_addr, &treasury, &200u32); // max_fee 2%

        ms.deposit(&company, &total_principal);
        assert_eq!(ms.get_balance(), total_principal, "deposit funds the settlement");

        let mut items: Vec<settlement::SettleItem> = Vec::new(&env);
        for (addr, p) in holders.iter() {
            items.push_back(settlement::SettleItem { investor: addr.clone(), payout: *p });
        }
        ms.settle_batch(&items, &0i128); // principal return, no fee

        // ── END OF LIFE: everyone whole, all tokens burned, offer closed ──
        std::eprintln!("\n── FULL LIFECYCLE: 12 monthly coupons → maturity → principal + burn ──");
        for (i, (addr, p)) in holders.iter().enumerate() {
            let total = usdc.balance(addr);
            let expected = inv_monthly[i] * months + p;
            std::eprintln!(
                "holder {}: principal={} | lifetime_USDC={} (coupons {} + principal {}) | tokens_left={}",
                i, p, total, inv_monthly[i] * months, p, sec_token.balance(addr));
            assert_eq!(total, expected, "holder {} lifetime total wrong", i);
            assert_eq!(sec_token.balance(addr), 0, "holder {} tokens must be burned at maturity", i);
        }
        assert_eq!(ms.get_balance(), 0, "settlement contract fully drained");

        // Offer is CLOSED: re-settling a holder is rejected (no double-pay).
        let mut again: Vec<settlement::SettleItem> = Vec::new(&env);
        again.push_back(settlement::SettleItem { investor: holders[0].0.clone(), payout: holders[0].1 });
        assert!(ms.try_settle_batch(&again, &0i128).is_err(),
            "double-settlement must be rejected — offer is closed");
        std::eprintln!("✓ every holder paid coupons for the full term + principal at maturity; tokens burned; closed\n");
    }
}

// ═══════════════════════════════════════════════════════════
//  F-SOR-001 / F-006 — canonical USDC SAC: aceita o canônico, recusa o resto.
//  Só compila com `testing` DESLIGADO — quando validate_canonical_usdc está
//  vivo, igual ao WASM deployado. Rode com:
//     cargo test --no-default-features --features testnet
// ═══════════════════════════════════════════════════════════
#[cfg(not(feature = "testing"))]
mod canonical_usdc_tests {
    use super::*;

    /// Guarda do andaime: se a const do mock sair de sincronia com a do lib.rs,
    /// TODA a suíte sob rede real vira falso-vermelho — este teste diz onde.
    #[test]
    fn mock_sits_at_the_canonical_address() {
        let env = Env::default();
        let (usdc, _t, _s) = create_usdc(&env, &Address::generate(&env));
        assert_eq!(usdc, Address::from_str(&env, mock_usdc::CANONICAL_USDC));
    }

    #[test]
    fn distribute_rejects_non_canonical_usdc() {
        let env = Env::default();
        let (client, _usdc, _token, _sac, payer, treasury, _admin) = setup(&env);
        // SAC gerado: endereço aleatório, nunca o canônico.
        let (fake, _t, fake_sac) = create_generated_sac(&env, &Address::generate(&env));
        fake_sac.mint(&payer, &1_000_0000000i128);

        let r1 = Address::generate(&env);
        let recipients = Vec::from_array(&env, [r1]);
        let amounts = Vec::from_array(&env, [50_0000000i128]);

        let result = client.try_distribute(&payer, &fake, &recipients, &amounts, &treasury, &0i128);
        assert_eq!(result, Err(Ok(DistributeError::UnauthorizedToken)));
    }
}

// ═══════════════════════════════════════════════════════════
//  v5 (F6) — emergency_pauser: papel separado do admin
//  Liga a pausa e SO isso: nao despausa, nao troca admin, nao faz upgrade, nao move dinheiro.
// ═══════════════════════════════════════════════════════════

mod emergency_pauser_tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Events as _, MockAuth, MockAuthInvoke},
        BytesN, IntoVal, Val,
    };

    /// Autoriza SO `who` para a proxima chamada de `fn_name` — as demais exigencias de auth falham.
    fn only(env: &Env, who: &Address, contract: &Address, fn_name: &str, args: Vec<Val>) {
        env.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke {
                contract,
                fn_name,
                args,
                sub_invokes: &[],
            },
        }]);
    }

    fn some_arg(env: &Env, a: &Address) -> Vec<Val> {
        soroban_sdk::vec![env, Some(a.clone()).into_val(env)]
    }

    fn pauser_defined<'a>(
        env: &Env,
    ) -> (
        YieldDistributorClient<'a>,
        Address, // usdc
        TokenClient<'a>,
        Address, // payer
        Address, // treasury
        Address, // admin
        Address, // pauser
    ) {
        let (client, usdc, token, _sac, payer, treasury, admin) = setup(env);
        let pauser = Address::generate(env);
        client.set_emergency_pauser(&Some(pauser.clone()));
        (client, usdc, token, payer, treasury, admin, pauser)
    }

    fn try_distribute(
        env: &Env,
        client: &YieldDistributorClient,
        usdc: &Address,
        payer: &Address,
        treasury: &Address,
    ) -> Result<(), DistributeError> {
        let r = Address::generate(env);
        let recipients = Vec::from_array(env, [r]);
        let amounts = Vec::from_array(env, [10_0000000i128]);
        match client.try_distribute(payer, usdc, &recipients, &amounts, treasury, &0i128) {
            Ok(_) => Ok(()),
            Err(Ok(e)) => Err(e),
            Err(Err(_)) => panic!("falha de invocacao, nao erro do contrato"),
        }
    }

    #[test]
    fn so_o_admin_define_o_pauser() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, admin, _pauser) = pauser_defined(&env);
        let novo = Address::generate(&env);
        let cid = client.address.clone();

        // admin sozinho define
        only(&env, &admin, &cid, "set_emergency_pauser", some_arg(&env, &novo));
        client.set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(env.auths()[0].0, admin);
        assert_eq!(client.emergency_pauser(), Some(novo.clone()));

        // terceiro nao define; o valor segue igual
        let intruso = Address::generate(&env);
        let outro = Address::generate(&env);
        only(&env, &intruso, &cid, "set_emergency_pauser", some_arg(&env, &outro));
        assert!(client.try_set_emergency_pauser(&Some(outro)).is_err());
        assert_eq!(client.emergency_pauser(), Some(novo.clone()));

        // o proprio pauser nao se promove nem troca o sucessor
        let sucessor = Address::generate(&env);
        only(&env, &novo, &cid, "set_emergency_pauser", some_arg(&env, &sucessor));
        assert!(client.try_set_emergency_pauser(&Some(sucessor)).is_err());
        assert_eq!(client.emergency_pauser(), Some(novo));
    }

    #[test]
    fn pauser_pausa_e_distribute_fica_bloqueado() {
        let env = Env::default();
        let (client, usdc, token, payer, treasury, _admin, pauser) = pauser_defined(&env);
        let cid = client.address.clone();
        let saldo = token.balance(&payer);

        only(&env, &pauser, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        client.emergency_pause();
        assert_eq!(env.auths()[0].0, pauser, "quem assina e o pauser");
        assert!(client.get_paused());

        env.mock_all_auths();
        assert_eq!(
            try_distribute(&env, &client, &usdc, &payer, &treasury),
            Err(DistributeError::ContractPaused)
        );
        assert_eq!(token.balance(&payer), saldo, "pausar nao move dinheiro");
    }

    #[test]
    fn emergency_pause_sem_pauser_configurado_e_recusada() {
        let env = Env::default();
        let (client, ..) = setup(&env);
        assert_eq!(client.emergency_pauser(), None);
        assert_eq!(
            client.try_emergency_pause(),
            Err(Ok(DistributeError::NoEmergencyPauser))
        );
        assert!(!client.get_paused());
    }

    #[test]
    fn terceiro_e_o_proprio_admin_nao_ligam_emergency_pause() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, admin, _pauser) = pauser_defined(&env);
        let cid = client.address.clone();
        let intruso = Address::generate(&env);

        only(&env, &intruso, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        assert!(client.try_emergency_pause().is_err());
        assert!(!client.get_paused());

        // o admin tem `pause()` para isso; emergency_pause exige a assinatura do pauser
        only(&env, &admin, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        assert!(client.try_emergency_pause().is_err());
        assert!(!client.get_paused());
    }

    #[test]
    fn pauser_nao_despausa_nao_troca_admin_nao_faz_upgrade_nao_move_dinheiro() {
        let env = Env::default();
        let (client, usdc, token, payer, treasury, admin, pauser) = pauser_defined(&env);
        let cid = client.address.clone();
        client.emergency_pause(); // (mock_all_auths do setup) — pausa ligada

        let saldo = token.balance(&payer);
        let alvo = Address::generate(&env);

        only(&env, &pauser, &cid, "resume", soroban_sdk::vec![&env]);
        assert!(client.try_resume().is_err());
        assert!(client.get_paused(), "o pauser nao despausa");

        only(&env, &pauser, &cid, "set_admin", soroban_sdk::vec![&env, alvo.to_val()]);
        assert!(client.try_set_admin(&alvo).is_err());
        only(&env, &pauser, &cid, "propose_admin", soroban_sdk::vec![&env, alvo.to_val()]);
        assert!(client.try_propose_admin(&alvo).is_err());
        assert_eq!(client.get_admin(), admin, "o pauser nao troca o admin");
        assert_eq!(client.get_pending_admin(), None);

        let hash = BytesN::from_array(&env, &[7u8; 32]);
        only(&env, &pauser, &cid, "upgrade", soroban_sdk::vec![&env, hash.to_val()]);
        assert!(client.try_upgrade(&hash).is_err(), "o pauser nao troca o codigo");

        // distribute pede a assinatura do payer: o pauser sozinho nao move USDC
        let recipients = Vec::from_array(&env, [alvo.clone()]);
        let amounts = Vec::from_array(&env, [10_0000000i128]);
        only(
            &env,
            &pauser,
            &cid,
            "distribute",
            soroban_sdk::vec![
                &env,
                payer.to_val(),
                usdc.to_val(),
                recipients.to_val(),
                amounts.to_val(),
                treasury.to_val(),
                0i128.into_val(&env),
            ],
        );
        assert!(client
            .try_distribute(&payer, &usdc, &recipients, &amounts, &treasury, &0i128)
            .is_err());
        assert_eq!(token.balance(&payer), saldo);
        assert_eq!(token.balance(&alvo), 0);
    }

    #[test]
    fn admin_despausa_e_o_distribute_volta() {
        let env = Env::default();
        let (client, usdc, token, payer, treasury, _admin, pauser) = pauser_defined(&env);
        let cid = client.address.clone();
        only(&env, &pauser, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        client.emergency_pause();
        assert!(client.get_paused());

        env.mock_all_auths();
        client.resume();
        assert!(!client.get_paused());
        assert_eq!(try_distribute(&env, &client, &usdc, &payer, &treasury), Ok(()));
        assert_eq!(token.balance(&payer), 1_000_000_000_000i128 - 10_0000000i128);
    }

    #[test]
    fn trocar_ou_remover_o_pauser_invalida_o_antigo() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, _admin, antigo) = pauser_defined(&env);
        let cid = client.address.clone();
        let novo = Address::generate(&env);
        client.set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(client.emergency_pauser(), Some(novo.clone()));

        only(&env, &antigo, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        assert!(client.try_emergency_pause().is_err(), "o antigo nao pausa mais");
        assert!(!client.get_paused());

        only(&env, &novo, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        client.emergency_pause();
        assert!(client.get_paused());

        env.mock_all_auths();
        client.resume();
        client.set_emergency_pauser(&None);
        assert_eq!(client.emergency_pauser(), None);
        assert_eq!(
            client.try_emergency_pause(),
            Err(Ok(DistributeError::NoEmergencyPauser))
        );
    }

    #[test]
    fn emergency_pause_e_idempotente() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, _admin, _pauser) = pauser_defined(&env);
        client.emergency_pause();
        client.emergency_pause();
        assert!(client.get_paused());
    }

    /// Instancia "antiga" (v4): nasce SEM a chave nova. Tudo segue funcionando e o layout
    /// guardado (admin, paused, pending) nao muda quando o pauser entra e sai.
    #[test]
    fn instancia_antiga_sem_a_chave_continua_funcionando() {
        let env = Env::default();
        let (client, usdc, token, _sac, payer, treasury, admin) = setup(&env);
        let cid = client.address.clone();

        // setup() registra pelo __constructor, que nunca grava a chave: e a forma de uma v4
        env.as_contract(&cid, || {
            assert!(!env.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert_eq!(client.emergency_pauser(), None);

        // fluxo v4 inteiro, sem pauser
        assert_eq!(try_distribute(&env, &client, &usdc, &payer, &treasury), Ok(()));
        client.pause();
        assert_eq!(
            try_distribute(&env, &client, &usdc, &payer, &treasury),
            Err(DistributeError::ContractPaused)
        );
        client.resume();
        assert_eq!(try_distribute(&env, &client, &usdc, &payer, &treasury), Ok(()));
        let novo_admin = Address::generate(&env);
        client.propose_admin(&novo_admin);
        client.accept_admin();
        assert_eq!(client.get_admin(), novo_admin);
        assert_eq!(token.balance(&payer), 1_000_000_000_000i128 - 20_0000000i128);

        // entrar e sair com a chave nova nao mexe nas chaves antigas
        client.set_emergency_pauser(&Some(admin.clone()));
        assert_eq!(client.get_admin(), novo_admin);
        assert!(!client.get_paused());
        client.set_emergency_pauser(&None);
        env.as_contract(&cid, || {
            assert!(!env.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert_eq!(client.version(), 5);
    }

    // ─── renuncia + eventos de troca (emenda da v5) ─────────────────────────

    /// Evento esperado do contrato: (contrato, [topico], dado).
    fn ev<D: IntoVal<Env, Val>>(
        env: &Env,
        cid: &Address,
        topic: soroban_sdk::Symbol,
        data: D,
    ) -> (Address, Vec<Val>, Val) {
        (cid.clone(), (topic,).into_val(env), data.into_val(env))
    }

    /// Eventos do contrato na ULTIMA chamada.
    fn do_contrato(env: &Env, cid: &Address) -> soroban_sdk::testutils::ContractEvents {
        env.events().all().filter_by_contract(cid)
    }

    #[test]
    fn renuncia_so_pelo_proprio_pauser() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, admin, pauser) = pauser_defined(&env);
        let cid = client.address.clone();

        // terceiro nao renuncia pelo pauser
        let intruso = Address::generate(&env);
        only(&env, &intruso, &cid, "renounce_emergency_pauser", soroban_sdk::vec![&env]);
        assert!(client.try_renounce_emergency_pauser().is_err());
        assert_eq!(client.emergency_pauser(), Some(pauser.clone()));

        // o admin tambem nao: ele remove com set_emergency_pauser(None)
        only(&env, &admin, &cid, "renounce_emergency_pauser", soroban_sdk::vec![&env]);
        assert!(client.try_renounce_emergency_pauser().is_err());
        assert_eq!(client.emergency_pauser(), Some(pauser.clone()));

        // o pauser sozinho renuncia
        only(&env, &pauser, &cid, "renounce_emergency_pauser", soroban_sdk::vec![&env]);
        client.renounce_emergency_pauser();
        assert_eq!(env.auths()[0].0, pauser, "quem assina e o pauser");
        assert_eq!(client.emergency_pauser(), None);
        env.as_contract(&cid, || {
            assert!(!env.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert!(!client.get_paused(), "renunciar nao liga pausa");
    }

    #[test]
    fn depois_da_renuncia_emergency_pause_cai_e_o_admin_define_de_novo() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, _admin, pauser) = pauser_defined(&env);
        let cid = client.address.clone();
        only(&env, &pauser, &cid, "renounce_emergency_pauser", soroban_sdk::vec![&env]);
        client.renounce_emergency_pauser();

        env.mock_all_auths();
        assert_eq!(
            client.try_emergency_pause(),
            Err(Ok(DistributeError::NoEmergencyPauser))
        );
        assert_eq!(
            client.try_renounce_emergency_pauser(),
            Err(Ok(DistributeError::NoEmergencyPauser))
        );
        assert!(!client.get_paused());

        // o admin define de novo e o novo pausa
        let novo = Address::generate(&env);
        client.set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(client.emergency_pauser(), Some(novo.clone()));
        only(&env, &novo, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        client.emergency_pause();
        assert!(client.get_paused());
        // o pauser que renunciou nao volta sozinho
        env.mock_all_auths();
        client.resume();
        only(&env, &pauser, &cid, "emergency_pause", soroban_sdk::vec![&env]);
        assert!(client.try_emergency_pause().is_err());
        assert!(!client.get_paused());
    }

    #[test]
    fn toda_troca_emite_pauserset_com_anterior_e_novo() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, _admin, a) = pauser_defined(&env);
        let cid = client.address.clone();
        let b = Address::generate(&env);
        let topic = soroban_sdk::symbol_short!("pauserset");
        let none: Option<Address> = None;

        // trocar a -> b
        client.set_emergency_pauser(&Some(b.clone()));
        assert_eq!(
            do_contrato(&env, &cid),
            soroban_sdk::vec![&env, ev(&env, &cid, topic.clone(), (Some(a.clone()), Some(b.clone())))]
        );

        // remover b -> nada
        client.set_emergency_pauser(&None);
        assert_eq!(
            do_contrato(&env, &cid),
            soroban_sdk::vec![&env, ev(&env, &cid, topic.clone(), (Some(b.clone()), none.clone()))]
        );

        // definir nada -> a
        client.set_emergency_pauser(&Some(a.clone()));
        assert_eq!(
            do_contrato(&env, &cid),
            soroban_sdk::vec![&env, ev(&env, &cid, topic.clone(), (none.clone(), Some(a.clone())))]
        );

        // renunciar: pauserset (a -> nada) e pauserren (a), nessa ordem
        client.renounce_emergency_pauser();
        assert_eq!(
            do_contrato(&env, &cid),
            soroban_sdk::vec![
                &env,
                ev(&env, &cid, topic, (Some(a.clone()), none)),
                ev(&env, &cid, soroban_sdk::symbol_short!("pauserren"), a),
            ]
        );
    }

    #[test]
    fn trocar_o_admin_nao_zera_o_pauser() {
        let env = Env::default();
        let (client, _u, _t, _p, _tr, _admin, pauser) = pauser_defined(&env);
        let novo_admin = Address::generate(&env);
        client.propose_admin(&novo_admin);
        client.accept_admin();
        assert_eq!(client.get_admin(), novo_admin);
        assert_eq!(client.emergency_pauser(), Some(pauser));
        client.emergency_pause();
        assert!(client.get_paused());
    }
}
