#![no_std]
//! Agente de transferência (F3, 24/08) — o SAC-admin institucional da plataforma.
//!
//! Papéis: `admin` no storage = `contract_admin`
//! (inclui `admin_transfer`); `operator` no storage = `platform_operator`.
//!
//! Modelo A2 da revisão de arquitetura (CONTRACTS_DEEP_RESEARCH_2026-08-23 §4):
//! o admin dos SACs deixa de ser a chave da emissora e passa a ser ESTE contrato,
//! com papéis escopados — e o exemplo `sac_admin_wrapper` da OZ é adaptado, não
//! copiado (lá o `manager` tem clawback; aqui clawback é só admin/settlement).
//!
//!   • admin (conta G 2-de-3): mint, clawback, set_authorized, registro de
//!     tokens, rotação em 2 passos, upgrade.
//!   • operator (G de operações): set_authorized do dia a dia (gate de KYC).
//!   • seller registrado POR TOKEN (contrato de venda): `deliver(to, amount)` —
//!     authorize → transfer → revoke NA MESMA invocação (SEP-8 em Soroban).
//!     Estado de repouso de todo holder: NÃO autorizado.
//!   • settlement registrado POR TOKEN: `clawback` (resgate/vencimento v4).
//!
//! v2 (03/10, phygital — docs/Tecnologia/PHYGITAL_DESIGN.md §1.2), ADITIVA sobre a v1,
//! com UMA retirada: o `transfer_dvp` da v1 saiu. Ele trocava o título de qualquer
//! holder com a assinatura do operator e de um comprador, sem o vendedor, por qualquer
//! preço > 0 — chave quente roubada + comprador cúmplice levavam o título por 1 stroop.
//! O `transfer_dvp_split` o substitui por inteiro (uma perna `{seller, preço}` é o DvP
//! da v1) e exige o vendedor. Entraram:
//!   • `set_payees` (admin): quem pode RECEBER dinheiro nas trocas de cada token;
//!   • `pay` (payer): pagamento em pernas, só para payees;
//!   • `transfer_dvp_split` (operator + seller + buyer): DvP com o dinheiro em pernas;
//!   • `redeem` (operator + holder): queima com o consentimento do holder;
//!   • `admin_transfer` (admin): clawback de um holder e mint em pernas, oferta conservada.
//!
//! v4 (06/10), a versão ÚNICA: a v2.1 do phygital + o conserto da PENDENCIAS 202. O
//! `transfer_dvp` VOLTA, agora com o vendedor assinando (operator + seller + buyer): a
//! dispensa da assinatura do vendedor vale SÓ na primária do `transfer_dvp_split`
//! (`seller == reg.seller` + preço fixado pelo admin); em toda revenda entre holders o
//! vendedor assina, nas duas funções.
//!
//! v5 (06/10): papel `emergency_pauser` SEPARADO do admin e uma PAUSA mínima
//! que o agente não tinha. O pauser só LIGA a pausa (`emergency_pause`); não despausa (`resume` é
//! do admin), não move saldo nem dinheiro, não troca admin/operator, não faz upgrade. O admin
//! define, troca e remove (`set_emergency_pauser`); o próprio pauser renuncia
//! (`renounce_emergency_pauser`); toda troca emite `pauserset` (anterior, novo) e a renúncia
//! também `pauserren`. Trocar o admin NÃO zera o pauser. Mesmos nomes e mesmo desenho do
//! `maturity_settlement`/`yield_distributor` v5. Chaves de storage NOVAS e opcionais
//! (`Paused`, `EmergencyPauser`): instância v4 sem elas segue funcionando (ausente = não
//! pausado, sem pauser) e vira v5 por `upgrade`, sem migração.
//!
//! O QUE A PAUSA BLOQUEIA (`Paused` = 13): os MOVIMENTOS NORMAIS — `deliver`, `mint`,
//! `transfer_dvp`, `transfer_dvp_split`, `pay`, `redeem`, `admin_transfer` (sete).
//! CONTINUAM abertas, de propósito, os ATOS DE CONFORMIDADE: `clawback` e `admin_clawback`
//! (ordem judicial, sanção, exploit) e `set_authorized`/`admin_set_authorized` (congelar um
//! holder não pode esperar a pausa acabar) — são o remédio da emergência. Também abertas:
//! `resume`, `upgrade`, `propose_admin`/`accept_admin`, `set_operator`, `set_emergency_pauser`,
//! `hand_over_sac` (a porta de saída do agente), registro/payees/preço (`register_token`,
//! `update_token`, `set_payees`, `set_primary_price`) e as leituras. O pauser só LIGA a pausa:
//! não faz clawback nem congela. O agente não custodia saldo: pausar congela operações, não
//! prende o dinheiro de ninguém.
//!
//! Singleton de longa vida (como o yield_distributor): `upgrade` existe e é do
//! admin 2-de-3 — trocar o agente é migrar o admin de TODOS os SACs, e a válvula
//! de upgrade aqui não prende dinheiro (o agente não custodia saldo nenhum).

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, BytesN,
    Env, Vec,
};

