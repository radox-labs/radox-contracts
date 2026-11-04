#![cfg(test)]
//! Suíte do v11 (os testes do v10 intactos + o bloco v11 no fim) — a base porta a ESPECIFICAÇÃO do v9.1 (134 testes + testnet 20/20, ratificada pelo
//! Pedro em 23/08) para o andaime de sala limpa. Cada regra de produto vira teste; os nomes
//! dizem a regra. U = 1 título/USDC inteiro em stroops.
use super::*;
use soroban_sdk::testutils::{Address as _, IssuerFlags, Ledger};
use soroban_sdk::{token, Address, Env};

const U: i128 = 10_000_000;
const DAY: u64 = 86_400;

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
/// registrado NO endereço canônico, o único que o F-006 aceita.
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

struct Setup<'a> {
    sale: TokenSaleClient<'a>,
    sale_id: Address,
    admin: Address,
    issuer: Address,
    company: Address,
    buyer: Address,
    sell: token::Client<'a>,
    sell_admin: token::StellarAssetClient<'a>,
    usdc: token::Client<'a>,
    usdc_admin: token::StellarAssetClient<'a>,
    cfg: Config,
}

fn mk(e: &Env, mode: u32, price: i128, min_raise: i128, deadline_ts: u64) -> Setup<'_> {
    mk_v11(e, mode, price, min_raise, deadline_ts, 0, false)
}

/// v11: `mk` com a janela de desistência e o piso de 2/3 (os v10 usam janela 0 e sem 2/3).
fn mk_v11(
    e: &Env,
    mode: u32,
    price: i128,
    min_raise: i128,
    deadline_ts: u64,
    withdraw_window_secs: u64,
    two_thirds_floor: bool,
) -> Setup<'_> {
    e.mock_all_auths();
    let admin = Address::generate(e);
    let issuer = Address::generate(e);
    let company = Address::generate(e);
    let (sell, sell_admin) = create_token_contract(e, &issuer);
    let (usdc, usdc_admin) = create_usdc_contract(e, &Address::generate(e));
    let cfg = Config {
        mode,
        admin: admin.clone(),
        issuer: issuer.clone(),
        sell_token: sell.address.clone(),
        buy_token: usdc.address.clone(),
        company: company.clone(),
        price,
        min_raise,
        deadline_ts,
        layout_version: 0, // o constructor sobrescreve
        withdraw_window_secs,
        two_thirds_floor,
    };
    let sale_id = e.register(TokenSale, (cfg.clone(),));
    let sale = TokenSaleClient::new(e, &sale_id);
    let buyer = Address::generate(e);
    usdc_admin.mint(&buyer, &(2_000 * U));
    Setup { sale, sale_id, admin, issuer, company, buyer, sell, sell_admin, usdc, usdc_admin, cfg }
}

/// Escrow padrão: 1.000 títulos depositados, preço par, ativada. Prazo em D+30.
fn setup_escrow(e: &Env, min_raise: i128) -> Setup<'_> {
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(e, MODE_ESCROW, U, min_raise, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    s
}

fn setup_direct(e: &Env) -> Setup<'_> {
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(e, MODE_DIRECT, U, 0, 0);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    s
}

fn past_deadline(e: &Env) {
    e.ledger().with_mut(|l| l.timestamp = 1_000_000 + 31 * DAY);
}

// ═══════════════════════════════════════════
//  1. Constructor — nasce pronta, atômica, validada
// ═══════════════════════════════════════════

#[test]
fn constructor_valid_escrow_starts_raising_inactive() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    let (st, active, target, sold, raised, pending) = s.sale.get_round();
    assert_eq!(st, STATUS_RAISING);
    assert!(active);
    assert_eq!(target, 1_000 * U);
    assert_eq!((sold, raised, pending), (0, 0, 0));
    assert_eq!(s.sale.version(), 11);
    assert_eq!(s.sale.get_config().layout_version, 2);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")]
fn constructor_escrow_without_deadline_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    mk(&e, MODE_ESCROW, U, 0, 0); // DeadlineRequired
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn constructor_unknown_mode_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    mk(&e, 7, U, 0, 1); // InvalidMode
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn constructor_zero_price_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    mk(&e, MODE_ESCROW, 0, 0, 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")]
fn constructor_direct_with_floor_rejected() {
    let e = Env::default();
    e.mock_all_auths();
    mk(&e, MODE_DIRECT, U, 10 * U, 0); // direct não tem rodada, logo não tem piso
}

// ═══════════════════════════════════════════
//  2. Ativação — alvo congelado do depósito; piso validado contra ele
// ═══════════════════════════════════════════

#[test]
fn activation_freezes_target_once() {
    let e = Env::default();
    let s = setup_escrow(&e, 0);
    s.sell_admin.mint(&s.sale_id, &(500 * U)); // depósito tardio não muda o alvo
    s.sale.set_active(&false);
    s.sale.set_active(&true);
    assert_eq!(s.sale.get_round().2, 1_000 * U, "alvo é write-once na 1ª ativação");
}

#[test]
fn activation_rejects_floor_above_target() {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, U, 2_000 * U, 1_000_000 + DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    assert_eq!(s.sale.try_set_active(&true), Err(Ok(SaleError::MinRaiseAboveTarget)));
    assert_eq!(s.sale.get_round().2, 0, "alvo não congelado, oferta não ativada");
}

#[test]
fn activation_without_deposit_rejected() {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, U, 0, 1_000_000 + DAY);
    assert_eq!(s.sale.try_set_active(&true), Err(Ok(SaleError::NotYetActivated)));
}

// ═══════════════════════════════════════════
//  3. Compra (escrow) — retenção, contabilidade, gates
// ═══════════════════════════════════════════

#[test]
fn buy_escrow_holds_money_and_accounts_exactly() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(300 * U));
    let (_, _, _, sold, raised, pending) = s.sale.get_round();
    assert_eq!((sold, raised, pending), (300 * U, 300 * U, 300 * U));
    assert_eq!(s.sale.get_contrib(&s.buyer), 300 * U);
    assert_eq!(s.usdc.balance(&s.sale_id), 300 * U, "dinheiro RETIDO no contrato");
    assert_eq!(s.usdc.balance(&s.company), 0, "empresa não recebe por compra");
    assert_eq!(s.sell.balance(&s.buyer), 0, "título só no fechamento");
}

#[test]
fn buy_with_nonzero_remainder_rejected() {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    // preço 3 stroops por título: 1 título custa 3/10^7 USDC → resto ≠ 0
    let s = mk(&e, MODE_ESCROW, 3, 0, 1_000_000 + DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    assert_eq!(s.sale.try_buy(&s.buyer, &1), Err(Ok(SaleError::RemainderNotZero)));
    // já um lote redondo (10^7 títulos-stroops × 3 / 10^7 = 3 stroops de USDC) passa
    s.sale.buy(&s.buyer, &U);
    assert_eq!(s.sale.get_contrib(&s.buyer), 3);
}

#[test]
fn buy_requires_sac_authorized_holder() {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let sell_sac = e.register_stellar_asset_contract_v2(issuer.clone());
    sell_sac.issuer().set_flag(IssuerFlags::RequiredFlag);
    sell_sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    let sell_admin = token::StellarAssetClient::new(&e, &sell_sac.address());
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let cfg = Config {
        mode: MODE_ESCROW, admin: admin.clone(), issuer: issuer.clone(),
        sell_token: sell_sac.address(), buy_token: usdc.address.clone(),
        company: Address::generate(&e), price: U, min_raise: 0,
        deadline_ts: 1_000_000 + DAY, layout_version: 0,
        withdraw_window_secs: 0, two_thirds_floor: false,
    };
    let sale_id = e.register(TokenSale, (cfg,));
    let sale = TokenSaleClient::new(&e, &sale_id);
    sell_admin.set_authorized(&sale_id, &true);
    sell_admin.mint(&sale_id, &(100 * U));
    sale.set_active(&true);
    let buyer = Address::generate(&e);
    usdc_admin.mint(&buyer, &(100 * U));
    // carteira sem KYC (não autorizada no SAC) não entra nem com dinheiro na mão
    sell_admin.set_authorized(&buyer, &false);
    assert_eq!(sale.try_buy(&buyer, &(10 * U)), Err(Ok(SaleError::NotAuthorizedHolder)));
    sell_admin.set_authorized(&buyer, &true);
    sale.buy(&buyer, &(10 * U));
}

#[test]
fn buy_gates_frozen_inactive_expired_overtarget() {
    let e = Env::default();
    let s = setup_escrow(&e, 0);
    s.sale.set_frozen(&s.buyer, &true);
    assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::FrozenInvestor)));
    s.sale.set_frozen(&s.buyer, &false);
    assert_eq!(s.sale.try_buy(&s.buyer, &(1_001 * U)), Err(Ok(SaleError::ExceedsRemaining)));
    s.sale.set_active(&false);
    assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::NotActive)));
    s.sale.set_active(&true);
    past_deadline(&e);
    assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::Expired)));
}

