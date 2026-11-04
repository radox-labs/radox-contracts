//! Suíte do agente de transferência (F3) — a spec A2/A6 executável:
//! papéis escopados, entrega atômica com repouso NÃO-autorizado, clawback só
//! da liquidação/admin, e a porta de saída (hand_over_sac).
#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::testutils::storage::Persistent as _;
use soroban_sdk::testutils::{Address as _, IssuerFlags, Ledger as _, MockAuth, MockAuthInvoke, StellarAssetContract};
use soroban_sdk::{contract, contractimpl, token, Address, Env, IntoVal};

// ── Stubs: o contrato de venda e o de liquidação chamam o agente COMO CONTRATOS
// (a auth de invocador-contrato cobre a subárvore — é o desenho do deliver). ──

#[contract]
pub struct SellerStub;
#[contractimpl]
impl SellerStub {
    pub fn do_deliver(e: Env, agent: Address, token: Address, to: Address, amount: i128) {
        TransferAgentClient::new(&e, &agent).deliver(&token, &to, &amount);
    }
    /// o abuso que a adaptação da OZ proíbe: o vendedor tentando confiscar
    pub fn do_clawback_abuse(e: Env, agent: Address, token: Address, from: Address, amount: i128) {
        TransferAgentClient::new(&e, &agent).clawback(&token, &from, &amount);
    }
}

#[contract]
pub struct SettlementStub;
#[contractimpl]
impl SettlementStub {
    pub fn do_clawback(e: Env, agent: Address, token: Address, from: Address, amount: i128) {
        TransferAgentClient::new(&e, &agent).clawback(&token, &from, &amount);
    }
}

struct World {
    e: Env,
    admin: Address,
    operator: Address,
    agent: Address,
    sac: StellarAssetContract,
    token: Address,
    seller: Address,
    settlement: Address,
    buyer: Address,
    /// O ativo de pagamento canonico deste mundo de teste.
    usdc: Address,
}

/// SAC de verdade, com as flags reais de mainnet (AUTH_REQUIRED + REVOCABLE +
/// CLAWBACK) e o admin ENTREGUE ao agente — o estado da cerimônia.
fn world() -> World {
    let e = Env::default();
    let admin = Address::generate(&e);
    let operator = Address::generate(&e);
    let issuer = Address::generate(&e);

    let sac = e.register_stellar_asset_contract_v2(issuer.clone());
    sac.issuer().set_flag(IssuerFlags::RequiredFlag);
    sac.issuer().set_flag(IssuerFlags::RevocableFlag);
    sac.issuer().set_flag(IssuerFlags::ClawbackEnabledFlag);
    let token = sac.address();

    // O `__constructor` agora exige `admin.require_auth()` — o admin CONSENTE em ser
    // instalado, em vez de ser nomeado por quem faz o deploy. Por isso o mock vem ANTES
    // do register: sem ele, todo teste morre no nascimento do contrato. (Que e o
    // comportamento certo: e exatamente a protecao que se quer.)
    e.mock_all_auths();
    let agent = e.register(
        TransferAgent,
        (Config { admin: admin.clone(), operator: operator.clone() },),
    );
    // cerimônia: o SAC passa a ser administrado pelo agente
    token::StellarAssetClient::new(&e, &token).set_admin(&agent);

    let seller = e.register(SellerStub, ());
    let settlement = e.register(SettlementStub, ());
    let buyer = Address::generate(&e);
    // Um USDC estavel para o mundo inteiro: e ELE que o registro fixa como ativo de
    // pagamento, e qualquer outro tem que ser recusado pelo transfer_dvp.
    let usdc = e.register_stellar_asset_contract_v2(Address::generate(&e)).address();
    e.set_auths(&[]); // fim do setup: cada teste decide o que autoriza

    World { e, admin, operator, agent, sac, token, seller, settlement, buyer, usdc }
}

fn client(w: &World) -> TransferAgentClient<'_> {
    TransferAgentClient::new(&w.e, &w.agent)
}

/// registra o token com admin mockado e deposita `amount` no seller via mint
fn register_and_fund(w: &World, amount: i128) {
    w.e.mock_all_auths();
    client(w).register_token(&w.token, &w.seller, &w.settlement, &w.usdc);
    client(w).mint(&w.token, &w.seller, &amount);
    w.e.set_auths(&[]);
}

fn balance(w: &World, who: &Address) -> i128 {
    token::Client::new(&w.e, &w.token).balance(who)
}
fn authorized(w: &World, who: &Address) -> bool {
    token::StellarAssetClient::new(&w.e, &w.token).authorized(who)
}

// ── Registro e emissão ──

#[test]
fn register_authorizes_seller_and_mint_funds_it() {
    let w = world();
    register_and_fund(&w, 1_000);
    assert!(client(&w).is_registered(&w.token));
    assert!(authorized(&w, &w.seller), "o seller precisa poder manter/enviar o depósito");
    assert_eq!(balance(&w, &w.seller), 1_000);
    let reg = client(&w).get_token(&w.token).unwrap();
    assert_eq!(reg.seller, w.seller);
    assert_eq!(reg.settlement, w.settlement);
}

#[test]
fn register_twice_is_rejected_update_is_the_path() {
    let w = world();
    register_and_fund(&w, 1);
    w.e.mock_all_auths();
    let err = client(&w).try_register_token(&w.token, &w.seller, &w.settlement, &w.usdc);
    assert_eq!(err, Err(Ok(AgentError::AlreadyRegistered)));
    // update troca os vínculos
    let seller2 = w.e.register(SellerStub, ());
    client(&w).update_token(&w.token, &seller2, &w.settlement, &w.usdc);
    assert_eq!(client(&w).get_token(&w.token).unwrap().seller, seller2);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn register_requires_admin() {
    let w = world();
    // ninguém autorizado → require_auth do admin explode
    client(&w).register_token(&w.token, &w.seller, &w.settlement, &w.usdc);
}

#[test]
fn mint_requires_admin_and_registration() {
    let w = world();
    w.e.mock_all_auths();
    let err = client(&w).try_mint(&w.token, &w.buyer, &10);
    assert_eq!(err, Err(Ok(AgentError::NotRegistered)), "mint só em token registrado");
}

// ── Entrega atômica (o caminho quente) ──

#[test]
fn deliver_by_registered_seller_leaves_holder_unauthorized() {
    let w = world();
    register_and_fund(&w, 1_000);
    // o STUB (contrato de venda) chama o agente — auth de invocador, sem mocks
    SellerStubClient::new(&w.e, &w.seller).do_deliver(&w.agent, &w.token, &w.buyer, &400);
    assert_eq!(balance(&w, &w.buyer), 400, "a entrega aconteceu");
    assert_eq!(balance(&w, &w.seller), 600);
    assert!(!authorized(&w, &w.buyer), "estado de repouso: NÃO autorizado (SEP-8)");
    // conservação de OFERTA no desenho clawback+mint: −400 no seller, +400 no
    // buyer, na MESMA invocação — nada nasce nem some (regra da casa: o teste
    // que inventa a própria premissa precisa da invariante de conservação)
    assert_eq!(balance(&w, &w.seller) + balance(&w, &w.buyer), 1_000);
}

#[test]
#[should_panic]
fn delivered_holder_cannot_transfer_p2p() {
    let w = world();
    register_and_fund(&w, 1_000);
    SellerStubClient::new(&w.e, &w.seller).do_deliver(&w.agent, &w.token, &w.buyer, &400);
    // holder congelado tenta mandar para outro — o SAC recusa (deauthorized)
    let other = Address::generate(&w.e);
    w.e.mock_all_auths();
    token::Client::new(&w.e, &w.token).transfer(&w.buyer, &other, &1);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn deliver_by_stranger_contract_is_rejected() {
    let w = world();
    register_and_fund(&w, 1_000);
    // um contrato NÃO registrado tenta entregar — require_auth(seller) explode
    let impostor = w.e.register(SellerStub, ());
    SellerStubClient::new(&w.e, &impostor).do_deliver(&w.agent, &w.token, &w.buyer, &1);
}

#[test]
fn deliver_rejects_unregistered_token_and_bad_amount() {
    let w = world();
    register_and_fund(&w, 10);
    let other_sac = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e));
    w.e.mock_all_auths();
    let err = client(&w).try_deliver(&other_sac.address(), &w.buyer, &1);
    assert_eq!(err, Err(Ok(AgentError::NotRegistered)));
    let err = client(&w).try_deliver(&w.token, &w.buyer, &0);
    assert_eq!(err, Err(Ok(AgentError::InvalidAmount)));
}

// ── Clawback (liquidação) ──