/// 3 = a "v2.1" (03/10): preço da primária no lugar da assinatura do seller registrado,
/// `pay` com o operator, TTL estendido na leitura.
/// 4 = a versão única (06/10): v2.1 + `transfer_dvp` com a assinatura do vendedor (PENDENCIAS 202).
/// 5 = F6 (06/10): `emergency_pauser` + pausa mínima do agente.
/// Dois binários nunca dizem a mesma versão.
pub const CONTRACT_VERSION: u32 = 5;

/// Teto de pernas por pagamento/troca (e de payees por token): galeria, Radox, artista,
/// vendedor e folga. Limita o custo da invocação e o tamanho do que cada parte assina.
pub const MAX_LEGS: u32 = 6;

/// Uma unidade inteira do título, em stroops (7 casas). O preço da primária é por unidade.
pub const UNIT: i128 = 10_000_000;

const TTL_THRESHOLD: u32 = 172_800; // ~10 dias
const TTL_EXTEND: u32 = 3_110_400; // ~180 dias

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AgentError {
    NotRegistered = 1,
    AlreadyRegistered = 2,
    InvalidAmount = 3,
    NoPendingAdmin = 4,
    NotPendingAdmin = 5,
    InsufficientBalance = 6,
    WrongPaymentAsset = 7,
    TooManyLegs = 8,
    PayeeNotRegistered = 9,
    EmptyLegs = 10,
    /// Venda pelo seller registrado sem preço de primária fixado pelo admin.
    PriceNotSet = 11,
    /// Venda pelo seller registrado com Σ pernas abaixo de preço × quantidade.
    BelowPrimaryPrice = 12,
    /// v5: o contrato está pausado — nenhum movimento normal (as sete funções) roda; clawback e congelar seguem.
    Paused = 13,
    /// v5: não há `emergency_pauser` configurado (ou ele já renunciou).
    NoEmergencyPauser = 14,
}

/// Uma perna de pagamento (USDC) ou de entrega (unidades do título, no `admin_transfer`).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leg {
    pub to: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub admin: Address,
    pub operator: Address,
}

/// Vínculos de UM token administrado: quem vende (deliver) e quem liquida (clawback).
#[contracttype]
#[derive(Clone)]
pub struct TokenReg {
    pub seller: Address,
    pub settlement: Address,
    /// Ativo aceito como DINHEIRO nas trocas deste token (`transfer_dvp_split`, `pay`).
    ///
    /// Fixado pelo ADMIN (2-de-3) no registro, nunca pelo chamador. Antes disto o
    /// `transfer_dvp` recebia o endereco do dinheiro como parametro e o usava direto:
    /// a disciplina de mandar o USDC canonico morava so no backend
    /// (`sorobanAgent.service.js:177`), e o backend assina com a chave QUENTE do
    /// operator. Quem roubasse essa chave montava a propria invocacao trocando o
    /// pagamento por um token sem valor — e o seller, que NAO assina a troca, perdia o
    /// titulo contra nada. Com o campo aqui, escolher "o que e dinheiro" deixa de ser
    /// poder da chave quente e passa a ser do multisig frio.
    pub payment_asset: Address,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Config,
    PendingAdmin,
    Token(Address),
    /// Quem pode receber DINHEIRO nas trocas deste token (v2). Fixado pelo admin.
    Payees(Address),
    /// Preço mínimo da venda PRIMÁRIA (stroops de USDC por unidade inteira), fixado pelo
    /// admin. É o que substitui a assinatura do seller registrado (v2.1).
    PrimaryPrice(Address),
    /// v5 — pausa (instância). Ausente = não pausado.
    Paused,
    /// v5 — o `emergency_pauser` (instância). Ausente = sem pauser. Última variante, opcional.
    EmergencyPauser,
}

#[contractevent(topics = ["deliver"])]
pub struct DeliverEvent {
    pub token: Address,
    pub to: Address,
    pub amount: i128,
}
/// O recibo da troca. `usdc_amount` é a soma das pernas (o que o comprador pagou);
/// `legs` diz quem recebeu quanto; `ref_id` é a linha do banco que pediu a troca.
#[contractevent(topics = ["dvp"])]
pub struct DvpEvent {
    pub token: Address,
    pub seller: Address,
    pub buyer: Address,
    pub token_amount: i128,
    pub usdc_amount: i128,
    pub legs: Vec<Leg>,
    pub ref_id: u64,
}
#[contractevent(topics = ["pay"])]
pub struct PayEvent {
    pub token: Address,
    pub payer: Address,
    pub usdc_amount: i128,
    pub legs: Vec<Leg>,
    pub ref_id: u64,
}
#[contractevent(topics = ["redeem"])]
pub struct RedeemEvent {
    pub token: Address,
    pub holder: Address,
    pub amount: i128,
    pub ref_id: u64,
}
#[contractevent(topics = ["adm_xfer"])]
pub struct AdminTransferEvent {
    pub token: Address,
    pub from: Address,
    pub amount: i128,
    pub legs: Vec<Leg>,
    pub ref_id: u64,
}
#[contractevent(topics = ["prim_px"])]
pub struct PrimaryPriceEvent {
    pub token: Address,
    pub price_per_unit: i128,
}
#[contractevent(topics = ["payees"])]
pub struct PayeesEvent {
    pub token: Address,
    pub payees: Vec<Address>,
}
#[contractevent(topics = ["clawback"])]
pub struct ClawbackEvent {
    pub token: Address,
    pub from: Address,
    pub amount: i128,
}
#[contractevent(topics = ["authz"])]
pub struct AuthorizedEvent {
    pub token: Address,
    pub addr: Address,
    pub authorized: bool,
}
#[contractevent(topics = ["mint"])]
pub struct MintEvent {
    pub token: Address,
    pub to: Address,
    pub amount: i128,
}
#[contractevent(topics = ["reg"])]
pub struct RegisterEvent {
    pub token: Address,
    pub seller: Address,
    pub settlement: Address,
    pub payment_asset: Address,
}