#[test]
fn terms_min_ticket_and_cap_with_closing_trade_exemption() {
    let e = Env::default();
    let s = setup_escrow(&e, 0);
    s.sale.set_terms(&(100 * U), &(950 * U));
    assert_eq!(s.sale.try_buy(&s.buyer, &(50 * U)), Err(Ok(SaleError::BelowMinTicket)));
    s.sale.buy(&s.buyer, &(900 * U));
    assert_eq!(s.sale.try_buy(&s.buyer, &(100 * U)), Err(Ok(SaleError::CapExceeded)));
    s.sale.set_terms(&(100 * U), &0);
    // a compra que FECHA a rodada (100 restantes) é isenta do ticket mínimo (D7)
    let other = Address::generate(&e);
    s.usdc_admin.mint(&other, &(200 * U));
    s.sale.buy(&other, &(100 * U));
    assert_eq!(s.sale.get_round().3, 1_000 * U, "rodada cheia");
}

// ═══════════════════════════════════════════
//  4. Fechamento — cheia a qualquer hora; no prazo com ≥ piso; senão falha
// ═══════════════════════════════════════════

#[test]
fn full_subscription_closes_anytime_and_pays_exactly_raised() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(1_000 * U));
    s.sale.close();
    s.sale.close(); // idempotente
    assert_eq!(s.usdc.balance(&s.company), 1_000 * U);
    assert_eq!(s.sale.get_round().0, STATUS_CLOSED);
}

#[test]
fn floor_reached_closes_only_at_deadline() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(500 * U));
    // antes do prazo: nem fecha (todo investidor tem a janela) nem falha
    assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)));
    assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::NotExpired)));
    past_deadline(&e);
    s.sale.close();
    assert_eq!(s.usdc.balance(&s.company), 500 * U, "empresa recebe o CAPTADO, não o alvo");
}

#[test]
fn below_floor_at_deadline_fails_and_refunds_in_full() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(300 * U));
    past_deadline(&e);
    assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)));
    s.sale.fail();
    s.sale.fail(); // idempotente
    s.sale.refund(&s.buyer);
    assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U, "principal de volta, inteiro (sem fee)");
    assert_eq!(s.usdc.balance(&s.company), 0);
    assert_eq!(s.sale.get_round().4, 0, "raised zerado");
}

#[test]
fn zero_floor_means_one_hundred_percent() {
    let e = Env::default();
    let s = setup_escrow(&e, 0);
    s.sale.buy(&s.buyer, &(999 * U));
    past_deadline(&e);
    assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)));
    s.sale.fail();
}

#[test]
fn close_requires_backing_for_every_pending_claim() {
    // Precisa de um SAC com flags REAIS (clawback habilitado no emissor) — como na mainnet.
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let issuer = Address::generate(&e);
    let sell_sac = e.register_stellar_asset_contract_v2(issuer.clone());
    sell_sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    sell_sac.issuer().set_flag(IssuerFlags::ClawbackEnabledFlag);
    let sell_admin = token::StellarAssetClient::new(&e, &sell_sac.address());
    let (usdc, usdc_admin) = create_usdc_contract(&e, &admin);
    let cfg = Config {
        mode: MODE_ESCROW, admin: admin.clone(), issuer: issuer.clone(),
        sell_token: sell_sac.address(), buy_token: usdc.address.clone(),
        company: Address::generate(&e), price: U, min_raise: 400 * U,
        deadline_ts: 1_000_000 + 30 * DAY, layout_version: 0,
        withdraw_window_secs: 0, two_thirds_floor: false,
    };
    let sale_id = e.register(TokenSale, (cfg,));
    let sale = TokenSaleClient::new(&e, &sale_id);
    sell_admin.mint(&sale_id, &(1_000 * U));
    sale.set_active(&true);
    let buyer = Address::generate(&e);
    usdc_admin.mint(&buyer, &(2_000 * U));
    sale.buy(&buyer, &(500 * U));
    sell_admin.clawback(&sale_id, &(600 * U)); // ficam 400 para 500 devidos
    past_deadline(&e);
    assert_eq!(sale.try_close(), Err(Ok(SaleError::TokensShort)));
    sell_admin.mint(&sale_id, &(100 * U));
    sale.close();
}

// ═══════════════════════════════════════════
//  5. Cancel — devolve tudo; nunca ao admin; recusado quando fechável
// ═══════════════════════════════════════════

#[test]
fn cancel_open_round_refunds_everyone_admin_gets_nothing() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(500 * U));
    s.sale.cancel();
    s.sale.cancel(); // idempotente
    s.sale.refund(&s.buyer);
    assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    assert_eq!(s.usdc.balance(&s.admin), 0, "cancel não manda NADA ao admin");
    // sobra de títulos volta à EMISSORA
    s.sale.withdraw(&(1_000 * U));
    assert_eq!(s.sell.balance(&s.issuer), 1_000 * U);
}

#[test]
fn cancel_rejected_once_round_is_closeable() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(500 * U));
    past_deadline(&e);
    assert_eq!(s.sale.try_cancel(), Err(Ok(SaleError::WrongState)), "fechável pertence aos investidores");
    s.sale.close();
}

// ═══════════════════════════════════════════
//  6. Claim / refund — exatos, ao endereço registrado; congelado retém tudo
// ═══════════════════════════════════════════

#[test]
fn claim_delivers_exact_tokens_from_contrib_times_price() {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    // preço 2 USDC por título — não-par prova a aritmética
    let s = mk(&e, MODE_ESCROW, 2 * U, 0, 1_000_000 + DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    s.sale.buy(&s.buyer, &(1_000 * U)); // custa 2.000 USDC
    s.sale.close();
    s.sale.claim(&s.buyer);
    assert_eq!(s.sell.balance(&s.buyer), 1_000 * U);
    assert_eq!(s.usdc.balance(&s.company), 2_000 * U);
    assert_eq!(s.sale.try_claim(&s.buyer), Err(Ok(SaleError::NothingToClaim)));
    assert_eq!(s.sale.get_round().5, 0, "nenhuma entrega pendente");
}

#[test]
fn frozen_investor_cannot_claim_or_refund_until_unfrozen() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(500 * U));
    s.sale.set_frozen(&s.buyer, &true);
    past_deadline(&e);
    s.sale.close();
    assert_eq!(s.sale.try_claim(&s.buyer), Err(Ok(SaleError::FrozenInvestor)));
    s.sale.set_frozen(&s.buyer, &false);
    s.sale.claim(&s.buyer);
    assert_eq!(s.sell.balance(&s.buyer), 500 * U);
}

#[test]
fn frozen_refund_held_in_failed_round_too() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    s.sale.buy(&s.buyer, &(100 * U));
    s.sale.set_frozen(&s.buyer, &true);
    past_deadline(&e);
    s.sale.fail();
    assert_eq!(s.sale.try_refund(&s.buyer), Err(Ok(SaleError::FrozenInvestor)));
    s.sale.set_frozen(&s.buyer, &false);
    s.sale.refund(&s.buyer);
    assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
}

// ═══════════════════════════════════════════
//  7. Withdraw — só títulos, só à emissora, só o que não lastreia ninguém
// ═══════════════════════════════════════════