#[test]
fn clawback_by_registered_settlement_ignores_frozen_state() {
    let w = world();
    register_and_fund(&w, 1_000);
    SellerStubClient::new(&w.e, &w.seller).do_deliver(&w.agent, &w.token, &w.buyer, &400);
    assert!(!authorized(&w, &w.buyer));
    SettlementStubClient::new(&w.e, &w.settlement).do_clawback(&w.agent, &w.token, &w.buyer, &400);
    assert_eq!(balance(&w, &w.buyer), 0, "resgate completo mesmo congelado");
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn clawback_by_seller_is_rejected() {
    let w = world();
    register_and_fund(&w, 100);
    // adaptação ≠ cópia da OZ: quem VENDE não confisca
    SellerStubClient::new(&w.e, &w.seller).do_clawback_abuse(&w.agent, &w.token, &w.buyer, &1);
}

#[test]
fn admin_clawback_works_with_admin_auth() {
    let w = world();
    register_and_fund(&w, 500);
    SellerStubClient::new(&w.e, &w.seller).do_deliver(&w.agent, &w.token, &w.buyer, &500);
    w.e.mock_all_auths();
    client(&w).admin_clawback(&w.token, &w.buyer, &200);
    assert_eq!(balance(&w, &w.buyer), 300);
}

// ── Gate de KYC / congelamento ──

#[test]
fn set_authorized_is_operator_scoped_admin_has_own_door() {
    let w = world();
    register_and_fund(&w, 10);
    // operator liga
    w.e.mock_auths(&[MockAuth {
        address: &w.operator,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "set_authorized",
            args: (w.token.clone(), w.buyer.clone(), true).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).set_authorized(&w.token, &w.buyer, &true);
    assert!(authorized(&w, &w.buyer));
    // admin desliga pela porta própria
    w.e.mock_auths(&[MockAuth {
        address: &w.admin,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "admin_set_authorized",
            args: (w.token.clone(), w.buyer.clone(), false).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).admin_set_authorized(&w.token, &w.buyer, &false);
    assert!(!authorized(&w, &w.buyer));
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn set_authorized_rejects_random_signer() {
    let w = world();
    register_and_fund(&w, 10);
    let mallory = Address::generate(&w.e);
    w.e.mock_auths(&[MockAuth {
        address: &mallory,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "set_authorized",
            args: (w.token.clone(), w.buyer.clone(), true).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).set_authorized(&w.token, &w.buyer, &true);
}

// ── Secundário intermediado pelo patrocinador (DvP) ──

/// SAC de DINHEIRO (o USDC do desenho): emissor sem AUTH_REQUIRED, como o
/// canônico — quem recebe não precisa do gate do agente.
fn cash(w: &World, holder: &Address, amount: i128) -> Address {
    // §DvP — cunha do USDC DO MUNDO. Antes criava um SAC novo por chamada, o que
    // agora seria recusado: o registro fixa qual ativo conta como dinheiro.
    let usdc = w.usdc.clone();
    w.e.mock_all_auths();
    token::StellarAssetClient::new(&w.e, &usdc).mint(holder, &amount);
    w.e.set_auths(&[]);
    usdc
}

/// mundo com 400 do título já na mão do w.buyer (que agora REVENDE)
fn world_with_holder(w: &World) {
    register_and_fund(w, 1_000);
    SellerStubClient::new(&w.e, &w.seller).do_deliver(&w.agent, &w.token, &w.buyer, &400);
}

// ── Governança ──

#[test]
fn admin_rotation_is_two_step() {
    let w = world();
    let new_admin = Address::generate(&w.e);
    w.e.mock_all_auths();
    let err = client(&w).try_accept_admin();
    assert_eq!(err, Err(Ok(AgentError::NoPendingAdmin)), "aceitar sem proposta");
    client(&w).propose_admin(&new_admin);
    assert_eq!(client(&w).get_config().admin, w.admin, "proposta não muda nada");
    client(&w).accept_admin();
    assert_eq!(client(&w).get_config().admin, new_admin);
}

#[test]
fn hand_over_sac_returns_admin_and_forgets_token() {
    let w = world();
    register_and_fund(&w, 10);
    let exit = Address::generate(&w.e);
    w.e.mock_all_auths();
    client(&w).hand_over_sac(&w.token, &exit);
    assert!(!client(&w).is_registered(&w.token), "o agente esquece o token");
    // o novo admin manda: autoriza e minta direto no SAC (AUTH_REQUIRED exige
    // a ordem — mint em endereço não autorizado é recusado pelo próprio SAC)
    let sac_c = token::StellarAssetClient::new(&w.e, &w.token);
    sac_c.set_authorized(&w.buyer, &true);
    sac_c.mint(&w.buyer, &7);
    assert_eq!(balance(&w, &w.buyer), 7);
}

#[test]
fn version_reports_five() {
    let w = world();
    assert_eq!(client(&w).version(), 5);
}

/// O admin CONSENTE em ser instalado — não é apenas nomeado por quem faz o deploy.
///
/// É a razão que o `yield_distributor` documenta: impede instalar um admin que o
/// deployer não controla (endereço digitado errado, ou fornecido por outra pessoa). E
/// este é o único ponto onde um admin entra sem passar pelo fluxo de dois passos.
///
/// Contrato `no_std`: sem `catch_unwind`, a prova é o `should_panic` — sem auth, o
/// nascimento do contrato aborta.
#[test]
#[should_panic]
fn constructor_requires_the_admin_to_consent() {
    let e = Env::default();
    let admin = Address::generate(&e);
    let operator = Address::generate(&e);
    e.set_auths(&[]); // nada autorizado
    e.register(TransferAgent, (Config { admin, operator },));
}

// ══ v2 (phygital) ══════════════════════════════════════════════════════════
//
// Cada recusa do desenho (PHYGITAL_DESIGN.md §1.2) tem um teste, e cada caminho feliz
// confere a CONSERVAÇÃO — título e dinheiro — além do resultado.

struct Payees {
    gallery: Address,
    radox: Address,
    artist: Address,
}

/// Token registrado + payees fixados pelo admin + o título já na mão do `w.buyer`
/// (400 de 1.000, entregues pelo contrato de venda — o estado que a produção alcança).
fn phygital(w: &World) -> Payees {
    world_with_holder(w);
    let p = Payees {
        gallery: Address::generate(&w.e),
        radox: Address::generate(&w.e),
        artist: Address::generate(&w.e),
    };
    w.e.mock_all_auths();
    client(w).set_payees(
        &w.token,
        &soroban_sdk::vec![&w.e, p.gallery.clone(), p.radox.clone(), p.artist.clone()],
    );
    w.e.set_auths(&[]);
    p
}

fn leg(to: &Address, amount: i128) -> Leg {
    Leg { to: to.clone(), amount }
}
fn usdc_of(w: &World, who: &Address) -> i128 {
    token::Client::new(&w.e, &w.usdc).balance(who)
}

// ── set_payees ──

#[test]
fn set_payees_stores_and_replaces_the_list() {
    let w = world();
    let p = phygital(&w);
    assert_eq!(client(&w).get_payees(&w.token).len(), 3);
    w.e.mock_all_auths();
    client(&w).set_payees(&w.token, &soroban_sdk::vec![&w.e, p.gallery.clone()]);
    let now = client(&w).get_payees(&w.token);
    assert_eq!(now.len(), 1, "a lista é substituída, não somada");
    assert_eq!(now.get(0).unwrap(), p.gallery);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn set_payees_requires_admin() {
    let w = world();
    world_with_holder(&w);
    // o OPERATOR (chave quente) tenta se pôr como recebedor
    w.e.mock_auths(&[MockAuth {
        address: &w.operator,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "set_payees",
            args: (w.token.clone(), soroban_sdk::vec![&w.e, w.operator.clone()]).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).set_payees(&w.token, &soroban_sdk::vec![&w.e, w.operator.clone()]);
}

#[test]
fn set_payees_refuses_unregistered_token_and_too_many() {
    let w = world();
    world_with_holder(&w);
    w.e.mock_all_auths();
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(
        client(&w).try_set_payees(&other, &soroban_sdk::vec![&w.e, w.buyer.clone()]),
        Err(Ok(AgentError::NotRegistered))
    );
    let mut many = soroban_sdk::Vec::new(&w.e);
    for _ in 0..7 {
        many.push_back(Address::generate(&w.e));
    }
    assert_eq!(client(&w).try_set_payees(&w.token, &many), Err(Ok(AgentError::TooManyLegs)));
}

// ── pay ──

#[test]
fn pay_splits_to_payees_and_conserves_money() {
    let w = world();
    let p = phygital(&w);
    cash(&w, &w.buyer, 1_000);
    w.e.mock_all_auths();
    client(&w).pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 120), leg(&p.radox, 80)], &42);
    assert_eq!(usdc_of(&w, &w.buyer), 800);
    assert_eq!(usdc_of(&w, &p.gallery), 120);
    assert_eq!(usdc_of(&w, &p.radox), 80);
    assert_eq!(usdc_of(&w, &w.buyer) + usdc_of(&w, &p.gallery) + usdc_of(&w, &p.radox), 1_000);
    assert_eq!(balance(&w, &w.buyer), 400, "pagar não mexe no título");
}

/// `pay` com exatamente os signatários escolhidos (0 = operator, 1 = pagador).
fn pay_with(signers: &[usize]) {
    let w = world();
    let p = phygital(&w);
    cash(&w, &w.buyer, 1_000);
    let legs = soroban_sdk::vec![&w.e, leg(&p.gallery, 10)];
    let args: soroban_sdk::Vec<soroban_sdk::Val> = (w.token.clone(), w.buyer.clone(), w.usdc.clone(), legs.clone(), 1u64).into_val(&w.e);
    let sub = [MockAuthInvoke {
        contract: &w.usdc,
        fn_name: "transfer",
        args: (w.buyer.clone(), p.gallery.clone(), 10i128).into_val(&w.e),
        sub_invokes: &[],
    }];
    let plain = MockAuthInvoke { contract: &w.agent, fn_name: "pay", args: args.clone(), sub_invokes: &[] };
    let paying = MockAuthInvoke { contract: &w.agent, fn_name: "pay", args, sub_invokes: &sub };
    let all = [MockAuth { address: &w.operator, invoke: &plain }, MockAuth { address: &w.buyer, invoke: &paying }];
    let auths: std::vec::Vec<MockAuth> = signers.iter().map(|i| all[*i].clone()).collect();
    w.e.mock_auths(&auths);
    client(&w).pay(&w.token, &w.buyer, &w.usdc, &legs, &1);
    assert_eq!(usdc_of(&w, &p.gallery), 10);
}

#[test]
fn pay_passes_with_operator_and_payer() {
    pay_with(&[0, 1]);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn pay_requires_the_payer() {
    pay_with(&[0]);
}

/// v2.1: o pagamento sempre passa pelo backend — o pagador sozinho não paga pelo agente.
#[test]
#[should_panic(expected = "Unauthorized")]
fn pay_requires_the_operator() {
    pay_with(&[1]);
}

#[test]
fn pay_refusals() {
    let w = world();
    let p = phygital(&w);
    cash(&w, &w.buyer, 1_000);
    w.e.mock_all_auths();
    let c = client(&w);
    let stranger = Address::generate(&w.e);
    assert_eq!(
        c.try_pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&stranger, 10)], &1),
        Err(Ok(AgentError::PayeeNotRegistered)),
        "perna para fora dos payees"
    );
    assert_eq!(
        c.try_pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::Vec::new(&w.e), &1),
        Err(Ok(AgentError::EmptyLegs))
    );
    let mut seven = soroban_sdk::Vec::new(&w.e);
    for _ in 0..7 {
        seven.push_back(leg(&p.gallery, 1));
    }
    assert_eq!(c.try_pay(&w.token, &w.buyer, &w.usdc, &seven, &1), Err(Ok(AgentError::TooManyLegs)));
    assert_eq!(
        c.try_pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 0)], &1),
        Err(Ok(AgentError::InvalidAmount))
    );
    assert_eq!(
        c.try_pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 5), leg(&p.radox, -5)], &1),
        Err(Ok(AgentError::InvalidAmount)),
        "perna negativa não compensa outra"
    );
    let fake = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(
        c.try_pay(&w.token, &w.buyer, &fake, &soroban_sdk::vec![&w.e, leg(&p.gallery, 1)], &1),
        Err(Ok(AgentError::WrongPaymentAsset))
    );
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(
        c.try_pay(&other, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 1)], &1),
        Err(Ok(AgentError::NotRegistered))
    );
    assert_eq!(usdc_of(&w, &w.buyer), 1_000, "nenhuma recusa moveu dinheiro");
}

