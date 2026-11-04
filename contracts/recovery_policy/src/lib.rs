#![no_std]
//! # Guardião com espera — policy de recuperação para a smart account OZ
//!
//! Decisão do Pedro (24/09/2026): ninguém fica de fora da própria carteira SEM
//! perder o USDC parado nela. A recuperação passa a ser "adicionar uma chave nova à
//! MESMA carteira", não reemitir títulos numa carteira nova.
//!
//! ## Como se instala
//! A carteira (OZ `stellar-accounts` 0.7.0, WASM `1e25669b…`) ganha uma context rule
//! `CallContract(<ela mesma>)` cujos signatários são os guardiões (`Signer::Delegated`)
//! e cuja única policy é este contrato. Quem instala é a própria carteira, com a
//! assinatura do dono (`add_context_rule`). A conta chama `install` aqui.
//!
//! ## O que o guardião consegue fazer (e só isso)
//! 1. `propose(conta, regra, chave_nova, aprovadores)` — os guardiões, com `require_auth`
//!    de cada um, registram on-chain a chave que querem adicionar. Emite
//!    `recovery_proposed` com `executable_at` (para o aviso por e-mail).
//! 2. Passado o atraso, a conta executa `add_context_rule(Default, _, None, [chave_nova], {})`
//!    autorizada pela regra dos guardiões; a conta chama `enforce` aqui, que só deixa
//!    passar EXATAMENTE essa chamada, com EXATAMENTE a chave proposta, e consome a proposta.
//!    A proposta vence `EXECUTION_WINDOW_SECS` depois do fim da espera (mainnet: 30 dias);
//!    vencida, a execução cai em `Expired` e os guardiões precisam propor de novo.
//!
//! Qualquer outra coisa pela regra dos guardiões — `execute` (mover USDC, chamar
//! outro contrato), `remove_signer`, `remove_context_rule`, `upgrade`, `add_signer` —
//! cai em `ActionNotAllowed`. Chamadas a outro contrato nem chegam aqui: o tipo
//! `CallContract(self)` da regra já as recusa na conta (`UnvalidatedContext`).
//!
//! ## Quem cancela
//! `cancel` exige a autorização da própria CONTA. A regra dos guardiões é
//! `CallContract(conta)` e não casa com uma chamada a este contrato, então só uma regra
//! do dono (`Default`, uma por passkey desde o PR #129) autoriza: qualquer chave atual
//! do dono cancela; guardião não cancela.
//!
//! ## Sem admin, sem upgrade — de propósito
//! Um admin capaz de trocar este código poderia zerar a espera de todas as carteiras
//! de uma vez (na carteira da empresa o guardião é só a governança da Radox). Bug achado
//! = nova versão implantada e a carteira troca a policy com a chave do dono.

use soroban_sdk::{
    auth::{Context, ContractContext},
    contract, contractclient, contracterror, contractevent, contractimpl, contracttype,
    panic_with_error, Address, Bytes, BytesN, Env, Map, String, Symbol, TryFromVal, Val, Vec,
};

const CONTRACT_VERSION: u32 = 2;

/// Espera de produção decidida pelo Pedro (24/09/2026): 7 dias.
pub const PRODUCTION_DELAY_SECS: u64 = 604_800;
/// Janela de execução de produção, decidida pelo Pedro (24/09/2026): a proposta vence
/// 30 dias depois do fim da espera. Proposta esquecida não fica executável para sempre.
pub const PRODUCTION_EXECUTION_WINDOW_SECS: u64 = 2_592_000;

// Piso da espera gravado no binário. No mainnet ninguém instala menos que 7 dias,
// nem por engano do backend. Testnet aceita 60 s só para a prova de rede.
#[cfg(feature = "mainnet")]
pub const MIN_DELAY_SECS: u64 = PRODUCTION_DELAY_SECS;
#[cfg(any(feature = "testnet", feature = "testing"))]
pub const MIN_DELAY_SECS: u64 = 60;
// Janela gravada no binário, como o piso: mainnet 30 dias; testnet 120 s para a prova.
#[cfg(feature = "mainnet")]
pub const EXECUTION_WINDOW_SECS: u64 = PRODUCTION_EXECUTION_WINDOW_SECS;
#[cfg(any(feature = "testnet", feature = "testing"))]
pub const EXECUTION_WINDOW_SECS: u64 = 120;
#[cfg(all(feature = "testing", any(feature = "testnet", feature = "mainnet")))]
compile_error!("feature `testing` junto com `testnet`/`mainnet`: use --no-default-features --features testnet|mainnet");
#[cfg(all(feature = "testnet", feature = "mainnet"))]
compile_error!("features `testnet` e `mainnet` sao mutuamente exclusivas");
#[cfg(not(any(feature = "testing", feature = "testnet", feature = "mainnet")))]
compile_error!("escolha uma rede: --features testnet|mainnet (ou o default `testing` para cargo test)");