#[test]
fn withdraw_locked_while_activated_surplus_only_after_partial_close() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    assert_eq!(s.sale.try_withdraw(&U), Err(Ok(SaleError::WrongState)), "ativada: títulos lastreiam a rodada");
    s.sale.buy(&s.buyer, &(500 * U));
    past_deadline(&e);
    s.sale.close();
    assert_eq!(s.sale.try_withdraw(&(501 * U)), Err(Ok(SaleError::ExceedsRemaining)));
    s.sale.withdraw(&(500 * U));
    assert_eq!(s.sell.balance(&s.issuer), 500 * U, "sobra vai à EMISSORA");
    assert_eq!(s.sale.try_withdraw(&U), Err(Ok(SaleError::ExceedsRemaining)), "o resto lastreia o claim");
    s.sale.claim(&s.buyer);
    assert_eq!(s.sell.balance(&s.buyer), 500 * U);
}

// ═══════════════════════════════════════════
//  8. Modo direct — contínua: paga e entrega na hora; sem rodada
// ═══════════════════════════════════════════

#[test]
fn direct_mode_pays_company_and_delivers_immediately() {
    let e = Env::default();
    let s = setup_direct(&e);
    s.sale.buy(&s.buyer, &(100 * U));
    assert_eq!(s.sell.balance(&s.buyer), 100 * U);
    assert_eq!(s.usdc.balance(&s.company), 100 * U);
    assert_eq!(s.usdc.balance(&s.sale_id), 0, "nada retido");
}

#[test]
fn direct_mode_has_no_round_functions() {
    let e = Env::default();
    let s = setup_direct(&e);
    assert_eq!(s.sale.try_close(), Err(Ok(SaleError::WrongState)));
    assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::WrongState)));
    assert_eq!(s.sale.try_cancel(), Err(Ok(SaleError::WrongState)));
    assert_eq!(s.sale.try_refund(&s.buyer), Err(Ok(SaleError::WrongState)));
    assert_eq!(s.sale.try_claim(&s.buyer), Err(Ok(SaleError::WrongState)));
}

// ═══════════════════════════════════════════
//  9. Conservação — o dinheiro fecha a conta em todos os desfechos
// ═══════════════════════════════════════════

#[test]
fn conservation_partial_close_every_stroop_accounted() {
    let e = Env::default();
    let s = setup_escrow(&e, 400 * U);
    let b2 = Address::generate(&e);
    s.usdc_admin.mint(&b2, &(1_000 * U));
    s.sale.buy(&s.buyer, &(350 * U));
    s.sale.buy(&b2, &(150 * U));
    past_deadline(&e);
    s.sale.close();
    s.sale.claim(&s.buyer);
    s.sale.claim(&b2);
    s.sale.withdraw(&(500 * U));
    // Σ saídas = Σ entradas, sem poeira em lugar nenhum
    assert_eq!(s.usdc.balance(&s.company), 500 * U);
    assert_eq!(s.sell.balance(&s.buyer) + s.sell.balance(&b2), 500 * U);
    assert_eq!(s.sell.balance(&s.issuer), 500 * U);
    assert_eq!(s.usdc.balance(&s.sale_id), 0);
    assert_eq!(s.sell.balance(&s.sale_id), 0);
}

// ═══ Red-team 24/08: valvula de recuperacao (upgrade) + alvo vendavel ═══

#[test]
fn upgrade_is_ledger_valve_and_never_locks_with_money_inside() {
    // Decisao do Pedro 24/08 (pos red-team): o admin (Ledger na mainnet) pode fazer
    // upgrade SEMPRE, inclusive com USDC de investidor escrowado — e a saida para o
    // deadlock "fechavel mas infechavel". NENHUM SaleError de estado bloqueia.
    let e = Env::default();
    let s = setup_escrow(&e, 4 * U);
    s.sale.buy(&s.buyer, &(5 * U)); // dinheiro dentro
    let hash = soroban_sdk::BytesN::from_array(&e, &[9u8; 32]);
    let r = s.sale.try_upgrade(&hash);
    // hash fake => host error (wasm inexistente); o que importa: NENHUM SaleError
    // (nem de estado, nem de auth) barra o admin — nao ha lock por desenho.
    assert!(!matches!(r, Err(Ok(_))), "nenhum SaleError bloqueia o upgrade do admin");
}

#[test]
fn set_active_rejects_unsellable_target() {
    // Red-team (LOW): alvo cujo balance*price nao e multiplo de 10^7 tem rabo
    // incompravel -> rodada condenada. Rejeitar na ativacao. min_raise=0 isola o
    // gate de vendabilidade do gate de piso.
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, 3_000_000, 0, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &15_000_005i128); // 15_000_005*3e6 % 1e7 == 5e6 != 0
    assert_eq!(
        s.sale.try_set_active(&true),
        Err(Ok(SaleError::TargetNotSellable)),
        "alvo com rabo incompravel recusado na ativacao"
    );
    // um alvo vendavel (multiplo) ativa normalmente:
    let e3 = Env::default();
    e3.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s3 = mk(&e3, MODE_ESCROW, 3_000_000, 0, 1_000_000 + 30 * DAY);
    s3.sell_admin.mint(&s3.sale_id, &(1_000 * U)); // 1000*U*3e6 % 1e7 == 0
    s3.sale.set_active(&true);
}

// ═══════════════════════════════════════════
//  10. PEND 158 — preço FORA DO PAR (deságio e prêmio)
//
//  O defeito do 158 era do par `sell_price`/`buy_price` do v9: as duas pontas
//  liam a razão em orientações OPOSTAS e uma oferta a 2,50 entregava 2,5
//  títulos por USDC (6,25x). O v10 tem UM campo — `price` = stroops de USDC
//  por 1 título inteiro — então a orientação está no nome e não há razão para
//  inverter. Estes testes provam a troca certa nos dois sentidos e que o par
//  continua idêntico bit a bit.
//
//  ARREDONDAMENTO: ninguém absorve resto. `usdc_cost_exact` recusa resto ≠ 0
//  (#10 RemainderNotZero) — não há truncamento a favor do investidor nem da
//  emissora, e por isso `claim`/`refund` são a inversa EXATA da compra.
// ═══════════════════════════════════════════

/// Roda o ciclo inteiro a um preço qualquer e devolve (usdc_da_empresa, titulos_do_comprador).
fn round_trip_at_price(price: i128, tokens_a: i128, tokens_b: i128) -> (i128, i128, i128) {
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, price, 0, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(tokens_a + tokens_b));
    s.sale.set_active(&true);
    let b2 = Address::generate(&e);
    s.usdc_admin.mint(&b2, &(10_000 * U));
    s.sale.buy(&s.buyer, &tokens_a);
    s.sale.buy(&b2, &tokens_b); // fecha a rodada (sold == target)
    s.sale.close();
    s.sale.claim(&s.buyer);
    s.sale.claim(&b2);
    // Conservação: nada preso no contrato, nada de poeira em lugar nenhum.
    assert_eq!(s.usdc.balance(&s.sale_id), 0, "nenhum stroop de USDC preso");
    assert_eq!(s.sell.balance(&s.sale_id), 0, "nenhum título preso");
    assert_eq!(s.sale.get_round().5, 0, "nenhuma entrega pendente");
    (
        s.usdc.balance(&s.company),
        s.sell.balance(&s.buyer),
        s.sell.balance(&b2),
    )
}

#[test]
fn nonpar_discount_delivers_more_tokens_per_usdc() {
    // Recebível com deságio: 0,85 USDC por título. 1.000 títulos = 850 USDC.
    let price = 8_500_000; // 0.85
    let (company_usdc, a, b) = round_trip_at_price(price, 400 * U, 600 * U);
    assert_eq!(a, 400 * U, "quem pagou 340 USDC recebeu 400 títulos");
    assert_eq!(b, 600 * U, "quem pagou 510 USDC recebeu 600 títulos");
    assert_eq!(company_usdc, 850 * U, "a empresa capta títulos × preço, não o valor de face");
    // A inversão do 158 daria 1.000/0,85 ≈ 1.176 títulos por 1.000 USDC (ou o oposto):
    assert_ne!(company_usdc, 1_000 * U, "captado NÃO é o valor de face do lote");
}