/// v5 — pausa ligada. Dado: `true` (valor único, como nos irmãos).
#[contractevent(topics = ["paused"], data_format = "single-value")]
pub struct PausedEvent {
    pub paused: bool,
}
/// v5 — pausa desligada pelo admin. Dado: `true`.
#[contractevent(topics = ["resumed"], data_format = "single-value")]
pub struct ResumedEvent {
    pub resumed: bool,
}
/// v5 — QUEM ligou a pausa de emergência.
#[contractevent(topics = ["emergpaus"], data_format = "single-value")]
pub struct EmergencyPausedEvent {
    pub pauser: Address,
}
/// v5 — toda troca do pauser: dado `(anterior, novo)`.
#[contractevent(topics = ["pauserset"], data_format = "vec")]
pub struct PauserSetEvent {
    pub previous: Option<Address>,
    pub new: Option<Address>,
}
/// v5 — o pauser renunciou. Dado: quem renunciou.
#[contractevent(topics = ["pauserren"], data_format = "single-value")]
pub struct PauserRenouncedEvent {
    pub pauser: Address,
}

#[contract]
pub struct TransferAgent;

#[contractimpl]
impl TransferAgent {
    /// Nasce pronto (Protocolo 22): papéis fixados no deploy.
    ///
    /// O `require_auth` do admin e pelo mesmo motivo que o `yield_distributor` documenta:
    /// *"ensures the admin address consents to being set (prevents installing an admin the
    /// deployer does not control)"*. O `Config` carrega admin E operator, entao um erro de
    /// digitacao em qualquer um dos dois ficaria assado no deploy — e este e o unico ponto
    /// onde um admin entra sem passar pelo fluxo de dois passos (`propose_admin`/`accept_admin`).
    pub fn __constructor(e: Env, config: Config) {
        config.admin.require_auth();
        e.storage().instance().set(&DataKey::Config, &config);
        bump(&e);
    }

    // ── Registro (admin) ──