const TTL_THRESHOLD: u32 = 518_400; // ~30 dias

// ── Tipos da conta OZ stellar-accounts 0.7.0 (mesmo XDR; lidos da interface do WASM
//    1e25669b… na testnet). Espelhados aqui para não puxar o crate inteiro. ──

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Signer {
    Delegated(Address),
    External(Address, Bytes),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextRuleType {
    Default,
    CallContract(Address),
    CreateContract(BytesN<32>),
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextRule {
    pub context_type: ContextRuleType,
    pub id: u32,
    pub name: String,
    pub policies: Vec<Address>,
    pub policy_ids: Vec<u32>,
    pub signer_ids: Vec<u32>,
    pub signers: Vec<Signer>,
    pub valid_until: Option<u32>,
}

#[allow(dead_code)]
#[contractclient(name = "SmartAccountClient")]
trait SmartAccountInterface {
    fn get_context_rule(e: Env, context_rule_id: u32) -> ContextRule;
}

// ── Estado ──

/// Parâmetro de instalação (o `Val` que a conta repassa a `install`).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryParams {
    /// Quantos guardiões precisam aprovar (proposta e execução).
    pub threshold: u32,
    /// Espera entre a proposta e a execução, em segundos (timestamp do ledger).
    pub delay: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub signer: Signer,
    pub proposed_at: u64,
    pub executable_at: u64,
    /// Último instante (inclusive) em que a execução vale: `executable_at + EXECUTION_WINDOW_SECS`.
    pub expires_at: u64,
}

#[contracttype]
enum DataKey {
    Params(Address, u32),
    Proposal(Address, u32),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum RecoveryError {
    NotInstalled = 1,
    AlreadyInstalled = 2,
    InvalidThreshold = 3,
    DelayTooShort = 4,
    RuleMustBeCallContractSelf = 5,
    GuardianMustBeDelegated = 6,
    NotAGuardian = 7,
    DuplicateApprover = 8,
    InsufficientApprovals = 9,
    SignerMustBeExternal = 10,
    NoProposal = 11,
    TooEarly = 12,
    ActionNotAllowed = 13,
    KeyMismatch = 14,
    MathOverflow = 15,
    Expired = 16,
}

// ── Eventos (o backend avisa o dono por e-mail a partir do `recovery_proposed`) ──

#[contractevent(topics = ["recovery_installed"])]
pub struct RecoveryInstalled {
    #[topic]
    pub smart_account: Address,
    pub context_rule_id: u32,
    pub threshold: u32,
    pub delay: u64,
}

#[contractevent(topics = ["recovery_proposed"])]
pub struct RecoveryProposed {
    #[topic]
    pub smart_account: Address,
    pub context_rule_id: u32,
    pub signer: Signer,
    pub proposed_at: u64,
    pub executable_at: u64,
    pub expires_at: u64,
}

#[contractevent(topics = ["recovery_cancelled"])]
pub struct RecoveryCancelled {
    #[topic]
    pub smart_account: Address,
    pub context_rule_id: u32,
}

#[contractevent(topics = ["recovery_executed"])]
pub struct RecoveryExecuted {
    #[topic]
    pub smart_account: Address,
    pub context_rule_id: u32,
    pub signer: Signer,
}

#[contractevent(topics = ["recovery_uninstalled"])]
pub struct RecoveryUninstalled {
    #[topic]
    pub smart_account: Address,
    pub context_rule_id: u32,
}

// ── Helpers ──

fn bump_instance(e: &Env) {
    let max = e.storage().max_ttl();
    e.storage().instance().extend_ttl(TTL_THRESHOLD, max);
}

fn bump(e: &Env, key: &DataKey) {
    let max = e.storage().max_ttl();
    e.storage().persistent().extend_ttl(key, TTL_THRESHOLD, max);
}

fn params_of(e: &Env, account: &Address, rule_id: u32) -> RecoveryParams {
    let key = DataKey::Params(account.clone(), rule_id);
    let p: RecoveryParams = e
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| panic_with_error!(e, RecoveryError::NotInstalled));
    bump(e, &key);
    p
}

fn is_guardian(rule: &ContextRule, addr: &Address) -> bool {
    rule.signers.contains(Signer::Delegated(addr.clone()))
}

#[contract]
pub struct RecoveryPolicy;

#[contractimpl]
impl RecoveryPolicy {
    // ── Interface de policy da conta OZ 0.7.0 (enforce / install / uninstall) ──

    /// Chamada pela conta dentro do `__check_auth`, para cada contexto autorizado
    /// pela regra dos guardiões. Entra em pânico = a conta recusa a transação inteira.
    pub fn enforce(
        e: Env,
        context: Context,
        authenticated_signers: Vec<Signer>,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();
        bump_instance(&e);
        let params = params_of(&e, &smart_account, context_rule.id);

        // 1. Limiar: guardiões (Delegated) da regra que de fato autenticaram. Com policy,
        //    a conta NÃO confere signatários (delega a nós) — sem esta conta, zero
        //    assinaturas passariam.
        let mut approvals: u32 = 0;
        for s in authenticated_signers.iter() {
            if matches!(s, Signer::Delegated(_)) && context_rule.signers.contains(&s) {
                approvals += 1;
            }
        }
        if approvals < params.threshold {
            panic_with_error!(&e, RecoveryError::InsufficientApprovals);
        }

        // 2. Ação: só `add_context_rule` na própria conta, com a forma exata.
        let Context::Contract(ContractContext { contract, fn_name, args }) = context else {
            panic_with_error!(&e, RecoveryError::ActionNotAllowed);
        };
        if contract != smart_account
            || fn_name != Symbol::new(&e, "add_context_rule")
            || args.len() != 5
        {
            panic_with_error!(&e, RecoveryError::ActionNotAllowed);
        }
        let ctx_type = ContextRuleType::try_from_val(&e, &args.get_unchecked(0))
            .unwrap_or_else(|_| panic_with_error!(&e, RecoveryError::ActionNotAllowed));
        let valid_until = Option::<u32>::try_from_val(&e, &args.get_unchecked(2))
            .unwrap_or_else(|_| panic_with_error!(&e, RecoveryError::ActionNotAllowed));
        let new_signers = Vec::<Signer>::try_from_val(&e, &args.get_unchecked(3))
            .unwrap_or_else(|_| panic_with_error!(&e, RecoveryError::ActionNotAllowed));
        let policies = Map::<Address, Val>::try_from_val(&e, &args.get_unchecked(4))
            .unwrap_or_else(|_| panic_with_error!(&e, RecoveryError::ActionNotAllowed));
        if ctx_type != ContextRuleType::Default
            || valid_until.is_some()
            || !policies.is_empty()
            || new_signers.len() != 1
        {
            panic_with_error!(&e, RecoveryError::ActionNotAllowed);
        }

        // 3. Proposta: existe, é a MESMA chave, e estamos na janela [executable_at, expires_at].
        let key = DataKey::Proposal(smart_account.clone(), context_rule.id);
        let proposal: Proposal = e
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&e, RecoveryError::NoProposal));
        if new_signers.get_unchecked(0) != proposal.signer {
            panic_with_error!(&e, RecoveryError::KeyMismatch);
        }
        let now = e.ledger().timestamp();
        if now < proposal.executable_at {
            panic_with_error!(&e, RecoveryError::TooEarly);
        }
        if now > proposal.expires_at {
            panic_with_error!(&e, RecoveryError::Expired);
        }

        // 4. Consome: uma proposta, uma execução.
        e.storage().persistent().remove(&key);
        RecoveryExecuted {
            smart_account,
            context_rule_id: context_rule.id,
            signer: proposal.signer,
        }
        .publish(&e);
    }

    /// Chamada pela conta em `add_context_rule` / `add_policy`.
    pub fn install(
        e: Env,
        install_params: RecoveryParams,
        context_rule: ContextRule,
        smart_account: Address,
    ) {
        smart_account.require_auth();
        bump_instance(&e);

        if context_rule.context_type != ContextRuleType::CallContract(smart_account.clone()) {
            panic_with_error!(&e, RecoveryError::RuleMustBeCallContractSelf);
        }
        let key = DataKey::Params(smart_account.clone(), context_rule.id);
        if e.storage().persistent().has(&key) {
            panic_with_error!(&e, RecoveryError::AlreadyInstalled);
        }
        if install_params.delay < MIN_DELAY_SECS {
            panic_with_error!(&e, RecoveryError::DelayTooShort);
        }
        for s in context_rule.signers.iter() {
            if !matches!(s, Signer::Delegated(_)) {
                panic_with_error!(&e, RecoveryError::GuardianMustBeDelegated);
            }
        }
        if install_params.threshold == 0 || install_params.threshold > context_rule.signers.len() {
            panic_with_error!(&e, RecoveryError::InvalidThreshold);
        }

        e.storage().persistent().set(&key, &install_params);
        bump(&e, &key);
        RecoveryInstalled {
            smart_account,
            context_rule_id: context_rule.id,
            threshold: install_params.threshold,
            delay: install_params.delay,
        }
        .publish(&e);
    }

    /// Chamada pela conta em `remove_policy` / `remove_context_rule` (via try_uninstall).
    pub fn uninstall(e: Env, context_rule: ContextRule, smart_account: Address) {
        smart_account.require_auth();
        let key = DataKey::Params(smart_account.clone(), context_rule.id);
        if !e.storage().persistent().has(&key) {
            panic_with_error!(&e, RecoveryError::NotInstalled);
        }
        e.storage().persistent().remove(&key);
        e.storage()
            .persistent()
            .remove(&DataKey::Proposal(smart_account.clone(), context_rule.id));
        RecoveryUninstalled { smart_account, context_rule_id: context_rule.id }.publish(&e);
    }

    // ── Recuperação ──

    /// Os guardiões registram a chave nova. Substitui uma proposta pendente (e o
    /// relógio recomeça) — guardião não cancela, então é assim que ele corrige um erro.
    pub fn propose(
        e: Env,
        smart_account: Address,
        context_rule_id: u32,
        signer: Signer,
        approvers: Vec<Address>,
    ) -> Proposal {
        bump_instance(&e);
        let params = params_of(&e, &smart_account, context_rule_id);
        if !matches!(signer, Signer::External(_, _)) {
            panic_with_error!(&e, RecoveryError::SignerMustBeExternal);
        }

        // A regra é lida da CONTA, não de cópia local: se o dono trocou os guardiões,
        // vale a lista de agora. Regra removida → a conta entra em pânico aqui.
        let rule = SmartAccountClient::new(&e, &smart_account).get_context_rule(&context_rule_id);
        if !rule.policies.contains(e.current_contract_address()) {
            panic_with_error!(&e, RecoveryError::NotInstalled);
        }
        let mut seen: Vec<Address> = Vec::new(&e);
        for a in approvers.iter() {
            if seen.contains(&a) {
                panic_with_error!(&e, RecoveryError::DuplicateApprover);
            }
            if !is_guardian(&rule, &a) {
                panic_with_error!(&e, RecoveryError::NotAGuardian);
            }
            a.require_auth();
            seen.push_back(a);
        }
        if seen.len() < params.threshold {
            panic_with_error!(&e, RecoveryError::InsufficientApprovals);
        }

        let now = e.ledger().timestamp();
        let executable_at = now
            .checked_add(params.delay)
            .unwrap_or_else(|| panic_with_error!(&e, RecoveryError::MathOverflow));
        let expires_at = executable_at
            .checked_add(EXECUTION_WINDOW_SECS)
            .unwrap_or_else(|| panic_with_error!(&e, RecoveryError::MathOverflow));
        let proposal = Proposal { signer: signer.clone(), proposed_at: now, executable_at, expires_at };
        let key = DataKey::Proposal(smart_account.clone(), context_rule_id);
        e.storage().persistent().set(&key, &proposal);
        bump(&e, &key);
        RecoveryProposed {
            smart_account,
            context_rule_id,
            signer,
            proposed_at: now,
            executable_at,
            expires_at,
        }
        .publish(&e);
        proposal
    }

    /// Qualquer chave atual do dono cancela (autorização da CONTA — ver doc do módulo).
    pub fn cancel(e: Env, smart_account: Address, context_rule_id: u32) {
        smart_account.require_auth();
        bump_instance(&e);
        let key = DataKey::Proposal(smart_account.clone(), context_rule_id);
        if !e.storage().persistent().has(&key) {
            panic_with_error!(&e, RecoveryError::NoProposal);
        }
        e.storage().persistent().remove(&key);
        RecoveryCancelled { smart_account, context_rule_id }.publish(&e);
    }

    // ── Views ──

    pub fn get_proposal(e: Env, smart_account: Address, context_rule_id: u32) -> Option<Proposal> {
        e.storage().persistent().get(&DataKey::Proposal(smart_account, context_rule_id))
    }

    pub fn get_params(e: Env, smart_account: Address, context_rule_id: u32) -> Option<RecoveryParams> {
        e.storage().persistent().get(&DataKey::Params(smart_account, context_rule_id))
    }

    pub fn min_delay(_e: Env) -> u64 {
        MIN_DELAY_SECS
    }

    pub fn execution_window(_e: Env) -> u64 {
        EXECUTION_WINDOW_SECS
    }

    pub fn version(_e: Env) -> u32 {
        CONTRACT_VERSION
    }
}

#[cfg(test)]
mod test;