#[test]
fn pay_is_atomic_when_one_leg_cannot_be_paid() {
    let w = world();
    let p = phygital(&w);
    cash(&w, &w.buyer, 100);
    w.e.mock_all_auths();
    let r = client(&w).try_pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 60), leg(&p.radox, 60)], &1);
    assert!(r.is_err(), "a segunda perna estoura o saldo");
    assert_eq!(usdc_of(&w, &w.buyer), 100, "nem a primeira perna ficou paga");
    assert_eq!(usdc_of(&w, &p.gallery), 0);
}

// ── transfer_dvp_split ──

/// Revenda: w.buyer (holder) vende 400 ao buyer2; o dinheiro se divide entre Radox,
/// galeria, artista e o próprio vendedor.
#[test]
fn split_swaps_title_and_pays_every_leg() {
    let w = world();
    let p = phygital(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 10_000);
    let legs = soroban_sdk::vec![&w.e, leg(&p.radox, 30), leg(&p.gallery, 40), leg(&p.artist, 10), leg(&w.buyer, 920)];
    w.e.mock_all_auths();
    client(&w).transfer_dvp_split(&w.buyer, &buyer2, &w.token, &400, &w.usdc, &legs, &7);

    assert_eq!(balance(&w, &w.buyer), 0, "o vendedor entregou a obra inteira");
    assert_eq!(balance(&w, &buyer2), 400);
    assert!(!authorized(&w, &buyer2), "repouso do novo holder: NÃO autorizado");
    assert_eq!(balance(&w, &w.seller) + balance(&w, &w.buyer) + balance(&w, &buyer2), 1_000, "oferta conservada");
    assert_eq!(usdc_of(&w, &buyer2), 9_000, "o comprador pagou Σ pernas = 1.000");
    assert_eq!(usdc_of(&w, &p.radox), 30);
    assert_eq!(usdc_of(&w, &p.gallery), 40);
    assert_eq!(usdc_of(&w, &p.artist), 10);
    assert_eq!(usdc_of(&w, &w.buyer), 920);
}

/// Venda primária: vende o DEPÓSITO do seller registrado (que fica autorizado — o
/// sanduíche preserva o estado de quem já estava autorizado).
#[test]
fn split_primary_from_the_registered_seller_keeps_its_authorization() {
    let w = world();
    let p = phygital(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 1_000);
    w.e.mock_all_auths();
    client(&w).set_primary_price(&w.token, &16_666_666);
    client(&w).transfer_dvp_split(&w.seller, &buyer2, &w.token, &600, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 900), leg(&p.radox, 100)], &1);
    assert_eq!(balance(&w, &w.seller), 0);
    assert_eq!(balance(&w, &buyer2), 600);
    assert!(authorized(&w, &w.seller), "o depósito do seller registrado segue autorizado");
    assert!(!authorized(&w, &buyer2));
}

fn split_refusal_world() -> (World, Payees, Address) {
    let w = world();
    let p = phygital(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 1_000);
    (w, p, buyer2)
}

fn run_split_with(signers_pick: &[usize]) {
    let (w, p, buyer2) = split_refusal_world();
    let legs = soroban_sdk::vec![&w.e, leg(&p.gallery, 50), leg(&w.buyer, 50)];
    let args: soroban_sdk::Vec<soroban_sdk::Val> = (w.buyer.clone(), buyer2.clone(), w.token.clone(), 100i128, w.usdc.clone(), legs.clone(), 9u64).into_val(&w.e);
    let pay_sub = [
        MockAuthInvoke {
            contract: &w.usdc,
            fn_name: "transfer",
            args: (buyer2.clone(), p.gallery.clone(), 50i128).into_val(&w.e),
            sub_invokes: &[],
        },
        MockAuthInvoke {
            contract: &w.usdc,
            fn_name: "transfer",
            args: (buyer2.clone(), w.buyer.clone(), 50i128).into_val(&w.e),
            sub_invokes: &[],
        },
    ];
    // Os três `require_auth` exatos que a troca pede; o do comprador cobre as pernas.
    // Com os três, passa — é o controle positivo das recusas, que tiram um de cada vez.
    let plain = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args: args.clone(), sub_invokes: &[] };
    let paying = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args, sub_invokes: &pay_sub };
    let all = [
        MockAuth { address: &w.operator, invoke: &plain },
        MockAuth { address: &w.buyer, invoke: &plain },
        MockAuth { address: &buyer2, invoke: &paying },
    ];
    let auths: std::vec::Vec<MockAuth> = signers_pick.iter().map(|i| all[*i].clone()).collect();
    w.e.mock_auths(&auths);
    client(&w).transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &legs, &9);
    assert_eq!(balance(&w, &buyer2), 100);
}

#[test]
fn split_passes_with_exactly_operator_seller_and_buyer() {
    run_split_with(&[0, 1, 2]);
}

/// O buraco da v1: chave do operator + comprador cúmplice, SEM o vendedor.
#[test]
#[should_panic(expected = "Unauthorized")]
fn split_requires_the_seller() {
    run_split_with(&[0, 2]);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn split_requires_the_buyer() {
    run_split_with(&[0, 1]);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn split_requires_the_operator() {
    run_split_with(&[1, 2]);
}

#[test]
fn split_refusals_move_nothing() {
    let (w, p, buyer2) = split_refusal_world();
    w.e.mock_all_auths();
    let c = client(&w);
    let stranger = Address::generate(&w.e);
    let ok_legs = soroban_sdk::vec![&w.e, leg(&p.gallery, 10)];

    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &soroban_sdk::vec![&w.e, leg(&stranger, 10)], &1),
        Err(Ok(AgentError::PayeeNotRegistered)),
        "a chave quente não cria destino para o dinheiro"
    );
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &soroban_sdk::Vec::new(&w.e), &1),
        Err(Ok(AgentError::EmptyLegs)),
        "DvP sem preço não é DvP"
    );
    let mut seven = soroban_sdk::Vec::new(&w.e);
    for _ in 0..7 {
        seven.push_back(leg(&p.gallery, 1));
    }
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &seven, &1),
        Err(Ok(AgentError::TooManyLegs))
    );
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &0, &w.usdc, &ok_legs, &1),
        Err(Ok(AgentError::InvalidAmount))
    );
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 0)], &1),
        Err(Ok(AgentError::InvalidAmount))
    );
    let fake = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &fake, &ok_legs, &1),
        Err(Ok(AgentError::WrongPaymentAsset))
    );
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &other, &100, &w.usdc, &ok_legs, &1),
        Err(Ok(AgentError::NotRegistered))
    );
    assert_eq!(
        c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &401, &w.usdc, &ok_legs, &1),
        Err(Ok(AgentError::InsufficientBalance)),
        "o vendedor tem 400"
    );
    assert_eq!(balance(&w, &w.buyer), 400, "nenhuma recusa moveu o título");
    assert_eq!(usdc_of(&w, &buyer2), 1_000, "nenhuma recusa moveu dinheiro");
}

#[test]
fn split_is_atomic_when_the_buyer_cannot_pay() {
    let (w, p, buyer2) = split_refusal_world(); // buyer2 tem 1.000
    w.e.mock_all_auths();
    let r = client(&w).try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p.gallery, 600), leg(&w.buyer, 600)], &1);
    assert!(r.is_err());
    assert_eq!(balance(&w, &w.buyer), 400, "o título NÃO se moveu");
    assert_eq!(balance(&w, &buyer2), 0);
    assert_eq!(usdc_of(&w, &buyer2), 1_000, "nem a primeira perna ficou paga");
}

// ── redeem ──

#[test]
fn redeem_burns_with_the_holder_consent() {
    let w = world();
    phygital(&w);
    w.e.mock_all_auths();
    client(&w).redeem(&w.token, &w.buyer, &400, &3);
    assert_eq!(balance(&w, &w.buyer), 0, "queimado");
    assert_eq!(balance(&w, &w.seller), 600, "ninguém mais foi tocado");
}

fn redeem_with(signers: &[usize]) {
    let w = world();
    phygital(&w);
    let all = [&w.operator, &w.buyer];
    let args: soroban_sdk::Vec<soroban_sdk::Val> = (w.token.clone(), w.buyer.clone(), 400i128, 3u64).into_val(&w.e);
    let invoke = MockAuthInvoke { contract: &w.agent, fn_name: "redeem", args, sub_invokes: &[] };
    let auths: std::vec::Vec<MockAuth> = signers.iter().map(|i| MockAuth { address: all[*i], invoke: &invoke }).collect();
    w.e.mock_auths(&auths);
    client(&w).redeem(&w.token, &w.buyer, &400, &3);
    assert_eq!(balance(&w, &w.buyer), 0);
}