#[test]
fn nonpar_premium_delivers_fewer_tokens_per_usdc() {
    // Prêmio: 1,10 USDC por título. 1.000 títulos = 1.100 USDC.
    let price = 11_000_000; // 1.10
    let (company_usdc, a, b) = round_trip_at_price(price, 900 * U, 100 * U);
    assert_eq!(a, 900 * U);
    assert_eq!(b, 100 * U);
    assert_eq!(company_usdc, 1_100 * U);
}

#[test]
fn par_price_is_unchanged_bit_for_bit() {
    // Regressão: toda oferta viva é ao par — o caminho par não pode mudar em nada.
    let (company_usdc, a, b) = round_trip_at_price(U, 400 * U, 600 * U);
    assert_eq!((company_usdc, a, b), (1_000 * U, 400 * U, 600 * U));
}

#[test]
fn nonpar_buy_cost_and_claim_are_exact_inverses() {
    // A armadilha do float: 11 USDC ÷ 1,10 dá 9.999999999999998 em IEEE-754.
    // A chain entrega 10,0000000 títulos exatos — e é este número que o livro
    // do backend tem que gravar (espelho `tokensForUsdcStroops`).
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, 11_000_000, 0, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(10 * U));
    s.sale.set_active(&true);
    s.sale.buy(&s.buyer, &(10 * U));
    assert_eq!(s.sale.get_contrib(&s.buyer), 11 * U, "10 títulos a 1,10 custam 11,0000000 USDC");
    s.sale.close();
    s.sale.claim(&s.buyer);
    assert_eq!(s.sell.balance(&s.buyer), 10 * U, "claim devolve EXATAMENTE os 10 títulos");
}

#[test]
fn nonpar_refund_returns_every_stroop_and_unwinds_pending() {
    // Rodada abaixo do piso a 0,85: o reembolso é a inversa exata, sem poeira.
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, 8_500_000, 800 * U, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    let before = s.usdc.balance(&s.buyer);
    s.sale.buy(&s.buyer, &(100 * U)); // 85 USDC — longe do piso de 800
    assert_eq!(s.usdc.balance(&s.buyer), before - 85 * U);
    past_deadline(&e);
    s.sale.fail();
    s.sale.refund(&s.buyer);
    assert_eq!(s.usdc.balance(&s.buyer), before, "devolveu cada stroop");
    assert_eq!(s.sale.get_round().5, 0, "entregas pendentes zeradas pelo refund");
    assert_eq!(s.usdc.balance(&s.sale_id), 0);
}

#[test]
fn nonpar_inexact_purchase_is_rejected_not_rounded() {
    // Regra de arredondamento: NINGUÉM absorve o resto — a compra é recusada.
    // A 0,85, comprar 1 título-stroop custaria 0,85 stroop de USDC.
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, 8_500_000, 0, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    s.sale.set_active(&true);
    assert_eq!(s.sale.try_buy(&s.buyer, &1), Err(Ok(SaleError::RemainderNotZero)));
    // O passo comprável a 0,85 é 20 títulos-stroops (10^7 / mdc(8,5e6, 10^7) = 20):
    assert_eq!(s.sale.try_buy(&s.buyer, &19), Err(Ok(SaleError::RemainderNotZero)));
    s.sale.buy(&s.buyer, &20);
    assert_eq!(s.sale.get_contrib(&s.buyer), 17, "20 títulos-stroops × 8,5e6 / 10^7 = 17 stroops");
}

// ═══════════════════════════════════════════════════════
//  F-006 — canonical USDC SAC: aceita o canônico, rejeita o resto.
//  Só compila com `testing` DESLIGADO — quando validate_canonical_usdc está
//  vivo, igual ao WASM deployado. Rode com:
//     cargo test --no-default-features --features testnet
// ═══════════════════════════════════════════════════════
#[cfg(not(feature = "testing"))]
mod canonical_usdc_tests {
    use super::*;

    /// Guarda do próprio andaime: se a const do mock e a do lib.rs saírem de
    /// sincronia, TODA a suíte sob rede real vira falso-vermelho — este teste
    /// diz onde. (O lib.rs mantém a const privada; a comparação é indireta:
    /// o construtor só aceita o endereço do mock se ele for o canônico.)
    #[test]
    fn mock_sits_at_the_canonical_address() {
        let e = Env::default();
        let (usdc, _) = create_usdc_contract(&e, &Address::generate(&e));
        assert_eq!(usdc.address, Address::from_str(&e, mock_usdc::CANONICAL_USDC));
        setup_escrow(&e, 400 * U); // constructor aceita ⇒ é o canônico do lib.rs
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #16)")] // SaleError::UnauthorizedToken
    fn constructor_rejects_non_canonical_usdc() {
        let e = Env::default();
        e.mock_all_auths();
        let admin = Address::generate(&e);
        let issuer = Address::generate(&e);
        let (sell, _) = create_token_contract(&e, &issuer);
        // SAC gerado: nunca igual à const canônica.
        let (fake_usdc, _) = create_token_contract(&e, &Address::generate(&e));
        let cfg = Config {
            mode: MODE_ESCROW,
            admin,
            issuer,
            sell_token: sell.address.clone(),
            buy_token: fake_usdc.address.clone(),
            company: Address::generate(&e),
            price: U,
            min_raise: 400 * U,
            deadline_ts: 1_000_000 + 30 * DAY,
            layout_version: 0,
            withdraw_window_secs: 0,
            two_thirds_floor: false,
        };
        e.register(TokenSale, (cfg,));
    }
}

// ── Overflow ───────────────────────────────────────────────────────────────
// O contrato tem OITO guardas `checked_*` que levantam SaleError::Overflow, e
// ate 09/09 nenhum teste as exercitava: o v8.1 tinha `test_overflow_i128_max` e
// `test_create_max_u32_prices`, e a sala limpa nao os portou. Trocar um
// `checked_add` por `+` nao quebrava nada nesta suite. Os dois irmaos tem a
// cobertura (maturity_settlement 8 testes, yield_distributor 6). Achado da
// varredura de documentos de 09/09, §111.7.

#[test]
fn overflow_no_alvo_da_ativacao_e_recusado_nao_embrulhado() {
    // set_active calcula o alvo como balance*price. Com preco absurdo o produto
    // estoura i128: tem que voltar Overflow, nunca dar a volta e gravar um Target
    // pequeno — um alvo embrulhado fecharia a rodada com uma fracao do dinheiro.
    let e = Env::default();
    e.ledger().with_mut(|l| l.timestamp = 1_000_000);
    let s = mk(&e, MODE_ESCROW, i128::MAX / 2, 0, 1_000_000 + 30 * DAY);
    s.sell_admin.mint(&s.sale_id, &(1_000 * U));
    assert_eq!(s.sale.try_set_active(&true), Err(Ok(SaleError::Overflow)));
}

// ═══════════════════════════════════════════════════════════════════════════
//  11. v11 (F7) — desistência, janelas, 2/3, suspensão, pausa e prazos esticados.
//      Um teste por regra e por borda. `v11::setup`: 1.000 títulos, preço par, prazo D+30,
//      janela de 5 dias, ativada em T0. DIRECT e os testes v10 ficam acima, sem mudar.
// ═══════════════════════════════════════════════════════════════════════════
mod v11 {
    use super::*;
    use soroban_sdk::testutils::{Events as _, MockAuth, MockAuthInvoke};
    use soroban_sdk::{IntoVal, Val};

    const T0: u64 = 1_000_000;
    const DL: u64 = T0 + 30 * DAY; // prazo da oferta
    const W: u64 = 5 * DAY; // janela de desistência