    /// Registra um SAC administrado: o contrato de venda (deliver) e o de
    /// liquidação (clawback). Autoriza o seller a MANTER/ENVIAR o depósito —
    /// todo o resto do mundo nasce e morre não autorizado.
    pub fn register_token(
        e: Env,
        token: Address,
        seller: Address,
        settlement: Address,
        payment_asset: Address,
    ) -> Result<(), AgentError> {
        config(&e).admin.require_auth();
        let key = DataKey::Token(token.clone());
        if e.storage().persistent().has(&key) {
            return Err(AgentError::AlreadyRegistered);
        }
        let reg = TokenReg {
            seller: seller.clone(),
            settlement: settlement.clone(),
            payment_asset,
        };
        e.storage().persistent().set(&key, &reg);
        e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
        sac(&e, &token).set_authorized(&seller, &true);
        RegisterEvent { token, seller, settlement, payment_asset: reg.payment_asset }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Substitui os vínculos de um token já registrado (nova oferta no mesmo
    /// ativo, contrato de liquidação novo). Admin.
    pub fn update_token(
        e: Env,
        token: Address,
        seller: Address,
        settlement: Address,
        payment_asset: Address,
    ) -> Result<(), AgentError> {
        config(&e).admin.require_auth();
        let key = DataKey::Token(token.clone());
        if !e.storage().persistent().has(&key) {
            return Err(AgentError::NotRegistered);
        }
        let reg = TokenReg {
            seller: seller.clone(),
            settlement: settlement.clone(),
            payment_asset,
        };
        e.storage().persistent().set(&key, &reg);
        e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
        sac(&e, &token).set_authorized(&seller, &true);
        RegisterEvent { token, seller, settlement, payment_asset: reg.payment_asset }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── O caminho quente: entrega atômica (seller registrado) ──

    /// Entrega SEP-8 numa invocação só, chamada PELO contrato de venda (trade
    /// direct / claim). A auth de invocador-contrato cobre SÓ o filho direto
    /// (este agente) — um `transfer(seller→to)` aninhado exigiria o seller
    /// autorizar o neto (`authorize_as_current_contract` em cada versão de
    /// venda). Em vez desse acoplamento, o movimento é DO PRÓPRIO agente, que
    /// é o admin do SAC: authorize(to) → clawback(seller) → mint(to) →
    /// revoke(to). Oferta conservada (−amount +amount na mesma invocação);
    /// estado de repouso do holder: NÃO autorizado.
    pub fn deliver(e: Env, token: Address, to: Address, amount: i128) -> Result<(), AgentError> {
        require_not_paused(&e)?;
        if amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        let reg = registration(&e, &token)?;
        reg.seller.require_auth();
        let admin_client = sac(&e, &token);
        admin_client.set_authorized(&to, &true);
        admin_client.clawback(&reg.seller, &amount);
        admin_client.mint(&to, &amount);
        admin_client.set_authorized(&to, &false);
        DeliverEvent { token, to, amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── Dinheiro em pernas (v2) ──

    /// Fixa QUEM pode receber dinheiro nas trocas deste token (galeria, tesouraria da
    /// Radox, o artista quando tem carteira). Admin — a mesma chave fria que fixa o
    /// `payment_asset`. Substitui a lista inteira; lista vazia desliga o `pay`.
    ///
    /// É o que impede a chave QUENTE de criar uma perna para si: no `pay` e no
    /// `transfer_dvp_split`, toda perna vai para um endereço daqui (ou para o vendedor).
    pub fn set_payees(e: Env, token: Address, payees: Vec<Address>) -> Result<(), AgentError> {
        if payees.len() > MAX_LEGS {
            return Err(AgentError::TooManyLegs);
        }
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        let key = DataKey::Payees(token.clone());
        e.storage().persistent().set(&key, &payees);
        e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
        PayeesEvent { token, payees }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Fixa o preço MÍNIMO da venda primária, em stroops de USDC por unidade inteira do
    /// título (`UNIT`). Admin — chave fria.
    ///
    /// Por que existe (v2.1): na primária o vendedor é o seller REGISTRADO (o
    /// distribuidor), que em produção é chave fria em multisig e não assina uma venda por
    /// comprador. A assinatura dele no `transfer_dvp_split` é então substituída por estes
    /// termos: o contrato só deixa sair o estoque do seller registrado se o comprador
    /// pagar Σ pernas ≥ preço × quantidade, e só para payees. Pôr a ops como signatária do
    /// distribuidor reabriria o buraco da v1 (ops assinaria operator + seller e levaria o
    /// estoque por 1 stroop); com o preço aqui, a chave quente não decide quanto custa.
    ///
    /// ponytail: preço por unidade inteira é um piso inteiro — o backend fixa
    /// ⌊preço_total / unidades⌋, e o piso fica abaixo do preço real por menos de 1 stroop
    /// por unidade. As pernas carregam o valor exato; o piso só barra venda barata.
    pub fn set_primary_price(e: Env, token: Address, price_per_unit: i128) -> Result<(), AgentError> {
        if price_per_unit <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        let key = DataKey::PrimaryPrice(token.clone());
        e.storage().persistent().set(&key, &price_per_unit);
        e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
        PrimaryPriceEvent { token, price_per_unit }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Pagamento em pernas (custódia, taxa de retirada). O PAGADOR e o OPERATOR assinam
    /// (v2.1): o pagamento sempre passa pelo backend, que confere a dívida antes de
    /// co-assinar. A assinatura do pagador cobre a invocação inteira — pernas, valores e
    /// `ref_id` —, então ninguém reaproveita a autorização com outro destino ou valor.
    /// Toda perna vai para um payee do token; o dinheiro é o `payment_asset` do admin.
    ///
    /// O `ref_id` do evento é um rótulo, NÃO fonte de verdade: o reconciliador casa o
    /// pagamento pelo hash que o próprio backend transmitiu (`pendingTxHash`) e confere,
    /// no evento, pagador e valor por destino contra a linha do banco.
    pub fn pay(
        e: Env,
        token: Address,
        payer: Address,
        usdc: Address,
        legs: Vec<Leg>,
        ref_id: u64,
    ) -> Result<(), AgentError> {
        require_not_paused(&e)?;
        let total = legs_total(&legs)?;
        let reg = registration(&e, &token)?;
        if usdc != reg.payment_asset {
            return Err(AgentError::WrongPaymentAsset);
        }
        check_payees(&e, &token, &legs, None)?;
        config(&e).operator.require_auth();
        payer.require_auth();
        let money = token::Client::new(&e, &usdc);
        for leg in legs.iter() {
            money.transfer(&payer, &leg.to, &leg.amount);
        }
        PayEvent { token, payer, usdc_amount: total, legs, ref_id }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Troca ATÔMICA de título por dinheiro, com o dinheiro em PERNAS. Três
    /// assinaturas, uma invocação:
    ///
    ///   • operator — a plataforma conduz (e só ela co-assina: é onde o backend impõe
    ///     a dívida de custódia antes de deixar o título sair);
    ///   • seller   — CONSENTE em vender por estas pernas exatas. É a diferença para
    ///     o `transfer_dvp` da v1, onde o título de qualquer holder saía por qualquer
    ///     preço > 0 com operator + um comprador;
    ///   • buyer    — autoriza o pagamento. A entry dele cobre a invocação inteira.
    ///
    /// EXCEÇÃO (v2.1) — a ÚNICA, e só AQUI (o `transfer_dvp` não a tem): venda PRIMÁRIA,
    /// `seller == reg.seller`: o seller registrado é
    /// chave fria e não assina. No lugar da assinatura dele valem os termos que o admin
    /// fixou: preço (`set_primary_price`; sem preço → `PriceNotSet`), Σ pernas ≥ preço ×
    /// quantidade (`BelowPrimaryPrice`) e pernas só para payees (nenhuma ao próprio
    /// seller fora dos payees). Qualquer outro seller assina, como antes.
    ///
    /// Toda perna vai para um payee do token ou para o próprio seller — a chave quente
    /// não cria destino. O título é o sanduíche da v1 (o agente é o admin do SAC):
    /// clawback(seller) → mint(buyer), oferta conservada, e o buyer volta ao estado de
    /// autorização em que estava. Nada é persistido: ou confirma inteira, ou nada move.
    #[allow(clippy::too_many_arguments)]
    pub fn transfer_dvp_split(
        e: Env,
        seller: Address,
        buyer: Address,
        token: Address,
        token_amount: i128,
        usdc: Address,
        legs: Vec<Leg>,
        ref_id: u64,
    ) -> Result<(), AgentError> {
        settle(&e, seller, buyer, token, token_amount, usdc, legs, ref_id, true)
    }

    /// Troca ATÔMICA de título por UM preço pago ao vendedor — o DvP de uma perna só
    /// (`{seller, usdc_amount}`), com a ABI da v1. Operator + SELLER + buyer assinam, SEMPRE
    /// (PENDENCIAS 202): este caminho não tem a dispensa da primária, nem para o seller
    /// registrado. A v1 não exigia o vendedor — a chave quente, fazendo-se de comprador,
    /// levava o saldo de qualquer holder por 1 stroop (testnet, 26/09, tx `e22a4316…`).
    /// A assinatura de cada parte cobre a invocação inteira, preço incluído. O evento é o
    /// do split (`ref_id` 0).
    pub fn transfer_dvp(
        e: Env,
        seller: Address,
        buyer: Address,
        token: Address,
        token_amount: i128,
        usdc: Address,
        usdc_amount: i128,
    ) -> Result<(), AgentError> {
        let legs = soroban_sdk::vec![&e, Leg { to: seller.clone(), amount: usdc_amount }];
        settle(&e, seller, buyer, token, token_amount, usdc, legs, 0, false)
    }

    /// Queima COM o consentimento do holder (entrega da obra física na retirada).
    /// Operator + holder: o holder sozinho não queima (o backend confere a dívida antes
    /// de co-assinar), e a plataforma sozinha não queima o título de ninguém.
    pub fn redeem(
        e: Env,
        token: Address,
        holder: Address,
        amount: i128,
        ref_id: u64,
    ) -> Result<(), AgentError> {
        require_not_paused(&e)?;
        if amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        registration(&e, &token)?;
        config(&e).operator.require_auth();
        holder.require_auth();
        if token::Client::new(&e, &token).balance(&holder) < amount {
            return Err(AgentError::InsufficientBalance);
        }
        sac(&e, &token).clawback(&holder, &amount);
        RedeemEvent { token, holder, amount, ref_id }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Governança (admin 2-de-3): tira Σ pernas de `from` e entrega cada perna, numa
    /// invocação — a resolução da inadimplência por fração. Oferta conservada
    /// (−Σ em `from`, +Σ nas pernas); cada destinatário volta ao estado de autorização
    /// em que estava. Os destinos não são restritos a payees: o admin já cunha.
    pub fn admin_transfer(
        e: Env,
        token: Address,
        from: Address,
        legs: Vec<Leg>,
        ref_id: u64,
    ) -> Result<(), AgentError> {
        require_not_paused(&e)?;
        let total = legs_total(&legs)?;
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        if token::Client::new(&e, &token).balance(&from) < total {
            return Err(AgentError::InsufficientBalance);
        }
        let admin_client = sac(&e, &token);
        admin_client.clawback(&from, &total);
        for leg in legs.iter() {
            mint_preserving_auth(&admin_client, &leg.to, leg.amount);
        }
        AdminTransferEvent { token, from, amount: total, legs, ref_id }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── Liquidação (settlement registrado ou admin) ──

    /// Clawback pelo contrato de LIQUIDAÇÃO registrado (resgate v4). O holder
    /// não precisa estar autorizado (clawback ignora o flag). Sem try_require_auth
    /// no SDK, cada papel tem sua porta explícita — admin usa admin_clawback.
    pub fn clawback(e: Env, token: Address, from: Address, amount: i128) -> Result<(), AgentError> {
        // Sem `require_not_paused`: clawback é ATO DE CONFORMIDADE (ordem judicial, sanção, exploit)
        // e tem de funcionar com o contrato pausado — é o remédio da emergência, não um movimento.
        if amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        let reg = registration(&e, &token)?;
        reg.settlement.require_auth();
        sac(&e, &token).clawback(&from, &amount);
        ClawbackEvent { token, from, amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Clawback pela conta de governança 2-de-3 (intervenção manual).
    pub fn admin_clawback(
        e: Env,
        token: Address,
        from: Address,
        amount: i128,
    ) -> Result<(), AgentError> {
        // Sem `require_not_paused`: ato de conformidade do admin, segue valendo sob a pausa (ver `clawback`).
        if amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        sac(&e, &token).clawback(&from, &amount);
        ClawbackEvent { token, from, amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── Operação (operator ou admin) ──

    /// Liga/desliga a autorização de um endereço (gate de KYC / congelamento).
    /// Porta do OPERATOR (dia a dia); o admin tem a admin_set_authorized.
    pub fn set_authorized(
        e: Env,
        token: Address,
        addr: Address,
        authorized: bool,
    ) -> Result<(), AgentError> {
        registration(&e, &token)?;
        config(&e).operator.require_auth();
        sac(&e, &token).set_authorized(&addr, &authorized);
        AuthorizedEvent { token, addr, authorized }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// A mesma alavanca, pela governança 2-de-3 (congelamento de emergência não
    /// pode depender da chave de operações estar saudável).
    pub fn admin_set_authorized(
        e: Env,
        token: Address,
        addr: Address,
        authorized: bool,
    ) -> Result<(), AgentError> {
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        sac(&e, &token).set_authorized(&addr, &authorized);
        AuthorizedEvent { token, addr, authorized }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── Emissão (admin) ──

    pub fn mint(e: Env, token: Address, to: Address, amount: i128) -> Result<(), AgentError> {
        require_not_paused(&e)?;
        if amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        sac(&e, &token).mint(&to, &amount);
        MintEvent { token, to, amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Devolve o admin de um SAC para outro endereço (migração/desligamento).
    /// Admin 2-de-3 — é a porta de saída do próprio agente.
    pub fn hand_over_sac(e: Env, token: Address, new_admin: Address) -> Result<(), AgentError> {
        registration(&e, &token)?;
        config(&e).admin.require_auth();
        sac(&e, &token).set_admin(&new_admin);
        // O agente esquece o token INTEIRO: registro, payees e preço da primária. Um
        // registro novo do mesmo token não pode herdar recebedores nem preço antigos.
        e.storage().persistent().remove(&DataKey::Token(token.clone()));
        e.storage().persistent().remove(&DataKey::Payees(token.clone()));
        e.storage().persistent().remove(&DataKey::PrimaryPrice(token));
        bump(&e);
        Ok(())
    }

    // ── Governança ──

    pub fn set_operator(e: Env, operator: Address) {
        let mut cfg = config(&e);
        cfg.admin.require_auth();
        cfg.operator = operator;
        e.storage().instance().set(&DataKey::Config, &cfg);
        bump(&e);
    }

    /// Rotação de admin em 2 passos (proposta + aceite) — igual aos irmãos.
    pub fn propose_admin(e: Env, new_admin: Address) {
        config(&e).admin.require_auth();
        e.storage().instance().set(&DataKey::PendingAdmin, &new_admin);
        bump(&e);
    }
    pub fn accept_admin(e: Env) -> Result<(), AgentError> {
        let pending: Address = e
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(AgentError::NoPendingAdmin)?;
        pending.require_auth();
        let mut cfg = config(&e);
        cfg.admin = pending;
        e.storage().instance().set(&DataKey::Config, &cfg);
        e.storage().instance().remove(&DataKey::PendingAdmin);
        bump(&e);
        Ok(())
    }

    // ── Pausa e emergency_pauser (v5, F6) ──

    /// Pausa o agente. Admin. Bloqueia os movimentos normais (lista no cabeçalho do
    /// crate; clawback e congelar seguem abertos); `resume`, `upgrade`, rotação de admin e `hand_over_sac` seguem abertos.
    pub fn pause(e: Env) {
        config(&e).admin.require_auth();
        e.storage().instance().set(&DataKey::Paused, &true);
        PausedEvent { paused: true }.publish(&e);
        bump(&e);
    }

    /// Despausa. SÓ o admin — o `emergency_pauser` não despausa.
    pub fn resume(e: Env) {
        config(&e).admin.require_auth();
        e.storage().instance().set(&DataKey::Paused, &false);
        ResumedEvent { resumed: true }.publish(&e);
        bump(&e);
    }

    /// Define (ou remove, com `None`) o `emergency_pauser`. Admin. Trocar invalida o
    /// anterior no ato (`emergency_pause` lê a chave a cada chamada). TODA troca emite
    /// `pauserset` com (anterior, novo): é o que o monitor e o fornecedor leem.
    pub fn set_emergency_pauser(e: Env, pauser: Option<Address>) {
        config(&e).admin.require_auth();
        let previous: Option<Address> = e.storage().instance().get(&DataKey::EmergencyPauser);
        match &pauser {
            Some(p) => e.storage().instance().set(&DataKey::EmergencyPauser, p),
            None => e.storage().instance().remove(&DataKey::EmergencyPauser),
        }
        PauserSetEvent { previous, new: pauser }.publish(&e);
        bump(&e);
    }

    /// O PRÓPRIO pauser renuncia: só ele assina (o admin remove com
    /// `set_emergency_pauser(None)`). Depois disto `emergency_pause` cai em `NoEmergencyPauser`
    /// até o admin definir outro. Emite `pauserset` (anterior, None) e `pauserren`. Não mexe
    /// em pausa, admin nem dinheiro.
    pub fn renounce_emergency_pauser(e: Env) -> Result<(), AgentError> {
        let pauser = emergency_pauser_of(&e).ok_or(AgentError::NoEmergencyPauser)?;
        pauser.require_auth();
        e.storage().instance().remove(&DataKey::EmergencyPauser);
        PauserSetEvent { previous: Some(pauser.clone()), new: None }.publish(&e);
        PauserRenouncedEvent { pauser }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Pausa de emergência: SÓ o `emergency_pauser` assina e liga a MESMA pausa de `pause()`.
    /// Não despausa, não move saldo nem dinheiro, não troca admin/operator, não faz upgrade —
    /// esta função não tem outro efeito. Idempotente.
    pub fn emergency_pause(e: Env) -> Result<(), AgentError> {
        let pauser = emergency_pauser_of(&e).ok_or(AgentError::NoEmergencyPauser)?;
        pauser.require_auth();
        e.storage().instance().set(&DataKey::Paused, &true);
        PausedEvent { paused: true }.publish(&e);
        EmergencyPausedEvent { pauser }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Upgrade do WASM (admin 2-de-3). O agente não custodia saldo — a válvula
    /// não prende dinheiro de ninguém.
    ///
    /// NOTA (09/09): esta linha dizia "≠ token_sale, onde upgrade morreu no v10".
    /// Não morreu. Foi removido por iniciativa em 24/08 e o Pedro REVOGOU no mesmo
    /// dia — `token_sale/src/lib.rs` tem `pub fn upgrade`, sem lock de dinheiro
    /// dentro, porque é exatamente com USDC escrowado que a Ledger precisa poder
    /// destravar. Os três contratos concordam hoje; era a prosa que divergia.
    pub fn upgrade(e: Env, new_wasm_hash: BytesN<32>) {
        config(&e).admin.require_auth();
        e.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    // ── Views ──

    pub fn version(_e: Env) -> u32 {
        CONTRACT_VERSION
    }
    pub fn get_paused(e: Env) -> bool {
        is_paused(&e)
    }
    pub fn emergency_pauser(e: Env) -> Option<Address> {
        emergency_pauser_of(&e)
    }
    pub fn get_config(e: Env) -> Config {
        config(&e)
    }
    pub fn get_token(e: Env, token: Address) -> Option<TokenReg> {
        e.storage().persistent().get(&DataKey::Token(token))
    }
    pub fn is_registered(e: Env, token: Address) -> bool {
        e.storage().persistent().has(&DataKey::Token(token))
    }
    pub fn get_payees(e: Env, token: Address) -> Vec<Address> {
        payees(&e, &token)
    }
    pub fn get_primary_price(e: Env, token: Address) -> Option<i128> {
        primary_price(&e, &token)
    }
}

// ── Helpers ──

/// O DvP por inteiro, para `transfer_dvp` (`primary_waiver = false`) e `transfer_dvp_split`
/// (`true`). A dispensa da assinatura do vendedor existe só com `primary_waiver` E
/// `seller == reg.seller` (a primária, com preço do admin); em toda outra troca o vendedor assina.
#[allow(clippy::too_many_arguments)]
fn settle(
    e: &Env,
    seller: Address,
    buyer: Address,
    token: Address,
    token_amount: i128,
    usdc: Address,
    legs: Vec<Leg>,
    ref_id: u64,
    primary_waiver: bool,
) -> Result<(), AgentError> {
    require_not_paused(e)?;
    if token_amount <= 0 {
        return Err(AgentError::InvalidAmount);
    }
    let total = legs_total(&legs)?;
    let reg = registration(e, &token)?;
    if usdc != reg.payment_asset {
        return Err(AgentError::WrongPaymentAsset);
    }
    let primary = primary_waiver && seller == reg.seller;
    if primary {
        check_payees(e, &token, &legs, None)?;
        let price = primary_price(e, &token).ok_or(AgentError::PriceNotSet)?;
        if total < price_floor(price, token_amount)? {
            return Err(AgentError::BelowPrimaryPrice);
        }
    } else {
        check_payees(e, &token, &legs, Some(&seller))?;
    }
    config(e).operator.require_auth();
    if !primary {
        seller.require_auth();
    }
    buyer.require_auth();
    if token::Client::new(e, &token).balance(&seller) < token_amount {
        return Err(AgentError::InsufficientBalance);
    }
    // dinheiro primeiro: recusa de qualquer perna derruba a invocação inteira
    // antes de qualquer mexida na oferta do título.
    let money = token::Client::new(e, &usdc);
    for leg in legs.iter() {
        money.transfer(&buyer, &leg.to, &leg.amount);
    }
    let admin_client = sac(e, &token);
    admin_client.clawback(&seller, &token_amount);
    mint_preserving_auth(&admin_client, &buyer, token_amount);
    DvpEvent { token, seller, buyer, token_amount, usdc_amount: total, legs, ref_id }.publish(e);
    bump(e);
    Ok(())
}

fn is_paused(e: &Env) -> bool {
    e.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}

/// Primeira linha de todo MOVIMENTO normal de saldo ou dinheiro (as sete funções do cabeçalho). Atos de
/// conformidade (`clawback`, `admin_clawback`, congelar) NÃO a chamam: valem sob a pausa.
fn require_not_paused(e: &Env) -> Result<(), AgentError> {
    if is_paused(e) {
        return Err(AgentError::Paused);
    }
    Ok(())
}

fn emergency_pauser_of(e: &Env) -> Option<Address> {
    e.storage().instance().get(&DataKey::EmergencyPauser)
}

fn config(e: &Env) -> Config {
    e.storage().instance().get(&DataKey::Config).unwrap()
}

/// Lê uma entrada persistente e, se ela existe, estende o TTL (v2.1). Registro, payees e
/// preço só eram estendidos quando ESCRITOS: um token usado uma vez por ano (renovação de
/// custódia) chegaria arquivado. Ler é usar; usar mantém vivo.
fn read_live<V: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>>(e: &Env, key: &DataKey) -> Option<V> {
    let v = e.storage().persistent().get(key);
    if v.is_some() {
        e.storage().persistent().extend_ttl(key, TTL_THRESHOLD, TTL_EXTEND);
    }
    v
}

fn registration(e: &Env, token: &Address) -> Result<TokenReg, AgentError> {
    read_live(e, &DataKey::Token(token.clone())).ok_or(AgentError::NotRegistered)
}

fn payees(e: &Env, token: &Address) -> Vec<Address> {
    read_live(e, &DataKey::Payees(token.clone())).unwrap_or_else(|| Vec::new(e))
}

fn primary_price(e: &Env, token: &Address) -> Option<i128> {
    read_live(e, &DataKey::PrimaryPrice(token.clone()))
}

/// ⌈preço × quantidade / UNIT⌉ — o piso da primária, arredondado A FAVOR do vendedor.
fn price_floor(price_per_unit: i128, token_amount: i128) -> Result<i128, AgentError> {
    let gross = price_per_unit.checked_mul(token_amount).ok_or(AgentError::InvalidAmount)?;
    let up = gross.checked_add(UNIT - 1).ok_or(AgentError::InvalidAmount)?;
    Ok(up / UNIT)
}

/// 1 ≤ pernas ≤ MAX_LEGS, toda perna > 0. Devolve a soma (sem estouro silencioso).
fn legs_total(legs: &Vec<Leg>) -> Result<i128, AgentError> {
    if legs.is_empty() {
        return Err(AgentError::EmptyLegs);
    }
    if legs.len() > MAX_LEGS {
        return Err(AgentError::TooManyLegs);
    }
    let mut total: i128 = 0;
    for leg in legs.iter() {
        if leg.amount <= 0 {
            return Err(AgentError::InvalidAmount);
        }
        total = total.checked_add(leg.amount).ok_or(AgentError::InvalidAmount)?;
    }
    Ok(total)
}

/// Toda perna vai para um payee do token (ou, numa troca, para o próprio vendedor).
fn check_payees(
    e: &Env,
    token: &Address,
    legs: &Vec<Leg>,
    seller: Option<&Address>,
) -> Result<(), AgentError> {
    let allowed = payees(e, token);
    for leg in legs.iter() {
        if seller != Some(&leg.to) && !allowed.contains(&leg.to) {
            return Err(AgentError::PayeeNotRegistered);
        }
    }
    Ok(())
}

/// Mint num endereço que pode estar em repouso NÃO autorizado: autoriza só se
/// precisar, cunha e devolve ao estado de antes. Quem já estava autorizado (o
/// depósito do seller registrado, um holder no gate de KYC) continua autorizado.
fn mint_preserving_auth(admin_client: &token::StellarAssetClient, to: &Address, amount: i128) {
    let was = admin_client.authorized(to);
    if !was {
        admin_client.set_authorized(to, &true);
    }
    admin_client.mint(to, &amount);
    if !was {
        admin_client.set_authorized(to, &false);
    }
}

fn sac<'a>(e: &Env, token: &Address) -> token::StellarAssetClient<'a> {
    token::StellarAssetClient::new(e, token)
}

fn bump(e: &Env) {
    e.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
}

#[cfg(all(test, feature = "testing"))]
mod test;