#[test]
fn redeem_passes_with_operator_and_holder() {
    redeem_with(&[0, 1]);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn redeem_requires_the_holder() {
    redeem_with(&[0]);
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn redeem_requires_the_operator() {
    redeem_with(&[1]);
}

#[test]
fn redeem_refusals() {
    let w = world();
    phygital(&w);
    w.e.mock_all_auths();
    assert_eq!(client(&w).try_redeem(&w.token, &w.buyer, &0, &1), Err(Ok(AgentError::InvalidAmount)));
    assert_eq!(client(&w).try_redeem(&w.token, &w.buyer, &401, &1), Err(Ok(AgentError::InsufficientBalance)));
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(client(&w).try_redeem(&other, &w.buyer, &1, &1), Err(Ok(AgentError::NotRegistered)));
    assert_eq!(balance(&w, &w.buyer), 400);
}

// ── admin_transfer ──

#[test]
fn admin_transfer_moves_a_fraction_and_conserves_supply() {
    let w = world();
    let p = phygital(&w);
    w.e.mock_all_auths();
    client(&w).admin_transfer(&w.token, &w.buyer, &soroban_sdk::vec![&w.e, leg(&p.gallery, 60), leg(&p.radox, 40)], &5);
    assert_eq!(balance(&w, &w.buyer), 300);
    assert_eq!(balance(&w, &p.gallery), 60);
    assert_eq!(balance(&w, &p.radox), 40);
    assert_eq!(
        balance(&w, &w.seller) + balance(&w, &w.buyer) + balance(&w, &p.gallery) + balance(&w, &p.radox),
        1_000,
        "oferta conservada"
    );
    assert!(!authorized(&w, &p.gallery) && !authorized(&w, &p.radox), "repouso NÃO autorizado");
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn admin_transfer_requires_admin() {
    let w = world();
    let p = phygital(&w);
    let legs = soroban_sdk::vec![&w.e, leg(&p.gallery, 1)];
    // o operator (chave quente) tenta tomar uma fração
    w.e.mock_auths(&[MockAuth {
        address: &w.operator,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "admin_transfer",
            args: (w.token.clone(), w.buyer.clone(), legs.clone(), 5u64).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).admin_transfer(&w.token, &w.buyer, &legs, &5);
}

#[test]
fn admin_transfer_refusals() {
    let w = world();
    let p = phygital(&w);
    w.e.mock_all_auths();
    let c = client(&w);
    assert_eq!(c.try_admin_transfer(&w.token, &w.buyer, &soroban_sdk::Vec::new(&w.e), &1), Err(Ok(AgentError::EmptyLegs)));
    assert_eq!(
        c.try_admin_transfer(&w.token, &w.buyer, &soroban_sdk::vec![&w.e, leg(&p.gallery, 0)], &1),
        Err(Ok(AgentError::InvalidAmount))
    );
    let mut seven = soroban_sdk::Vec::new(&w.e);
    for _ in 0..7 {
        seven.push_back(leg(&p.gallery, 1));
    }
    assert_eq!(c.try_admin_transfer(&w.token, &w.buyer, &seven, &1), Err(Ok(AgentError::TooManyLegs)));
    assert_eq!(
        c.try_admin_transfer(&w.token, &w.buyer, &soroban_sdk::vec![&w.e, leg(&p.gallery, 300), leg(&p.radox, 101)], &1),
        Err(Ok(AgentError::InsufficientBalance)),
        "Σ pernas > saldo do holder"
    );
    assert_eq!(balance(&w, &w.buyer), 400);
}

// ══ v2.1 ═══════════════════════════════════════════════════════════════════

/// Primária pelo seller REGISTRADO: operator + comprador, SEM o seller (chave fria em
/// produção). Piso: ⌈16.666.666 × 600 / 10^7⌉ = 1.000.
fn primary_with(price: Option<i128>, legs_of: fn(&World, &Payees) -> soroban_sdk::Vec<Leg>) -> (World, Payees, Address, Result<(), AgentError>) {
    let w = world();
    let p = phygital(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 10_000);
    if let Some(px) = price {
        w.e.mock_all_auths();
        client(&w).set_primary_price(&w.token, &px);
    }
    let legs = legs_of(&w, &p);
    let args: soroban_sdk::Vec<soroban_sdk::Val> =
        (w.seller.clone(), buyer2.clone(), w.token.clone(), 600i128, w.usdc.clone(), legs.clone(), 2u64).into_val(&w.e);
    let subs: std::vec::Vec<MockAuthInvoke> = legs
        .iter()
        .map(|l| MockAuthInvoke {
            contract: &w.usdc,
            fn_name: "transfer",
            args: (buyer2.clone(), l.to.clone(), l.amount).into_val(&w.e),
            sub_invokes: &[],
        })
        .collect();
    let plain = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args: args.clone(), sub_invokes: &[] };
    let paying = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args, sub_invokes: &subs };
    w.e.mock_auths(&[MockAuth { address: &w.operator, invoke: &plain }, MockAuth { address: &buyer2, invoke: &paying }]);
    let r = client(&w)
        .try_transfer_dvp_split(&w.seller, &buyer2, &w.token, &600, &w.usdc, &legs, &2)
        .map(|_| ())
        .map_err(|e| e.unwrap());
    (w, p, buyer2, r)
}

#[test]
fn primary_sells_without_the_registered_seller_signature_at_the_fixed_price() {
    let (w, p, buyer2, r) = primary_with(Some(16_666_666), |w, p| soroban_sdk::vec![&w.e, leg(&p.gallery, 900), leg(&p.radox, 100)]);
    assert_eq!(r, Ok(()));
    assert_eq!(balance(&w, &buyer2), 600);
    assert_eq!(balance(&w, &w.seller), 0);
    assert_eq!(usdc_of(&w, &p.gallery) + usdc_of(&w, &p.radox), 1_000);
}

#[test]
fn primary_refuses_below_the_fixed_price() {
    let (w, _p, buyer2, r) = primary_with(Some(16_666_666), |w, p| soroban_sdk::vec![&w.e, leg(&p.gallery, 899), leg(&p.radox, 100)]);
    assert_eq!(r, Err(AgentError::BelowPrimaryPrice), "999 < ⌈999,99996⌉ = 1.000");
    assert_eq!(balance(&w, &buyer2), 0);
}

#[test]
fn primary_refuses_without_a_fixed_price() {
    let (_w, _p, _b, r) = primary_with(None, |w, p| soroban_sdk::vec![&w.e, leg(&p.gallery, 1_000)]);
    assert_eq!(r, Err(AgentError::PriceNotSet));
}

/// Sem a assinatura do seller registrado, nenhuma perna pode ir para ele (nem para
/// fora dos payees): o dinheiro da primária só chega a quem o admin fixou.
#[test]
fn primary_refuses_a_leg_to_the_seller_or_outside_payees() {
    let (_w, _p, _b, r) = primary_with(Some(1), |w, _p| soroban_sdk::vec![&w.e, leg(&w.seller, 1_000)]);
    assert_eq!(r, Err(AgentError::PayeeNotRegistered));
    let (_w, _p, _b, r) = primary_with(Some(1), |w, _p| soroban_sdk::vec![&w.e, leg(&w.operator, 1_000)]);
    assert_eq!(r, Err(AgentError::PayeeNotRegistered));
}

#[test]
fn price_floor_rounds_up_for_the_seller() {
    assert_eq!(price_floor(12, UNIT / 2), Ok(6));
    assert_eq!(price_floor(1, 1), Ok(1), "1 stroop de título a 1 por unidade custa 1, não 0");
    assert_eq!(price_floor(i128::MAX, 2), Err(AgentError::InvalidAmount));
}

#[test]
fn set_primary_price_refusals() {
    let w = world();
    world_with_holder(&w);
    w.e.mock_all_auths();
    assert_eq!(client(&w).try_set_primary_price(&w.token, &0), Err(Ok(AgentError::InvalidAmount)));
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(client(&w).try_set_primary_price(&other, &1), Err(Ok(AgentError::NotRegistered)));
    client(&w).set_primary_price(&w.token, &7);
    assert_eq!(client(&w).get_primary_price(&w.token), Some(7));
}

#[test]
#[should_panic(expected = "Unauthorized")]
fn set_primary_price_requires_admin() {
    let w = world();
    world_with_holder(&w);
    // a chave quente tenta baratear o estoque
    w.e.mock_auths(&[MockAuth {
        address: &w.operator,
        invoke: &MockAuthInvoke {
            contract: &w.agent,
            fn_name: "set_primary_price",
            args: (w.token.clone(), 1i128).into_val(&w.e),
            sub_invokes: &[],
        },
    }]);
    client(&w).set_primary_price(&w.token, &1);
}

/// Ler é usar: registro e payees têm o TTL estendido quando lidos (não só quando escritos).
#[test]
fn reading_the_registration_and_payees_extends_their_ttl() {
    let w = world();
    phygital(&w);
    let ttl = |k: &DataKey| w.e.as_contract(&w.agent, || w.e.storage().persistent().get_ttl(k));
    let reg_key = DataKey::Token(w.token.clone());
    let pay_key = DataKey::Payees(w.token.clone());
    // envelhece as entradas até ficarem ABAIXO do limiar de extensão
    w.e.ledger().with_mut(|l| l.sequence_number += TTL_EXTEND - TTL_THRESHOLD + 100);
    let (reg_before, pay_before) = (ttl(&reg_key), ttl(&pay_key));
    assert!(reg_before < TTL_THRESHOLD && pay_before < TTL_THRESHOLD);
    // `pay` lê registro e payees
    let p_gallery = client(&w).get_payees(&w.token).get(0).unwrap();
    cash(&w, &w.buyer, 10);
    w.e.mock_all_auths();
    client(&w).pay(&w.token, &w.buyer, &w.usdc, &soroban_sdk::vec![&w.e, leg(&p_gallery, 1)], &1);
    assert_eq!(ttl(&reg_key), TTL_EXTEND, "registro estendido na leitura");
    assert_eq!(ttl(&pay_key), TTL_EXTEND, "payees estendidos na leitura");
}

#[test]
fn hand_over_forgets_payees_and_primary_price() {
    let w = world();
    phygital(&w);
    w.e.mock_all_auths();
    client(&w).set_primary_price(&w.token, &5);
    client(&w).hand_over_sac(&w.token, &w.agent);
    // re-registro do mesmo token não herda recebedores nem preço
    client(&w).register_token(&w.token, &w.seller, &w.settlement, &w.usdc);
    assert_eq!(client(&w).get_payees(&w.token).len(), 0);
    assert_eq!(client(&w).get_primary_price(&w.token), None);
}

// ══ v4 (06/10) — a versão única: o `transfer_dvp` volta COM o vendedor (PENDENCIAS 202) ═══
//
// `mock_auths` (e não `mock_all_auths`): cada teste diz EXATAMENTE quem assina, com os
// argumentos exatos. Uma parte ausente = a rede recusa.

const DVP_TOKENS: i128 = 100;
const DVP_PRICE: i128 = 900;

/// Os três `MockAuth` do `transfer_dvp`; `skip` tira os papéis que NÃO assinam. O comprador
/// autoriza a raiz E o `transfer` do USDC que ela chama (comprador → vendedor).
fn dvp_run(w: &World, seller: &Address, buyer: &Address, skip: &[&str]) -> bool {
    let args: soroban_sdk::Vec<soroban_sdk::Val> =
        (seller.clone(), buyer.clone(), w.token.clone(), DVP_TOKENS, w.usdc.clone(), DVP_PRICE).into_val(&w.e);
    let pay_args: soroban_sdk::Vec<soroban_sdk::Val> = (buyer.clone(), seller.clone(), DVP_PRICE).into_val(&w.e);
    let pay = [MockAuthInvoke { contract: &w.usdc, fn_name: "transfer", args: pay_args, sub_invokes: &[] }];
    let root = |subs| MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args: args.clone(), sub_invokes: subs };
    let (op_inv, seller_inv, buyer_inv) = (root(&[]), root(&[]), root(&pay));
    let mut auths = std::vec::Vec::new();
    if !skip.contains(&"operator") { auths.push(MockAuth { address: &w.operator, invoke: &op_inv }); }
    if !skip.contains(&"seller") { auths.push(MockAuth { address: seller, invoke: &seller_inv }); }
    if !skip.contains(&"buyer") { auths.push(MockAuth { address: buyer, invoke: &buyer_inv }); }
    w.e.mock_auths(&auths);
    client(w).try_transfer_dvp(seller, buyer, &w.token, &DVP_TOKENS, &w.usdc, &DVP_PRICE).is_ok()
}

/// w.buyer (holder, 400) revende ao buyer2 (que tem 5.000 de USDC).
fn dvp_world() -> (World, Address) {
    let w = world();
    world_with_holder(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 5_000);
    (w, buyer2)
}

#[test]
fn dvp_with_all_three_signatures_settles() {
    let (w, buyer2) = dvp_world();
    assert!(dvp_run(&w, &w.buyer, &buyer2, &[]), "operator + vendedor + comprador: a troca sai");
    assert_eq!(balance(&w, &w.buyer), 300, "o vendedor entregou o papel");
    assert_eq!(balance(&w, &buyer2), 100);
    assert!(!authorized(&w, &buyer2), "repouso do novo holder: NÃO autorizado");
    assert_eq!(usdc_of(&w, &buyer2), 4_100, "o comprador pagou");
    assert_eq!(usdc_of(&w, &w.buyer), 900, "o preço foi para o VENDEDOR, não para a plataforma");
    assert_eq!(balance(&w, &w.seller) + balance(&w, &w.buyer) + balance(&w, &buyer2), 1_000, "oferta conservada");
}

/// O caso do 202: operator + comprador, SEM o vendedor.
#[test]
fn dvp_requires_the_seller_signature() {
    let (w, buyer2) = dvp_world();
    assert!(!dvp_run(&w, &w.buyer, &buyer2, &["seller"]), "operator + comprador sem o vendedor: recusado");
    assert_eq!(balance(&w, &w.buyer), 400, "o título do vendedor não saiu");
    assert_eq!(balance(&w, &buyer2), 0);
    assert_eq!(usdc_of(&w, &buyer2), 5_000, "nem o dinheiro");
}

#[test]
fn dvp_requires_the_operator() {
    let (w, buyer2) = dvp_world();
    assert!(!dvp_run(&w, &w.buyer, &buyer2, &["operator"]), "sem a plataforma, nada");
    assert_eq!(balance(&w, &w.buyer), 400);
}

#[test]
fn dvp_requires_the_buyer_signature() {
    let (w, buyer2) = dvp_world();
    assert!(!dvp_run(&w, &w.buyer, &buyer2, &["buyer"]), "o comprador nunca autorizou o pagamento");
    assert_eq!(usdc_of(&w, &buyer2), 5_000);
}

/// A dispensa é só da primária do SPLIT: no `transfer_dvp` até o seller REGISTRADO assina.
#[test]
fn dvp_has_no_primary_waiver_even_for_the_registered_seller() {
    let w = world();
    world_with_holder(&w);
    let buyer2 = Address::generate(&w.e);
    cash(&w, &buyer2, 5_000);
    w.e.mock_all_auths();
    client(&w).set_primary_price(&w.token, &1);
    assert!(!dvp_run(&w, &w.seller, &buyer2, &["seller"]), "nem com preço fixado o transfer_dvp dispensa o seller");
    assert_eq!(balance(&w, &w.seller), 600);
    assert!(dvp_run(&w, &w.seller, &buyer2, &[]), "com as três assinaturas, passa");
    assert_eq!(balance(&w, &buyer2), 100);
}

/// O caso 3b da prova de 26/09 (tx `e22a4316…`): a chave quente do operator é TAMBÉM o
/// comprador e paga 1 stroop. Na v1 isso levava o saldo inteiro de um holder.
#[test]
fn dvp_operator_as_its_own_buyer_cannot_take_a_holder_balance() {
    let w = world();
    world_with_holder(&w);
    cash(&w, &w.operator, 1);
    let args: soroban_sdk::Vec<soroban_sdk::Val> =
        (w.buyer.clone(), w.operator.clone(), w.token.clone(), 400i128, w.usdc.clone(), 1i128).into_val(&w.e);
    let pay_args: soroban_sdk::Vec<soroban_sdk::Val> = (w.operator.clone(), w.buyer.clone(), 1i128).into_val(&w.e);
    let pay = [MockAuthInvoke { contract: &w.usdc, fn_name: "transfer", args: pay_args, sub_invokes: &[] }];
    // Duas entries da MESMA chave: uma como operator (raiz), outra como comprador (raiz + pagamento).
    let as_op = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args: args.clone(), sub_invokes: &[] };
    let as_buyer = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args, sub_invokes: &pay };
    w.e.mock_auths(&[
        MockAuth { address: &w.operator, invoke: &as_op },
        MockAuth { address: &w.operator, invoke: &as_buyer },
    ]);
    let r = client(&w).try_transfer_dvp(&w.buyer, &w.operator, &w.token, &400, &w.usdc, &1);
    assert!(r.is_err(), "o operator sozinho não move o saldo de ninguém");
    assert_eq!(balance(&w, &w.buyer), 400, "o holder ficou com tudo");
    assert_eq!(balance(&w, &w.operator), 0);
}