    fn setup(e: &Env, min_raise: i128) -> Setup<'_> {
        e.ledger().with_mut(|l| l.timestamp = T0);
        let s = mk_v11(e, MODE_ESCROW, U, min_raise, DL, W, false);
        s.sell_admin.mint(&s.sale_id, &(1_000 * U));
        s.sale.set_active(&true);
        s
    }
    fn at(e: &Env, ts: u64) {
        e.ledger().with_mut(|l| l.timestamp = ts);
    }
    fn inv(e: &Env, s: &Setup) -> Address {
        let a = Address::generate(e);
        s.usdc_admin.mint(&a, &(2_000 * U));
        a
    }
    /// Só `who` assina `fn_name` — o resto cai por auth.
    fn only(e: &Env, s: &Setup, who: &Address, fn_name: &str, args: soroban_sdk::Vec<Val>) {
        e.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke { contract: &s.sale_id, fn_name, args, sub_invokes: &[] },
        }]);
    }
    fn nada(e: &Env) -> soroban_sdk::Vec<Val> {
        soroban_sdk::vec![e]
    }
    fn sym(e: &Env, x: &str) -> soroban_sdk::Symbol {
        soroban_sdk::Symbol::new(e, x)
    }

    // ── 11.1 Config e 2/3 ──

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn direct_recusa_janela_de_desistencia() {
        let e = Env::default();
        mk_v11(&e, MODE_DIRECT, U, 0, 0, W, false);
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #1)")]
    fn direct_recusa_piso_de_dois_tercos() {
        let e = Env::default();
        mk_v11(&e, MODE_DIRECT, U, 0, 0, 0, true);
    }

    #[test]
    fn dois_tercos_nas_bordas_exato_e_um_stroop_abaixo() {
        // alvo 900 USDC: 2/3 = 600 exatos.
        let tenta = |min: i128, flag: bool| {
            let e = Env::default();
            e.ledger().with_mut(|l| l.timestamp = T0);
            let s = mk_v11(&e, MODE_ESCROW, U, min, DL, W, flag);
            s.sell_admin.mint(&s.sale_id, &(900 * U));
            s.sale.try_set_active(&true)
        };
        assert!(tenta(600 * U, true).is_ok(), "exato 2/3 passa");
        assert_eq!(tenta(600 * U - 1, true), Err(Ok(SaleError::MinRaiseBelowTwoThirds)), "um stroop abaixo");
        assert_eq!(tenta(1, true), Err(Ok(SaleError::MinRaiseBelowTwoThirds)));
        assert!(tenta(900 * U, true).is_ok(), "100% passa");
        assert!(tenta(0, true).is_ok(), "min 0 = 100% passa");
        assert!(tenta(1, false).is_ok(), "sem a flag, qualquer piso (oferta estrangeira)");
    }

    #[test]
    fn dois_tercos_recusado_nao_congela_o_alvo() {
        let e = Env::default();
        e.ledger().with_mut(|l| l.timestamp = T0);
        let s = mk_v11(&e, MODE_ESCROW, U, 100 * U, DL, W, true);
        s.sell_admin.mint(&s.sale_id, &(900 * U));
        assert_eq!(s.sale.try_set_active(&true), Err(Ok(SaleError::MinRaiseBelowTwoThirds)));
        assert_eq!(s.sale.get_round().2, 0, "alvo não congelado, oferta não ativada");
    }

    // ── 11.2 Desistência ──

    #[test]
    fn desistencia_dentro_da_janela_devolve_tudo_e_desfaz_a_posicao() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.set_terms(&0, &(600 * U));
        s.sale.buy(&s.buyer, &(500 * U));
        assert_eq!(s.usdc.balance(&s.buyer), 1_500 * U);
        at(&e, T0 + W - 1);
        s.sale.withdraw_commitment(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U, "recebe TODO o USDC de volta");
        assert_eq!(s.usdc.balance(&s.sale_id), 0);
        assert_eq!(s.sale.get_round(), (STATUS_RAISING, true, 1_000 * U, 0, 0, 0), "sold, raised e pendentes descem");
        assert_eq!(s.sale.get_contrib(&s.buyer), 0);
        // o teto por investidor volta: 600 cabem de novo (se o spent não zerasse, 500+600 > 600)
        s.sale.buy(&s.buyer, &(600 * U));
    }

    #[test]
    fn janela_no_segundo_exato() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), T0 + W);
        at(&e, T0 + W);
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WithdrawWindowClosed)), "agora == fim: fechada");
        at(&e, T0 + W - 1);
        s.sale.withdraw_commitment(&s.buyer);
    }

    #[test]
    fn compra_renova_a_janela_da_ultima_compra() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(300 * U));
        at(&e, T0 + W - 10);
        s.sale.buy(&s.buyer, &(200 * U)); // renova: a janela passa a vir daqui
        at(&e, T0 + W + 5); // a 1ª janela já venceu; a 2ª não
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), T0 + W - 10 + W);
        s.sale.withdraw_commitment(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U, "devolve as DUAS compras");
        assert_eq!(s.sale.get_round().3, 0);
    }

    #[test]
    fn desistencia_vale_depois_do_prazo_e_a_compra_nao() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        at(&e, DL - 10);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, DL + 10);
        let outro = inv(&e, &s);
        assert_eq!(s.sale.try_buy(&outro, &U), Err(Ok(SaleError::Expired)), "compra depois do prazo: recusada");
        s.sale.withdraw_commitment(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn desistencia_recusas_congelado_nada_e_estado() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let outro = inv(&e, &s);
        assert_eq!(s.sale.try_withdraw_commitment(&outro), Err(Ok(SaleError::NothingToClaim)), "nada a devolver");
        s.sale.buy(&s.buyer, &(500 * U));
        s.sale.set_frozen(&s.buyer, &true);
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::FrozenInvestor)), "congelado retém, como o refund");
        s.sale.set_frozen(&s.buyer, &false);
        s.sale.cancel();
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WrongState)), "só em RAISING");
    }

    #[test]
    fn janela_zero_nao_ha_desistencia() {
        let e = Env::default();
        let s = setup_escrow(&e, 400 * U); // v10: janela 0
        s.sale.buy(&s.buyer, &(500 * U));
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WithdrawWindowClosed)));
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), 0);
        assert_eq!(s.sale.get_last_window_end(), 0);
    }

    #[test]
    fn desistencia_so_do_proprio_investidor() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        let intruso = Address::generate(&e);
        only(&e, &s, &intruso, "withdraw_commitment", (s.buyer.clone(),).into_val(&e));
        assert!(s.sale.try_withdraw_commitment(&s.buyer).is_err(), "terceiro não desiste por ele");
        only(&e, &s, &s.buyer, "withdraw_commitment", (s.buyer.clone(),).into_val(&e));
        s.sale.withdraw_commitment(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn desistencia_emite_withdrawn_com_investidor_valor_e_titulos() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        s.sale.withdraw_commitment(&s.buyer);
        let dado: soroban_sdk::Map<soroban_sdk::Symbol, Val> = soroban_sdk::map![
            &e,
            (sym(&e, "amount"), (500 * U).into_val(&e)),
            (sym(&e, "investor"), s.buyer.clone().into_val(&e)),
            (sym(&e, "tokens"), (500 * U).into_val(&e)),
        ];
        let esperado = soroban_sdk::vec![&e, (s.sale_id.clone(), (sym(&e, "withdrawn"),).into_val(&e), dado.into_val(&e))];
        // a última chamada emitiu o withdrawn do contrato (o SAC emite o transfer à parte)
        assert_eq!(e.events().all().filter_by_contract(&s.sale_id), esperado);
    }

    // ── 11.3 Vaga que reabre ──

    #[test]
    fn vaga_reaberta_cabe_a_proxima_compra_e_a_segunda_pela_mesma_vaga_e_recusada() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let (b, c, d) = (inv(&e, &s), inv(&e, &s), inv(&e, &s));
        s.sale.buy(&s.buyer, &(600 * U));
        s.sale.buy(&b, &(400 * U)); // cheia
        assert_eq!(s.sale.try_buy(&c, &U), Err(Ok(SaleError::ExceedsRemaining)), "cheia: vaga preenchida");
        s.sale.withdraw_commitment(&s.buyer); // 600 de volta à venda, sem passo extra
        assert_eq!(s.sale.get_round().3, 400 * U);
        s.sale.buy(&c, &(600 * U)); // a vaga reaberta cabe inteira
        assert_eq!(s.sale.try_buy(&d, &(600 * U)), Err(Ok(SaleError::ExceedsRemaining)), "segunda pela mesma vaga: vaga preenchida");
        assert_eq!(s.sale.get_round().3, 1_000 * U);
    }

    // ── 11.4 Fechamento e falha esperando as janelas ──

    #[test]
    fn cheia_antes_do_prazo_espera_a_ultima_janela() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(1_000 * U));
        at(&e, T0 + W - 1);
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::WithdrawWindowOpen)), "cheia, esperando janela");
        assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::NotExpired)));
        at(&e, T0 + W);
        assert!(T0 + W < DL);
        s.sale.close(); // janela vencida e cheia: fecha ANTES do prazo (DN-E11)
        assert_eq!(s.usdc.balance(&s.company), 1_000 * U);
    }

    #[test]
    fn vencida_com_minimo_espera_a_janela_da_compra_do_ultimo_minuto() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        at(&e, DL - 10);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, DL + 5);
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::WithdrawWindowOpen)));
        assert_eq!(s.sale.get_last_window_end(), DL - 10 + W);
        at(&e, DL - 10 + W);
        s.sale.close();
        assert_eq!(s.usdc.balance(&s.company), 500 * U);
    }

    #[test]
    fn minimo_atingido_mas_nao_cheia_nao_fecha_antes_do_prazo_mesmo_sem_janela_aberta() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W + 1); // janela vencida, prazo não
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)));
    }

    #[test]
    fn desistencia_pos_prazo_derruba_abaixo_do_minimo_e_a_oferta_falha_e_reembolsa_todos() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let b = inv(&e, &s);
        at(&e, DL - 10);
        s.sale.buy(&s.buyer, &(300 * U));
        s.sale.buy(&b, &(200 * U)); // 500 ≥ 400
        at(&e, DL + 1);
        assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::WithdrawWindowOpen)), "janela aberta: não falha");
        s.sale.withdraw_commitment(&s.buyer); // 200 < 400
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)));
        at(&e, DL - 10 + W);
        s.sale.fail();
        s.sale.refund(&b);
        assert_eq!(s.usdc.balance(&b), 2_000 * U, "todos recebem de volta");
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
        assert_eq!(s.usdc.balance(&s.company), 0);
        assert_eq!(s.sale.get_round().4, 0);
    }

    #[test]
    fn fail_recusado_se_o_minimo_foi_batido_a_saida_e_close() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, DL + W);
        assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::WrongState)));
        s.sale.close();
    }

    #[test]
    fn cancel_de_cheia_esperando_janela_passa_e_com_janela_vencida_nao() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(1_000 * U));
        s.sale.cancel(); // cheia, janela aberta: ainda não é fechável (DEF-C9 no v11)
        s.sale.refund(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);

        let e2 = Env::default();
        let s2 = setup(&e2, 400 * U);
        s2.sale.buy(&s2.buyer, &(1_000 * U));
        at(&e2, T0 + W);
        assert_eq!(s2.sale.try_cancel(), Err(Ok(SaleError::WrongState)), "cheia e sem janela aberta pertence aos investidores");
    }

    #[test]
    fn conservacao_apos_desistencias_parciais() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let (b, c) = (inv(&e, &s), inv(&e, &s));
        s.sale.buy(&s.buyer, &(300 * U));
        s.sale.buy(&b, &(500 * U));
        s.sale.buy(&c, &(200 * U));
        s.sale.withdraw_commitment(&b);
        let (_, _, _, sold, raised, pend) = s.sale.get_round();
        assert_eq!((sold, raised, pend), (500 * U, 500 * U, 500 * U));
        assert_eq!(s.usdc.balance(&s.sale_id), raised, "o saldo do contrato é o captado retido");
        at(&e, DL + W);
        s.sale.close();
        s.sale.claim(&s.buyer);
        s.sale.claim(&c);
        assert_eq!(s.sell.balance(&s.buyer), 300 * U);
        assert_eq!(s.sell.balance(&c), 200 * U);
        assert_eq!(s.sale.get_round().5, 0);
    }

    // ── 11.5 Suspensão ──

    #[test]
    fn suspensao_bloqueia_compra_abre_revogacao_fora_da_janela_e_trava_close_e_fail() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        let novo = inv(&e, &s);
        at(&e, T0 + W + 100); // a janela do investidor já venceu
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WithdrawWindowClosed)));
        let revoga_ate = T0 + W + 100 + 7 * DAY;
        s.sale.suspend(&revoga_ate);
        assert_eq!(s.sale.get_suspension(), (true, revoga_ate));
        assert_eq!(s.sale.try_buy(&novo, &U), Err(Ok(SaleError::Suspended)));
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::Suspended)));
        assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::Suspended)));
        s.sale.withdraw_commitment(&s.buyer); // revogação: vale fora da janela
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn revogacao_termina_no_timestamp_exato() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W + 100);
        let r = T0 + W + 1_000;
        s.sale.suspend(&r);
        at(&e, r);
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WithdrawWindowClosed)));
        at(&e, r - 1);
        s.sale.withdraw_commitment(&s.buyer);
    }

    #[test]
    fn resume_sale_reabre_a_compra_e_a_revogacao_aberta_continua_ate_o_fim() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W + 100);
        let r = T0 + W + 1_000;
        s.sale.suspend(&r);
        s.sale.resume_sale();
        assert_eq!(s.sale.get_suspension(), (false, r));
        let novo = inv(&e, &s);
        s.sale.buy(&novo, &(100 * U)); // compra reaberta
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::Suspended)), "close espera o fim da revogação mesmo retomada");
        s.sale.withdraw_commitment(&s.buyer); // a revogação do aviso segue de pé
        at(&e, r);
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::TargetNotReached)), "libera o close; aqui falta o piso");
    }

    #[test]
    fn suspensao_recusas_prazo_invalido_duplicada_e_resume_sem_suspensao() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        assert_eq!(s.sale.try_resume_sale(), Err(Ok(SaleError::NotSuspended)));
        assert_eq!(s.sale.try_suspend(&T0), Err(Ok(SaleError::InvalidRevokeUntil)), "revoke_until == agora");
        assert_eq!(s.sale.try_suspend(&(T0 - 1)), Err(Ok(SaleError::InvalidRevokeUntil)));
        s.sale.suspend(&(T0 + 100));
        assert_eq!(s.sale.try_suspend(&(T0 + 500)), Err(Ok(SaleError::Suspended)), "já suspensa");
        s.sale.resume_sale();
        s.sale.suspend(&(T0 + 50)); // segunda suspensão não encurta a revogação anterior
        assert_eq!(s.sale.get_suspension(), (true, T0 + 100));
    }

    #[test]
    fn suspensao_so_do_admin_e_nao_vale_fora_de_raising() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let intruso = Address::generate(&e);
        only(&e, &s, &intruso, "suspend", (T0 + 100,).into_val(&e));
        assert!(s.sale.try_suspend(&(T0 + 100)).is_err());
        only(&e, &s, &s.admin, "suspend", (T0 + 100,).into_val(&e));
        s.sale.suspend(&(T0 + 100));
        only(&e, &s, &intruso, "resume_sale", nada(&e));
        assert!(s.sale.try_resume_sale().is_err());
        e.mock_all_auths();
        s.sale.resume_sale();
        s.sale.cancel();
        assert_eq!(s.sale.try_suspend(&(T0 + 200)), Err(Ok(SaleError::WrongState)));
    }

    #[test]
    fn cancel_passa_com_a_oferta_suspensa_e_o_refund_devolve() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(1_000 * U));
        at(&e, T0 + W);
        s.sale.suspend(&(T0 + W + DAY));
        s.sale.cancel(); // fechável seria close; suspensa, não é
        s.sale.refund(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    // ── 11.6 Pausa e emergency_pauser ──

    fn com_pauser(e: &Env, s: &Setup) -> Address {
        let p = Address::generate(e);
        s.sale.set_emergency_pauser(&Some(p.clone()));
        p
    }

    #[test]
    fn emergency_pause_exige_pauser_configurado_e_so_ele_assina() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        assert_eq!(s.sale.try_emergency_pause(), Err(Ok(SaleError::NoEmergencyPauser)));
        let p = com_pauser(&e, &s);
        for quem in [Address::generate(&e), s.admin.clone(), s.issuer.clone()] {
            only(&e, &s, &quem, "emergency_pause", nada(&e));
            assert!(s.sale.try_emergency_pause().is_err(), "só o pauser liga");
            assert!(!s.sale.get_paused());
        }
        only(&e, &s, &p, "emergency_pause", nada(&e));
        s.sale.emergency_pause();
        assert!(s.sale.get_paused());
        assert_eq!(s.sale.emergency_pauser(), Some(p.clone()));
        assert_eq!(s.sale.get_emergency_pauser(), Some(p));
    }

    #[test]
    fn pauser_nao_faz_nada_alem_de_pausar() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let p = com_pauser(&e, &s);
        s.sale.buy(&s.buyer, &(500 * U));
        let outro = Address::generate(&e);
        let hash = soroban_sdk::BytesN::from_array(&e, &[7u8; 32]);
        macro_rules! nega {
            ($fn:literal, $args:expr, $call:expr) => {{
                only(&e, &s, &p, $fn, $args.into_val(&e));
                assert!($call.is_err(), concat!("o pauser NAO pode ", $fn));
            }};
        }
        for pausado in [false, true] {
            if pausado {
                only(&e, &s, &p, "emergency_pause", nada(&e));
                s.sale.emergency_pause();
            }
            nega!("resume", (), s.sale.try_resume());
            nega!("pause", (), s.sale.try_pause());
            nega!("set_emergency_pauser", (Some(outro.clone()),), s.sale.try_set_emergency_pauser(&Some(outro.clone())));
            nega!("upgrade", (hash.clone(),), s.sale.try_upgrade(&hash));
            nega!("cancel", (), s.sale.try_cancel());
            nega!("set_frozen", (outro.clone(), true), s.sale.try_set_frozen(&outro, &true));
            nega!("set_terms", (1i128, 2i128), s.sale.try_set_terms(&1, &2));
            nega!("withdraw", (1i128,), s.sale.try_withdraw(&1));
            nega!("suspend", (T0 + 9u64,), s.sale.try_suspend(&(T0 + 9)));
            nega!("resume_sale", (), s.sale.try_resume_sale());
            nega!("set_active", (false,), s.sale.try_set_active(&false));
            assert_eq!(s.sale.get_paused(), pausado, "o pauser não despausou nem mudou o estado");
            assert_eq!(s.sale.emergency_pauser(), Some(p.clone()));
            assert_eq!(s.sale.get_round(), (STATUS_RAISING, true, 1_000 * U, 500 * U, 500 * U, 500 * U));
            assert!(!s.sale.is_investor_frozen(&outro));
            assert_eq!(s.usdc.balance(&s.sale_id), 500 * U, "o dinheiro não se moveu");
        }
    }

    #[test]
    fn pausa_bloqueia_buy_e_desistencia() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        s.sale.pause();
        assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::ContractPaused)));
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::ContractPaused)));
        assert_eq!(s.usdc.balance(&s.sale_id), 500 * U);
        s.sale.resume();
        s.sale.buy(&s.buyer, &U);
        s.sale.withdraw_commitment(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn pausa_bloqueia_close() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(1_000 * U));
        at(&e, T0 + W);
        s.sale.pause();
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::ContractPaused)));
        s.sale.resume();
        s.sale.close();
    }

    #[test]
    fn pausa_bloqueia_fail_e_refund() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(300 * U));
        at(&e, DL + W);
        s.sale.pause();
        assert_eq!(s.sale.try_fail(), Err(Ok(SaleError::ContractPaused)));
        s.sale.resume();
        s.sale.fail();
        s.sale.pause();
        assert_eq!(s.sale.try_refund(&s.buyer), Err(Ok(SaleError::ContractPaused)));
        s.sale.resume();
        s.sale.refund(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn pausa_bloqueia_claim_e_withdraw_de_sobra() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, DL + W);
        s.sale.close();
        s.sale.pause();
        assert_eq!(s.sale.try_claim(&s.buyer), Err(Ok(SaleError::ContractPaused)));
        assert_eq!(s.sale.try_withdraw(&(500 * U)), Err(Ok(SaleError::ContractPaused)));
        s.sale.resume();
        s.sale.claim(&s.buyer);
        s.sale.withdraw(&(500 * U));
        assert_eq!(s.sell.balance(&s.buyer), 500 * U);
        assert_eq!(s.sell.balance(&s.issuer), 500 * U);
    }

    #[test]
    fn pausa_nao_bloqueia_conformidade_cancel_config_e_upgrade() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        s.sale.pause();
        s.sale.set_frozen(&s.buyer, &true);
        s.sale.set_frozen(&s.buyer, &false);
        s.sale.set_terms(&0, &0);
        s.sale.extend_ttl();
        s.sale.suspend(&(T0 + DAY));
        s.sale.resume_sale();
        let hash = soroban_sdk::BytesN::from_array(&e, &[9u8; 32]);
        assert!(!matches!(s.sale.try_upgrade(&hash), Err(Ok(_))), "nenhum SaleError barra o upgrade sob pausa");
        s.sale.cancel(); // proteção do investidor: não move dinheiro
        assert_eq!(s.sale.get_round().0, STATUS_CANCELED);
        assert_eq!(s.sale.try_refund(&s.buyer), Err(Ok(SaleError::ContractPaused)), "o refund é que espera o resume");
        s.sale.resume();
        s.sale.refund(&s.buyer);
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn pause_e_resume_so_do_admin_e_idempotentes() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let intruso = Address::generate(&e);
        only(&e, &s, &intruso, "pause", nada(&e));
        assert!(s.sale.try_pause().is_err());
        only(&e, &s, &s.admin, "pause", nada(&e));
        s.sale.pause();
        at(&e, T0 + 100);
        only(&e, &s, &s.admin, "pause", nada(&e));
        s.sale.pause(); // idempotente: NÃO reinicia o relógio da pausa
        at(&e, T0 + 250);
        assert_eq!(s.sale.get_pause_total(), 250);
        only(&e, &s, &intruso, "resume", nada(&e));
        assert!(s.sale.try_resume().is_err());
        only(&e, &s, &s.admin, "resume", nada(&e));
        s.sale.resume();
        at(&e, T0 + 900);
        only(&e, &s, &s.admin, "resume", nada(&e));
        s.sale.resume(); // despausar o não pausado não soma nada
        assert_eq!(s.sale.get_pause_total(), 250);
    }

    #[test]
    fn renuncia_so_do_proprio_pauser_e_depois_cai_em_no_emergency_pauser() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        assert_eq!(s.sale.try_renounce_emergency_pauser(), Err(Ok(SaleError::NoEmergencyPauser)));
        let p = com_pauser(&e, &s);
        for quem in [Address::generate(&e), s.admin.clone()] {
            only(&e, &s, &quem, "renounce_emergency_pauser", nada(&e));
            assert!(s.sale.try_renounce_emergency_pauser().is_err());
            assert_eq!(s.sale.emergency_pauser(), Some(p.clone()));
        }
        only(&e, &s, &p, "renounce_emergency_pauser", nada(&e));
        s.sale.renounce_emergency_pauser();
        assert_eq!(s.sale.emergency_pauser(), None);
        e.mock_all_auths();
        assert_eq!(s.sale.try_emergency_pause(), Err(Ok(SaleError::NoEmergencyPauser)));
        let novo = com_pauser(&e, &s); // o admin define de novo
        assert_eq!(s.sale.emergency_pauser(), Some(novo));
    }

    #[test]
    fn trocar_ou_remover_o_pauser_invalida_o_antigo() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let a = com_pauser(&e, &s);
        let b = Address::generate(&e);
        s.sale.set_emergency_pauser(&Some(b.clone()));
        only(&e, &s, &a, "emergency_pause", nada(&e));
        assert!(s.sale.try_emergency_pause().is_err(), "o pauser antigo não pausa mais");
        e.mock_all_auths();
        s.sale.set_emergency_pauser(&None);
        only(&e, &s, &b, "emergency_pause", nada(&e));
        assert_eq!(s.sale.try_emergency_pause(), Err(Ok(SaleError::NoEmergencyPauser)));
    }

    #[test]
    fn eventos_do_pauser_e_da_pausa_tem_os_simbolos_dos_v5() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let a = Address::generate(&e);
        let ev = |topic: &str, data: Val| (s.sale_id.clone(), (sym(&e, topic),).into_val(&e), data);
        let do_contrato = || e.events().all().filter_by_contract(&s.sale_id);
        s.sale.set_emergency_pauser(&Some(a.clone()));
        assert_eq!(do_contrato(), soroban_sdk::vec![&e, ev("pauserset", (None::<Address>, Some(a.clone())).into_val(&e))]);
        s.sale.emergency_pause();
        assert_eq!(
            do_contrato(),
            soroban_sdk::vec![&e, ev("paused", true.into_val(&e)), ev("emergpaus", a.clone().into_val(&e))]
        );
        s.sale.resume();
        assert_eq!(do_contrato(), soroban_sdk::vec![&e, ev("resumed", true.into_val(&e))]);
        s.sale.renounce_emergency_pauser();
        assert_eq!(
            do_contrato(),
            soroban_sdk::vec![
                &e,
                ev("pauserset", (Some(a.clone()), None::<Address>).into_val(&e)),
                ev("pauserren", a.into_val(&e))
            ]
        );
        s.sale.suspend(&(T0 + 100));
        assert_eq!(do_contrato(), soroban_sdk::vec![&e, ev("suspended", (T0 + 100).into_val(&e))]);
        s.sale.resume_sale();
        assert_eq!(do_contrato(), soroban_sdk::vec![&e, ev("resumedsl", true.into_val(&e))]);
    }

    // ── 11.7 Prazos esticados pela pausa ──

    #[test]
    fn pausa_fechada_estica_o_prazo_e_a_compra_cabe_ate_o_prazo_efetivo() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        assert_eq!(s.sale.get_effective_deadline(), DL);
        at(&e, T0 + 100);
        s.sale.pause();
        at(&e, T0 + 100 + 3_600);
        assert_eq!(s.sale.get_effective_deadline(), DL + 3_600, "pausa aberta: o prazo corre junto");
        s.sale.resume();
        assert_eq!(s.sale.get_effective_deadline(), DL + 3_600);
        at(&e, DL + 3_599);
        s.sale.buy(&s.buyer, &U); // depois do prazo original, antes do efetivo
        at(&e, DL + 3_600);
        assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::Expired)));
    }

    #[test]
    fn duas_pausas_somam() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        at(&e, T0 + 10);
        s.sale.pause();
        at(&e, T0 + 110);
        s.sale.resume();
        at(&e, T0 + 5_000);
        s.sale.pause();
        at(&e, T0 + 5_300);
        s.sale.resume();
        assert_eq!(s.sale.get_pause_total(), 400);
        assert_eq!(s.sale.get_effective_deadline(), DL + 400);
    }

    #[test]
    fn pausa_estende_a_janela_que_teria_vencido_durante_ela() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W - 100); // faltam 100 s de janela
        s.sale.pause();
        at(&e, T0 + W + 1_000); // a janela original já passou, mas o contrato estava pausado
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::ContractPaused)));
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), T0 + W + 1_100, "aberta: a janela anda junto com o relógio");
        s.sale.resume();
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), T0 + W + 1_100);
        s.sale.withdraw_commitment(&s.buyer); // ainda aceita: restam 100 s
        assert_eq!(s.usdc.balance(&s.buyer), 2_000 * U);
    }

    #[test]
    fn janela_esticada_fecha_no_segundo_exato() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W - 100);
        s.sale.pause();
        at(&e, T0 + W + 1_000);
        s.sale.resume();
        at(&e, T0 + W + 1_100); // o fim efetivo: fechada
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WithdrawWindowClosed)));
        at(&e, T0 + W + 1_099);
        s.sale.withdraw_commitment(&s.buyer);
    }

    #[test]
    fn a_pausa_anterior_a_compra_nao_estica_a_janela_dela() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        let b = inv(&e, &s);
        s.sale.buy(&b, &(100 * U)); // comprou em T0, antes de qualquer pausa
        at(&e, T0 + 10);
        s.sale.pause();
        at(&e, T0 + 510);
        s.sale.resume(); // 500 s pausados
        at(&e, T0 + 1_000);
        s.sale.buy(&s.buyer, &(100 * U)); // acc = 500 na compra
        at(&e, T0 + 1_100);
        s.sale.pause();
        at(&e, T0 + 1_400);
        s.sale.resume(); // +300
        assert_eq!(s.sale.get_withdraw_until(&s.buyer), T0 + 1_000 + W + 300, "só a pausa DEPOIS da compra");
        assert_eq!(s.sale.get_withdraw_until(&b), T0 + W + 800, "a do início pega as duas");
        assert_eq!(s.sale.get_last_window_end(), T0 + 1_000 + W + 300, "o máximo da oferta");
    }

    #[test]
    fn close_espera_a_janela_esticada_pela_pausa() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(1_000 * U));
        at(&e, T0 + W - 50);
        s.sale.pause();
        at(&e, T0 + W + 450);
        s.sale.resume(); // 500 s pausados
        at(&e, T0 + W + 499);
        assert_eq!(s.sale.try_close(), Err(Ok(SaleError::WithdrawWindowOpen)));
        at(&e, T0 + W + 500);
        s.sale.close();
    }

    #[test]
    fn pausa_estende_a_revogacao() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W + 100);
        let r = T0 + W + 1_100;
        s.sale.suspend(&r);
        at(&e, T0 + W + 600);
        s.sale.pause();
        at(&e, T0 + W + 2_600); // 2.000 s pausados; a revogação original (r) já passou
        s.sale.resume();
        assert_eq!(s.sale.get_suspension(), (true, r + 2_000));
        s.sale.withdraw_commitment(&s.buyer); // ainda dentro da revogação esticada
    }

    #[test]
    fn suspensao_feita_sob_pausa_conta_o_prazo_pelo_relogio_da_rede() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        s.sale.buy(&s.buyer, &(500 * U));
        at(&e, T0 + W + 100);
        s.sale.pause();
        at(&e, T0 + W + 300);
        let r = T0 + W + 1_000; // pedido em tempo absoluto, com 200 s de pausa corrente
        s.sale.suspend(&r);
        assert_eq!(s.sale.get_suspension(), (true, r), "no ato, o fim é o pedido");
        at(&e, T0 + W + 500); // a pausa segue 200 s além do pedido da suspensão
        s.sale.resume();
        assert_eq!(s.sale.get_suspension().1, r + 200, "a pausa DEPOIS do suspend estica; a antes já estava no pedido");
    }

    #[test]
    fn prazo_ja_vencido_continua_vencido_depois_de_uma_pausa() {
        let e = Env::default();
        let s = setup(&e, 400 * U);
        at(&e, DL + 10);
        assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::Expired)));
        s.sale.pause();
        at(&e, DL + 5_000);
        s.sale.resume();
        assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::Expired)), "a pausa não reabre uma oferta já vencida");
    }

    // ── 11.8 DIRECT intocado ──

    #[test]
    fn direct_nao_tem_regras_v11_alem_da_pausa() {
        let e = Env::default();
        let s = setup_direct(&e);
        assert_eq!(s.sale.try_withdraw_commitment(&s.buyer), Err(Ok(SaleError::WrongState)));
        assert_eq!(s.sale.try_suspend(&(1_000_000 + 10)), Err(Ok(SaleError::WrongState)));
        s.sale.buy(&s.buyer, &(10 * U));
        assert_eq!(s.usdc.balance(&s.company), 10 * U, "direct paga a empresa na compra, como no v10");
        s.sale.pause();
        assert_eq!(s.sale.try_buy(&s.buyer, &U), Err(Ok(SaleError::ContractPaused)));
        s.sale.resume();
        s.sale.buy(&s.buyer, &U);
        assert_eq!(s.sale.get_last_window_end(), 0);
        assert_eq!(s.sale.get_effective_deadline(), 0, "direct sem prazo");
    }
}