/// A assinatura do vendedor prende o PREÇO: autorizar 100 por 900 não vale para 100 por 1.
#[test]
fn dvp_seller_signature_does_not_cover_a_different_price() {
    let (w, buyer2) = dvp_world();
    let signed: soroban_sdk::Vec<soroban_sdk::Val> =
        (w.buyer.clone(), buyer2.clone(), w.token.clone(), DVP_TOKENS, w.usdc.clone(), DVP_PRICE).into_val(&w.e);
    let cheap: soroban_sdk::Vec<soroban_sdk::Val> =
        (w.buyer.clone(), buyer2.clone(), w.token.clone(), DVP_TOKENS, w.usdc.clone(), 1i128).into_val(&w.e);
    let pay_args: soroban_sdk::Vec<soroban_sdk::Val> = (buyer2.clone(), w.buyer.clone(), 1i128).into_val(&w.e);
    let pay = [MockAuthInvoke { contract: &w.usdc, fn_name: "transfer", args: pay_args, sub_invokes: &[] }];
    let op = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args: cheap.clone(), sub_invokes: &[] };
    let sel = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args: signed, sub_invokes: &[] };
    let buy = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp", args: cheap, sub_invokes: &pay };
    w.e.mock_auths(&[
        MockAuth { address: &w.operator, invoke: &op },
        MockAuth { address: &w.buyer, invoke: &sel },
        MockAuth { address: &buyer2, invoke: &buy },
    ]);
    let r = client(&w).try_transfer_dvp(&w.buyer, &buyer2, &w.token, &DVP_TOKENS, &w.usdc, &1);
    assert!(r.is_err(), "o vendedor assinou 900, não 1");
    assert_eq!(balance(&w, &w.buyer), 400);
}

#[test]
fn dvp_refusals_move_nothing() {
    let (w, buyer2) = dvp_world();
    w.e.mock_all_auths();
    let c = client(&w);
    assert_eq!(c.try_transfer_dvp(&w.buyer, &buyer2, &w.token, &0, &w.usdc, &900), Err(Ok(AgentError::InvalidAmount)));
    assert_eq!(c.try_transfer_dvp(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &0), Err(Ok(AgentError::InvalidAmount)), "DvP sem preço não é DvP");
    let other = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(c.try_transfer_dvp(&w.buyer, &buyer2, &other, &100, &w.usdc, &900), Err(Ok(AgentError::NotRegistered)));
    let fake = w.e.register_stellar_asset_contract_v2(Address::generate(&w.e)).address();
    assert_eq!(c.try_transfer_dvp(&w.buyer, &buyer2, &w.token, &100, &fake, &900), Err(Ok(AgentError::WrongPaymentAsset)));
    assert_eq!(c.try_transfer_dvp(&w.buyer, &buyer2, &w.token, &401, &w.usdc, &900), Err(Ok(AgentError::InsufficientBalance)), "o vendedor tem 400");
    // sem dinheiro: a perna do USDC derruba a invocação inteira
    let poor = Address::generate(&w.e);
    cash(&w, &poor, 10);
    w.e.mock_all_auths();
    assert!(c.try_transfer_dvp(&w.buyer, &poor, &w.token, &100, &w.usdc, &900).is_err());
    assert_eq!(balance(&w, &w.buyer), 400, "nenhuma recusa moveu o título");
    assert_eq!(usdc_of(&w, &buyer2), 5_000, "nem o dinheiro");
    assert_eq!(usdc_of(&w, &poor), 10);
}

/// O split: ter preço de primária fixado NÃO dispensa o vendedor numa REVENDA (vendedor ≠
/// registrado). Operator + comprador sem o vendedor → recusado; com os três → liquida.
#[test]
fn split_resale_needs_the_seller_even_when_a_primary_price_is_set() {
    for with_seller in [false, true] {
        let (w, p, buyer2) = split_refusal_world();
        w.e.mock_all_auths();
        client(&w).set_primary_price(&w.token, &1);
        let legs = soroban_sdk::vec![&w.e, leg(&p.gallery, 50), leg(&w.buyer, 50)];
        let args: soroban_sdk::Vec<soroban_sdk::Val> =
            (w.buyer.clone(), buyer2.clone(), w.token.clone(), 100i128, w.usdc.clone(), legs.clone(), 9u64).into_val(&w.e);
        let subs = [
            MockAuthInvoke { contract: &w.usdc, fn_name: "transfer", args: (buyer2.clone(), p.gallery.clone(), 50i128).into_val(&w.e), sub_invokes: &[] },
            MockAuthInvoke { contract: &w.usdc, fn_name: "transfer", args: (buyer2.clone(), w.buyer.clone(), 50i128).into_val(&w.e), sub_invokes: &[] },
        ];
        let plain = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args: args.clone(), sub_invokes: &[] };
        let paying = MockAuthInvoke { contract: &w.agent, fn_name: "transfer_dvp_split", args, sub_invokes: &subs };
        let mut auths = std::vec![MockAuth { address: &w.operator, invoke: &plain }, MockAuth { address: &buyer2, invoke: &paying }];
        if with_seller {
            auths.push(MockAuth { address: &w.buyer, invoke: &plain });
        }
        w.e.mock_auths(&auths);
        let r = client(&w).try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &100, &w.usdc, &legs, &9);
        if with_seller {
            assert!(r.is_ok(), "com o vendedor, a revenda liquida");
            assert_eq!(balance(&w, &buyer2), 100);
        } else {
            assert!(r.is_err(), "sem o vendedor, a revenda é recusada mesmo com preço de primária");
            assert_eq!(balance(&w, &w.buyer), 400);
            assert_eq!(balance(&w, &buyer2), 0);
        }
    }
}

// ══ v5 (F6) — emergency_pauser e a pausa do agente ═════════════════════════
//
// A pausa nasce AQUI (o agente não tinha). O que ela bloqueia é a lista fechada de funções que
// movem saldo do título ou dinheiro; o que continua aberto é o que o admin precisa para sair
// do problema (resume, upgrade, rotação de admin, hand_over_sac). O pauser só liga a pausa.

mod emergency_pauser_tests {
    use super::*;
    use soroban_sdk::testutils::Events as _;
    use soroban_sdk::{BytesN, Val};

    /// Autoriza SÓ `who` para a próxima chamada de `fn_name` — as demais exigências de auth falham.
    fn only(w: &World, who: &Address, fn_name: &str, args: soroban_sdk::Vec<Val>) {
        w.e.mock_auths(&[MockAuth {
            address: who,
            invoke: &MockAuthInvoke { contract: &w.agent, fn_name, args, sub_invokes: &[] },
        }]);
    }

    fn some_arg(w: &World, a: &Address) -> soroban_sdk::Vec<Val> {
        soroban_sdk::vec![&w.e, Some(a.clone()).into_val(&w.e)]
    }

    fn none() -> Option<Address> {
        None
    }

    /// Erro DO CONTRATO (ou Ok). Falha de invocação (auth, panic do host) derruba o teste:
    /// aqui interessa saber QUAL erro de contrato voltou.
    fn res<T, C>(r: Result<Result<T, C>, Result<AgentError, soroban_sdk::InvokeError>>) -> Result<(), AgentError> {
        match r {
            Ok(Ok(_)) => Ok(()),
            Err(Ok(e)) => Err(e),
            _ => panic!("falha de invocacao, nao erro do contrato"),
        }
    }

    /// Mundo completo: título registrado, 400 na mão do `w.buyer`, payees, USDC para o comprador
    /// e para o `buyer2`. Devolve (payees, buyer2).
    fn scenario(w: &World) -> (Payees, Address) {
        let p = phygital(w);
        let buyer2 = Address::generate(&w.e);
        cash(w, &buyer2, 5_000);
        cash(w, &w.buyer, 1_000);
        (p, buyer2)
    }

    fn with_pauser(w: &World) -> Address {
        let pauser = Address::generate(&w.e);
        w.e.mock_all_auths();
        client(w).set_emergency_pauser(&Some(pauser.clone()));
        w.e.set_auths(&[]);
        pauser
    }

    fn client_emergency(w: &World, pauser: &Address) {
        only(w, pauser, "emergency_pause", soroban_sdk::vec![&w.e]);
        client(w).emergency_pause();
    }

    /// As SETE funções que a pausa bloqueia (movimentos normais; clawback e congelar ficam de fora, de propósito), com argumentos VÁLIDOS (cada uma liquida quando
    /// não há pausa), todas sob `mock_all_auths` — assim o único motivo de recusa possível é a pausa.
    fn run_all(w: &World, p: &Payees, buyer2: &Address) -> std::vec::Vec<(&'static str, Result<(), AgentError>)> {
        w.e.mock_all_auths();
        let c = client(w);
        let one = soroban_sdk::vec![&w.e, leg(&p.gallery, 10)];
        let split = soroban_sdk::vec![&w.e, leg(&p.gallery, 5), leg(&w.buyer, 5)];
        let out = std::vec![
            ("deliver", res(c.try_deliver(&w.token, buyer2, &10))),
            ("mint", res(c.try_mint(&w.token, &w.seller, &5))),
            ("pay", res(c.try_pay(&w.token, &w.buyer, &w.usdc, &one, &1))),
            ("transfer_dvp", res(c.try_transfer_dvp(&w.buyer, buyer2, &w.token, &10, &w.usdc, &20))),
            ("transfer_dvp_split", res(c.try_transfer_dvp_split(&w.buyer, buyer2, &w.token, &10, &w.usdc, &split, &2))),
            ("redeem", res(c.try_redeem(&w.token, &w.buyer, &10, &1))),
            ("admin_transfer", res(c.try_admin_transfer(&w.token, &w.buyer, &one, &1))),
        ];
        w.e.set_auths(&[]);
        out
    }

    /// Foto do dinheiro e do título de todo mundo que as sete funções tocam.
    fn snapshot(w: &World, p: &Payees, buyer2: &Address) -> std::vec::Vec<i128> {
        std::vec![
            balance(w, &w.seller),
            balance(w, &w.buyer),
            balance(w, buyer2),
            balance(w, &p.gallery),
            usdc_of(w, &w.buyer),
            usdc_of(w, buyer2),
            usdc_of(w, &p.gallery),
        ]
    }

    #[test]
    fn so_o_admin_define_o_pauser() {
        let w = world();
        let novo = Address::generate(&w.e);
        only(&w, &w.admin, "set_emergency_pauser", soroban_sdk::vec![&w.e, Some(novo.clone()).into_val(&w.e)]);
        client(&w).set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(w.e.auths()[0].0, w.admin);
        assert_eq!(client(&w).emergency_pauser(), Some(novo.clone()));

        // o operator (chave quente) e um terceiro não definem; o valor segue igual
        for intruso in [w.operator.clone(), Address::generate(&w.e)] {
            let outro = Address::generate(&w.e);
            only(&w, &intruso, "set_emergency_pauser", some_arg(&w, &outro));
            assert!(client(&w).try_set_emergency_pauser(&Some(outro)).is_err());
            assert_eq!(client(&w).emergency_pauser(), Some(novo.clone()));
        }

        // o próprio pauser não se promove nem escolhe o sucessor
        let sucessor = Address::generate(&w.e);
        only(&w, &novo, "set_emergency_pauser", some_arg(&w, &sucessor));
        assert!(client(&w).try_set_emergency_pauser(&Some(sucessor)).is_err());
        assert_eq!(client(&w).emergency_pauser(), Some(novo));
    }

    #[test]
    fn emergency_pause_sem_pauser_configurado_e_recusada() {
        let w = world();
        w.e.mock_all_auths();
        assert_eq!(client(&w).emergency_pauser(), None);
        assert_eq!(client(&w).try_emergency_pause(), Err(Ok(AgentError::NoEmergencyPauser)));
        assert!(!client(&w).get_paused());
    }

    #[test]
    fn terceiro_operator_e_admin_nao_ligam_emergency_pause() {
        let w = world();
        with_pauser(&w);
        for quem in [Address::generate(&w.e), w.operator.clone(), w.admin.clone()] {
            only(&w, &quem, "emergency_pause", soroban_sdk::vec![&w.e]);
            assert!(client(&w).try_emergency_pause().is_err());
            assert!(!client(&w).get_paused());
        }
    }

    /// O coração da prova: pausado, as sete funções devolvem `Paused` e NADA se move; as
    /// abertas continuam; depois do `resume` as mesmas sete chamadas liquidam.
    #[test]
    fn pausa_bloqueia_exatamente_o_que_move_saldo() {
        let w = world();
        let (p, buyer2) = scenario(&w);
        let pauser = with_pauser(&w);
        let antes = snapshot(&w, &p, &buyer2);

        client_emergency(&w, &pauser);
        assert_eq!(w.e.auths()[0].0, pauser, "quem assina é o pauser");
        assert!(client(&w).get_paused());

        for (nome, r) in run_all(&w, &p, &buyer2) {
            assert_eq!(r, Err(AgentError::Paused), "{nome} tem que parar na pausa");
        }
        assert_eq!(snapshot(&w, &p, &buyer2), antes, "pausar e recusar não movem nada");

        // o que continua aberto sob a pausa (admin sob mock_all_auths)
        w.e.mock_all_auths();
        let c = client(&w);
        c.set_authorized(&w.token, &buyer2, &true);
        c.admin_set_authorized(&w.token, &buyer2, &false);
        c.set_payees(&w.token, &soroban_sdk::vec![&w.e, p.gallery.clone(), p.radox.clone()]);
        c.set_primary_price(&w.token, &1_000);
        c.update_token(&w.token, &w.seller, &w.settlement, &w.usdc);
        c.set_operator(&w.operator);
        let admin2 = Address::generate(&w.e);
        c.propose_admin(&admin2);
        c.accept_admin();
        assert_eq!(c.get_config().admin, admin2, "a rotação de admin segue aberta sob a pausa");
        assert!(c.get_paused(), "nada disso despausou");
        assert_eq!(c.get_payees(&w.token).len(), 2);

        // o admin despausa; as mesmas sete chamadas agora liquidam
        c.resume();
        w.e.set_auths(&[]);
        assert!(!client(&w).get_paused());
        for (nome, r) in run_all(&w, &p, &buyer2) {
            assert_eq!(r, Ok(()), "{nome} volta a rodar depois do resume");
        }
        assert_ne!(snapshot(&w, &p, &buyer2), antes);
    }

    /// Atos de CONFORMIDADE seguem sob a pausa (ordem judicial, sanção, exploit): `clawback` (settlement
    /// registrado), `admin_clawback` (admin) e congelar (`set_authorized` do operator,
    /// `admin_set_authorized` do admin) rodam com o agente pausado, cada um só pelo signatário certo.
    /// O pauser, que só liga a pausa, não faz nenhum deles.
    #[test]
    fn conformidade_segue_sob_a_pausa() {
        let w = world();
        let (_p, _buyer2) = scenario(&w);
        let pauser = with_pauser(&w);
        client_emergency(&w, &pauser);
        assert!(client(&w).get_paused());

        let antes = balance(&w, &w.buyer);
        // o pauser NÃO faz nenhum dos quatro (sob a pausa, a recusa é de AUTH, não de Paused)
        only(&w, &pauser, "clawback", (w.token.clone(), w.buyer.clone(), 5i128).into_val(&w.e));
        assert!(client(&w).try_clawback(&w.token, &w.buyer, &5).is_err());
        only(&w, &pauser, "admin_clawback", (w.token.clone(), w.buyer.clone(), 5i128).into_val(&w.e));
        assert!(client(&w).try_admin_clawback(&w.token, &w.buyer, &5).is_err());
        only(&w, &pauser, "set_authorized", (w.token.clone(), w.buyer.clone(), false).into_val(&w.e));
        assert!(client(&w).try_set_authorized(&w.token, &w.buyer, &false).is_err());
        only(&w, &pauser, "admin_set_authorized", (w.token.clone(), w.buyer.clone(), false).into_val(&w.e));
        assert!(client(&w).try_admin_set_authorized(&w.token, &w.buyer, &false).is_err());
        assert_eq!(balance(&w, &w.buyer), antes, "o pauser não moveu nada");

        // o signatário certo faz, pausado
        only(&w, &w.settlement, "clawback", (w.token.clone(), w.buyer.clone(), 5i128).into_val(&w.e));
        assert_eq!(res(client(&w).try_clawback(&w.token, &w.buyer, &5)), Ok(()), "clawback do settlement sob a pausa");
        only(&w, &w.admin, "admin_clawback", (w.token.clone(), w.buyer.clone(), 7i128).into_val(&w.e));
        assert_eq!(res(client(&w).try_admin_clawback(&w.token, &w.buyer, &7)), Ok(()), "admin_clawback sob a pausa");
        assert_eq!(balance(&w, &w.buyer), antes - 12, "os dois clawbacks tiraram 5 + 7");
        only(&w, &w.operator, "set_authorized", (w.token.clone(), w.buyer.clone(), true).into_val(&w.e));
        client(&w).set_authorized(&w.token, &w.buyer, &true);
        only(&w, &w.admin, "admin_set_authorized", (w.token.clone(), w.buyer.clone(), false).into_val(&w.e));
        client(&w).admin_set_authorized(&w.token, &w.buyer, &false);
        assert!(client(&w).get_paused(), "nada disso despausou");

        // e o movimento normal continua parado
        w.e.mock_all_auths();
        assert_eq!(res(client(&w).try_mint(&w.token, &w.seller, &5)), Err(AgentError::Paused));
    }

    /// Pausado, `hand_over_sac` (a porta de saída do agente) ainda abre.
    #[test]
    fn hand_over_sac_segue_aberto_sob_a_pausa() {
        let w = world();
        scenario(&w);
        let pauser = with_pauser(&w);
        client_emergency(&w, &pauser);
        let saida = Address::generate(&w.e);
        w.e.mock_all_auths();
        client(&w).hand_over_sac(&w.token, &saida);
        assert!(!client(&w).is_registered(&w.token));
        assert!(client(&w).get_paused());
    }

    /// `pause`/`resume` do admin: só ele; o pauser e o operator não despausam.
    #[test]
    fn pause_e_resume_sao_do_admin() {
        let w = world();
        let pauser = with_pauser(&w);
        for quem in [pauser.clone(), w.operator.clone(), Address::generate(&w.e)] {
            only(&w, &quem, "pause", soroban_sdk::vec![&w.e]);
            assert!(client(&w).try_pause().is_err(), "só o admin pausa por pause()");
            assert!(!client(&w).get_paused());
        }
        only(&w, &w.admin, "pause", soroban_sdk::vec![&w.e]);
        client(&w).pause();
        assert!(client(&w).get_paused());
        for quem in [pauser, w.operator.clone(), Address::generate(&w.e)] {
            only(&w, &quem, "resume", soroban_sdk::vec![&w.e]);
            assert!(client(&w).try_resume().is_err(), "só o admin despausa");
            assert!(client(&w).get_paused());
        }
        only(&w, &w.admin, "resume", soroban_sdk::vec![&w.e]);
        client(&w).resume();
        assert!(!client(&w).get_paused());
    }

    /// O pauser, com a pausa ligada ou não, não faz NADA além de pausar: não despausa, não
    /// move saldo nem dinheiro, não troca admin/operator/pauser, não mexe em registro, payees
    /// ou preço, não faz upgrade. Cada chamada é o ÚNICO auth do pauser; os controles positivos
    /// (mesma chamada, signatário certo) provam que o formato do argumento está certo.
    #[test]
    fn pauser_nao_faz_nada_alem_de_pausar() {
        let w = world();
        let (p, buyer2) = scenario(&w);
        let pauser = with_pauser(&w);

        // controles: com o signatário certo e os MESMOS argumentos, a chamada confirma
        let um = soroban_sdk::vec![&w.e, leg(&p.gallery, 10)];
        only(&w, &w.admin, "mint", (w.token.clone(), w.seller.clone(), 5i128).into_val(&w.e));
        assert_eq!(res(client(&w).try_mint(&w.token, &w.seller, &5)), Ok(()));
        only(&w, &w.admin, "admin_clawback", (w.token.clone(), w.seller.clone(), 5i128).into_val(&w.e));
        assert_eq!(res(client(&w).try_admin_clawback(&w.token, &w.seller, &5)), Ok(()));
        only(&w, &w.admin, "admin_transfer", (w.token.clone(), w.buyer.clone(), um.clone(), 1u64).into_val(&w.e));
        assert_eq!(res(client(&w).try_admin_transfer(&w.token, &w.buyer, &um, &1)), Ok(()));
        let op2 = Address::generate(&w.e);
        only(&w, &w.admin, "set_operator", (op2.clone(),).into_val(&w.e));
        client(&w).set_operator(&op2);
        only(&w, &w.admin, "set_operator", (w.operator.clone(),).into_val(&w.e));
        client(&w).set_operator(&w.operator);

        for pausado in [false, true] {
            if pausado {
                client_emergency(&w, &pauser);
            }
            let antes = snapshot(&w, &p, &buyer2);
            let cfg = client(&w).get_config();
            macro_rules! nega {
                ($fn:literal, $args:expr, $call:expr) => {{
                    only(&w, &pauser, $fn, $args.into_val(&w.e));
                    assert!($call.is_err(), concat!("o pauser NAO pode chamar ", $fn));
                }};
            }
            let split = soroban_sdk::vec![&w.e, leg(&p.gallery, 5), leg(&w.buyer, 5)];
            let hash = BytesN::from_array(&w.e, &[7u8; 32]);
            let alvo = Address::generate(&w.e);
            let c = client(&w);
            only(&w, &pauser, "resume", soroban_sdk::vec![&w.e]);
            assert!(c.try_resume().is_err(), "o pauser NAO despausa");
            only(&w, &pauser, "pause", soroban_sdk::vec![&w.e]);
            assert!(c.try_pause().is_err(), "o pauser NAO pausa por pause()");
            nega!("propose_admin", (alvo.clone(),), c.try_propose_admin(&alvo));
            nega!("set_operator", (alvo.clone(),), c.try_set_operator(&alvo));
            nega!("upgrade", (hash.clone(),), c.try_upgrade(&hash));
            nega!("hand_over_sac", (w.token.clone(), alvo.clone()), c.try_hand_over_sac(&w.token, &alvo));
            nega!(
                "register_token",
                (alvo.clone(), alvo.clone(), alvo.clone(), w.usdc.clone()),
                c.try_register_token(&alvo, &alvo, &alvo, &w.usdc)
            );
            nega!(
                "update_token",
                (w.token.clone(), alvo.clone(), alvo.clone(), w.usdc.clone()),
                c.try_update_token(&w.token, &alvo, &alvo, &w.usdc)
            );
            nega!(
                "set_payees",
                (w.token.clone(), soroban_sdk::vec![&w.e, alvo.clone()]),
                c.try_set_payees(&w.token, &soroban_sdk::vec![&w.e, alvo.clone()])
            );
            nega!("set_primary_price", (w.token.clone(), 1i128), c.try_set_primary_price(&w.token, &1));
            nega!("mint", (w.token.clone(), alvo.clone(), 5i128), c.try_mint(&w.token, &alvo, &5));
            nega!("clawback", (w.token.clone(), w.buyer.clone(), 5i128), c.try_clawback(&w.token, &w.buyer, &5));
            nega!(
                "admin_clawback",
                (w.token.clone(), w.buyer.clone(), 5i128),
                c.try_admin_clawback(&w.token, &w.buyer, &5)
            );
            nega!(
                "admin_set_authorized",
                (w.token.clone(), alvo.clone(), true),
                c.try_admin_set_authorized(&w.token, &alvo, &true)
            );
            nega!(
                "set_authorized",
                (w.token.clone(), alvo.clone(), true),
                c.try_set_authorized(&w.token, &alvo, &true)
            );
            nega!("deliver", (w.token.clone(), alvo.clone(), 5i128), c.try_deliver(&w.token, &alvo, &5));
            nega!(
                "admin_transfer",
                (w.token.clone(), w.buyer.clone(), um.clone(), 1u64),
                c.try_admin_transfer(&w.token, &w.buyer, &um, &1)
            );
            nega!("redeem", (w.token.clone(), w.buyer.clone(), 5i128, 1u64), c.try_redeem(&w.token, &w.buyer, &5, &1));
            nega!(
                "pay",
                (w.token.clone(), w.buyer.clone(), w.usdc.clone(), um.clone(), 1u64),
                c.try_pay(&w.token, &w.buyer, &w.usdc, &um, &1)
            );
            nega!(
                "transfer_dvp",
                (w.buyer.clone(), buyer2.clone(), w.token.clone(), 10i128, w.usdc.clone(), 20i128),
                c.try_transfer_dvp(&w.buyer, &buyer2, &w.token, &10, &w.usdc, &20)
            );
            nega!(
                "transfer_dvp_split",
                (w.buyer.clone(), buyer2.clone(), w.token.clone(), 10i128, w.usdc.clone(), split.clone(), 2u64),
                c.try_transfer_dvp_split(&w.buyer, &buyer2, &w.token, &10, &w.usdc, &split, &2)
            );
            // nem se troca nem escolhe sucessor
            nega!("set_emergency_pauser", (Some(alvo.clone()),), c.try_set_emergency_pauser(&Some(alvo.clone())));

            assert_eq!(client(&w).get_paused(), pausado, "o pauser nao mudou a pausa");
            assert_eq!(client(&w).get_config().admin, cfg.admin);
            assert_eq!(client(&w).get_config().operator, cfg.operator);
            assert_eq!(client(&w).emergency_pauser(), Some(pauser.clone()));
            assert_eq!(snapshot(&w, &p, &buyer2), antes, "nenhuma recusa moveu algo");
            assert!(client(&w).is_registered(&w.token));
        }
    }

    #[test]
    fn trocar_ou_remover_o_pauser_invalida_o_antigo() {
        let w = world();
        let antigo = with_pauser(&w);
        let novo = Address::generate(&w.e);
        w.e.mock_all_auths();
        client(&w).set_emergency_pauser(&Some(novo.clone()));
        assert_eq!(client(&w).emergency_pauser(), Some(novo.clone()));

        only(&w, &antigo, "emergency_pause", soroban_sdk::vec![&w.e]);
        assert!(client(&w).try_emergency_pause().is_err(), "o antigo não pausa mais");
        assert!(!client(&w).get_paused());

        client_emergency(&w, &novo);
        assert!(client(&w).get_paused());

        w.e.mock_all_auths();
        client(&w).resume();
        client(&w).set_emergency_pauser(&None);
        assert_eq!(client(&w).emergency_pauser(), None);
        assert_eq!(client(&w).try_emergency_pause(), Err(Ok(AgentError::NoEmergencyPauser)));
    }

    #[test]
    fn emergency_pause_e_idempotente() {
        let w = world();
        with_pauser(&w);
        w.e.mock_all_auths();
        client(&w).emergency_pause();
        client(&w).emergency_pause();
        assert!(client(&w).get_paused());
    }

    /// Instância "antiga" (v4): nasce SEM as chaves novas. Tudo segue funcionando — inclusive
    /// o DvP de três assinaturas — e o pauser entra e sai sem mexer no layout antigo.
    #[test]
    fn instancia_antiga_sem_as_chaves_continua_funcionando() {
        let (w, buyer2) = dvp_world();
        w.e.as_contract(&w.agent, || {
            assert!(!w.e.storage().instance().has(&DataKey::EmergencyPauser));
            assert!(!w.e.storage().instance().has(&DataKey::Paused));
        });
        assert_eq!(client(&w).emergency_pauser(), None);
        assert!(!client(&w).get_paused(), "sem a chave, não está pausado");

        // fluxo v4 inteiro, sem pauser: o DvP de operator + vendedor + comprador liquida
        assert!(dvp_run(&w, &w.buyer, &buyer2, &[]));
        assert_eq!(balance(&w, &buyer2), 100);

        // o admin pausa e despausa por pause()/resume() — a instância antiga ganha só a chave Paused
        w.e.mock_all_auths();
        client(&w).pause();
        assert_eq!(res(client(&w).try_mint(&w.token, &w.seller, &1)), Err(AgentError::Paused));
        client(&w).resume();
        assert_eq!(res(client(&w).try_mint(&w.token, &w.seller, &1)), Ok(()));
        let novo_admin = Address::generate(&w.e);
        client(&w).propose_admin(&novo_admin);
        client(&w).accept_admin();
        assert_eq!(client(&w).get_config().admin, novo_admin);

        // entrar e sair com a chave nova não mexe nas antigas
        client(&w).set_emergency_pauser(&Some(w.operator.clone()));
        assert_eq!(client(&w).get_config().admin, novo_admin);
        client(&w).set_emergency_pauser(&None);
        w.e.as_contract(&w.agent, || {
            assert!(!w.e.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert_eq!(client(&w).version(), 5);
    }

    // ─── renúncia + eventos de troca ────────────────────────────────────────

    /// Evento esperado do contrato: (contrato, [tópico], dado).
    fn ev<D: IntoVal<Env, Val>>(
        w: &World,
        topic: soroban_sdk::Symbol,
        data: D,
    ) -> (Address, soroban_sdk::Vec<Val>, Val) {
        (w.agent.clone(), (topic,).into_val(&w.e), data.into_val(&w.e))
    }

    /// Eventos do contrato na ÚLTIMA chamada.
    fn do_contrato(w: &World) -> soroban_sdk::testutils::ContractEvents {
        w.e.events().all().filter_by_contract(&w.agent)
    }

    #[test]
    fn renuncia_so_pelo_proprio_pauser() {
        let w = world();
        let pauser = with_pauser(&w);

        // terceiro e o admin não renunciam pelo pauser (o admin remove com set_emergency_pauser(None))
        for quem in [Address::generate(&w.e), w.admin.clone(), w.operator.clone()] {
            only(&w, &quem, "renounce_emergency_pauser", soroban_sdk::vec![&w.e]);
            assert!(client(&w).try_renounce_emergency_pauser().is_err());
            assert_eq!(client(&w).emergency_pauser(), Some(pauser.clone()));
        }

        only(&w, &pauser, "renounce_emergency_pauser", soroban_sdk::vec![&w.e]);
        client(&w).renounce_emergency_pauser();
        assert_eq!(w.e.auths()[0].0, pauser, "quem assina é o pauser");
        assert_eq!(client(&w).emergency_pauser(), None);
        w.e.as_contract(&w.agent, || {
            assert!(!w.e.storage().instance().has(&DataKey::EmergencyPauser));
        });
        assert!(!client(&w).get_paused(), "renunciar não liga pausa");
    }

    #[test]
    fn depois_da_renuncia_emergency_pause_cai_e_o_admin_define_de_novo() {
        let w = world();
        let pauser = with_pauser(&w);
        only(&w, &pauser, "renounce_emergency_pauser", soroban_sdk::vec![&w.e]);
        client(&w).renounce_emergency_pauser();

        w.e.mock_all_auths();
        assert_eq!(client(&w).try_emergency_pause(), Err(Ok(AgentError::NoEmergencyPauser)));
        assert_eq!(client(&w).try_renounce_emergency_pauser(), Err(Ok(AgentError::NoEmergencyPauser)));

        let novo = Address::generate(&w.e);
        client(&w).set_emergency_pauser(&Some(novo.clone()));
        client_emergency(&w, &novo);
        assert!(client(&w).get_paused());

        // o pauser que renunciou não volta sozinho
        w.e.mock_all_auths();
        client(&w).resume();
        only(&w, &pauser, "emergency_pause", soroban_sdk::vec![&w.e]);
        assert!(client(&w).try_emergency_pause().is_err());
        assert!(!client(&w).get_paused());
    }

    #[test]
    fn toda_troca_emite_pauserset_com_anterior_e_novo() {
        let w = world();
        let a = with_pauser(&w);
        let b = Address::generate(&w.e);
        let topic = soroban_sdk::symbol_short!("pauserset");
        w.e.mock_all_auths();

        client(&w).set_emergency_pauser(&Some(b.clone()));
        assert_eq!(
            do_contrato(&w),
            soroban_sdk::vec![&w.e, ev(&w, topic.clone(), (Some(a.clone()), Some(b.clone())))]
        );

        client(&w).set_emergency_pauser(&None);
        assert_eq!(
            do_contrato(&w),
            soroban_sdk::vec![&w.e, ev(&w, topic.clone(), (Some(b.clone()), none()))]
        );

        client(&w).set_emergency_pauser(&Some(a.clone()));
        assert_eq!(
            do_contrato(&w),
            soroban_sdk::vec![&w.e, ev(&w, topic.clone(), (none(), Some(a.clone())))]
        );

        // renunciar: pauserset (a -> nada) e pauserren (a), nessa ordem
        client(&w).renounce_emergency_pauser();
        assert_eq!(
            do_contrato(&w),
            soroban_sdk::vec![
                &w.e,
                ev(&w, topic, (Some(a.clone()), none())),
                ev(&w, soroban_sdk::symbol_short!("pauserren"), a),
            ]
        );
    }

    #[test]
    fn eventos_de_pausa_e_despausa() {
        let w = world();
        let pauser = with_pauser(&w);
        w.e.mock_all_auths();
        client(&w).emergency_pause();
        assert_eq!(
            do_contrato(&w),
            soroban_sdk::vec![
                &w.e,
                ev(&w, soroban_sdk::symbol_short!("paused"), true),
                ev(&w, soroban_sdk::symbol_short!("emergpaus"), pauser),
            ]
        );
        client(&w).resume();
        assert_eq!(do_contrato(&w), soroban_sdk::vec![&w.e, ev(&w, soroban_sdk::symbol_short!("resumed"), true)]);
        client(&w).pause();
        assert_eq!(do_contrato(&w), soroban_sdk::vec![&w.e, ev(&w, soroban_sdk::symbol_short!("paused"), true)]);
    }

    #[test]
    fn trocar_o_admin_nao_zera_o_pauser() {
        let w = world();
        let pauser = with_pauser(&w);
        let novo_admin = Address::generate(&w.e);
        w.e.mock_all_auths();
        client(&w).propose_admin(&novo_admin);
        client(&w).accept_admin();
        assert_eq!(client(&w).get_config().admin, novo_admin);
        assert_eq!(client(&w).emergency_pauser(), Some(pauser));
        client(&w).emergency_pause();
        assert!(client(&w).get_paused());
    }

    /// Pausado, o `deliver` chamado PELO contrato de venda (o caminho real) também para.
    #[test]
    fn deliver_pelo_contrato_de_venda_para_na_pausa() {
        let w = world();
        register_and_fund(&w, 100);
        let pauser = with_pauser(&w);
        client_emergency(&w, &pauser);
        w.e.mock_all_auths();
        let r = SellerStubClient::new(&w.e, &w.seller).try_do_deliver(&w.agent, &w.token, &w.buyer, &10);
        assert!(r.is_err());
        assert_eq!(balance(&w, &w.buyer), 0);
        assert_eq!(balance(&w, &w.seller), 100);
    }
}
