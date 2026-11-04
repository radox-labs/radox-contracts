//! Suíte do guardião com espera. Lógica da policy isolada; a integração com a conta
//! OZ real (e as recusas dela) se prova na testnet — `frontend/guardian-recovery-proof.mjs`.
#![cfg(test)]

use super::*;
use soroban_sdk::testutils::{Address as _, Events as _, Ledger};
use soroban_sdk::{contract, contractimpl, map, vec, Address, Bytes, Env, IntoVal, Val};

/// A conta: só o que a policy lê dela (`get_context_rule`).
#[contract]
pub struct AccountStub;
#[contractimpl]
impl AccountStub {
    pub fn set_rule(e: Env, rule: ContextRule) {
        e.storage().instance().set(&rule.id, &rule);
    }
    pub fn get_context_rule(e: Env, context_rule_id: u32) -> ContextRule {
        e.storage().instance().get(&context_rule_id).unwrap()
    }
}

struct World {
    e: Env,
    policy_id: Address,
    account: Address,
    inst: Address,
    gov: Address,
    rule: ContextRule,
    key_a: Signer,
    key_b: Signer,
}

const RULE_ID: u32 = 1;

fn err(e: RecoveryError) -> soroban_sdk::Error {
    soroban_sdk::Error::from_contract_error(e as u32)
}

fn passkey(e: &Env, tag: u8) -> Signer {
    Signer::External(Address::generate(e), Bytes::from_array(e, &[tag; 65]))
}

fn setup(threshold: u32) -> World {
    let e = Env::default();
    e.mock_all_auths();
    e.ledger().set_timestamp(1_000_000);
    let policy_id = e.register(RecoveryPolicy, ());
    let account = e.register(AccountStub, ());
    let inst = Address::generate(&e);
    let gov = Address::generate(&e);
    let rule = ContextRule {
        context_type: ContextRuleType::CallContract(account.clone()),
        id: RULE_ID,
        name: String::from_str(&e, "guardiao"),
        policies: vec![&e, policy_id.clone()],
        policy_ids: vec![&e, 0],
        signer_ids: vec![&e, 1, 2],
        signers: vec![&e, Signer::Delegated(inst.clone()), Signer::Delegated(gov.clone())],
        valid_until: None,
    };
    AccountStubClient::new(&e, &account).set_rule(&rule);
    RecoveryPolicyClient::new(&e, &policy_id).install(&RecoveryParams { threshold, delay: MIN_DELAY_SECS }, &rule, &account);
    let key_a = passkey(&e, 0xA);
    let key_b = passkey(&e, 0xB);
    World { e, policy_id, account, inst, gov, rule, key_a, key_b }
}

impl World {
    fn policy(&self) -> RecoveryPolicyClient<'_> {
        RecoveryPolicyClient::new(&self.e, &self.policy_id)
    }
    fn both(&self) -> Vec<Address> {
        vec![&self.e, self.inst.clone(), self.gov.clone()]
    }
    fn both_signers(&self) -> Vec<Signer> {
        vec![&self.e, Signer::Delegated(self.inst.clone()), Signer::Delegated(self.gov.clone())]
    }
    fn wait(&self, secs: u64) {
        let t = self.e.ledger().timestamp();
        self.e.ledger().set_timestamp(t + secs);
    }
    /// O contexto que a conta entrega ao `enforce` para `add_context_rule(...)`.
    fn add_rule_ctx(&self, ctx_type: ContextRuleType, valid_until: Option<u32>, signers: Vec<Signer>, policies: Map<Address, Val>) -> Context {
        let args: Vec<Val> = vec![
            &self.e,
            ctx_type.into_val(&self.e),
            String::from_str(&self.e, "recuperada").into_val(&self.e),
            valid_until.into_val(&self.e),
            signers.into_val(&self.e),
            policies.into_val(&self.e),
        ];
        Context::Contract(ContractContext {
            contract: self.account.clone(),
            fn_name: Symbol::new(&self.e, "add_context_rule"),
            args,
        })
    }
    fn add_key_ctx(&self, key: &Signer) -> Context {
        self.add_rule_ctx(ContextRuleType::Default, None, vec![&self.e, key.clone()], Map::new(&self.e))
    }
    fn call_ctx(&self, contract: &Address, fn_name: &str) -> Context {
        Context::Contract(ContractContext {
            contract: contract.clone(),
            fn_name: Symbol::new(&self.e, fn_name),
            args: Vec::new(&self.e),
        })
    }
    fn enforce(&self, ctx: &Context, signers: &Vec<Signer>) -> Result<(), soroban_sdk::Error> {
        match self.policy().try_enforce(ctx, signers, &self.rule, &self.account) {
            Ok(_) => Ok(()),
            Err(Ok(got)) => Err(got),
            Err(Err(got)) => panic!("erro de host inesperado: {got:?}"),
        }
    }
}

// ── install ──

#[test]
fn install_guarda_parametros() {
    let w = setup(2);
    assert_eq!(
        w.policy().get_params(&w.account, &RULE_ID),
        Some(RecoveryParams { threshold: 2, delay: MIN_DELAY_SECS })
    );
}

#[test]
fn install_duas_vezes_recusa() {
    let w = setup(2);
    let r = w.policy().try_install(&RecoveryParams { threshold: 2, delay: MIN_DELAY_SECS }, &w.rule, &w.account);
    assert_eq!(r, Err(Ok(err(RecoveryError::AlreadyInstalled))));
}

#[test]
fn install_recusa_espera_abaixo_do_piso() {
    let w = setup(2);
    let mut rule = w.rule.clone();
    rule.id = 7;
    let r = w.policy().try_install(&RecoveryParams { threshold: 2, delay: MIN_DELAY_SECS - 1 }, &rule, &w.account);
    assert_eq!(r, Err(Ok(err(RecoveryError::DelayTooShort))));
}

#[cfg(feature = "mainnet")]
#[test]
fn mainnet_piso_e_sete_dias() {
    assert_eq!(MIN_DELAY_SECS, 604_800);
    assert_eq!(MIN_DELAY_SECS, PRODUCTION_DELAY_SECS);
}

#[test]
fn install_recusa_regra_que_nao_e_callcontract_self() {
    let w = setup(2);
    for ctx_type in [ContextRuleType::Default, ContextRuleType::CallContract(Address::generate(&w.e))] {
        let mut rule = w.rule.clone();
        rule.id = 8;
        rule.context_type = ctx_type;
        let r = w.policy().try_install(&RecoveryParams { threshold: 1, delay: MIN_DELAY_SECS }, &rule, &w.account);
        assert_eq!(r, Err(Ok(err(RecoveryError::RuleMustBeCallContractSelf))));
    }
}

#[test]
fn install_recusa_guardiao_passkey() {
    let w = setup(2);
    let mut rule = w.rule.clone();
    rule.id = 9;
    rule.signers.push_back(w.key_a.clone());
    let r = w.policy().try_install(&RecoveryParams { threshold: 1, delay: MIN_DELAY_SECS }, &rule, &w.account);
    assert_eq!(r, Err(Ok(err(RecoveryError::GuardianMustBeDelegated))));
}

#[test]
fn install_recusa_limiar_invalido() {
    let w = setup(2);
    for (id, threshold) in [(10u32, 0u32), (11, 3)] {
        let mut rule = w.rule.clone();
        rule.id = id;
        let r = w.policy().try_install(&RecoveryParams { threshold, delay: MIN_DELAY_SECS }, &rule, &w.account);
        assert_eq!(r, Err(Ok(err(RecoveryError::InvalidThreshold))));
    }
}

// ── propose ──

#[test]
fn proposta_exige_os_dois_guardioes_e_emite_evento() {
    let w = setup(2);
    let p = w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    assert_eq!(p.executable_at, p.proposed_at + MIN_DELAY_SECS);
    assert_eq!(p.expires_at, p.executable_at + EXECUTION_WINDOW_SECS);
    // require_auth de CADA guardião, sobre esta chamada exata.
    let auths = w.e.auths();
    assert!(auths.iter().any(|(a, _)| *a == w.inst));
    assert!(auths.iter().any(|(a, _)| *a == w.gov));
    assert_eq!(w.e.events().all().events().len(), 1);
    assert_eq!(w.policy().get_proposal(&w.account, &RULE_ID), Some(p));
}

#[test]
fn proposta_com_um_guardiao_so_recusa() {
    let w = setup(2);
    let r = w.policy().try_propose(&w.account, &RULE_ID, &w.key_a, &vec![&w.e, w.gov.clone()]);
    assert_eq!(r, Err(Ok(err(RecoveryError::InsufficientApprovals))));
}

#[test]
fn proposta_de_estranho_recusa() {
    let w = setup(1);
    let r = w.policy().try_propose(&w.account, &RULE_ID, &w.key_a, &vec![&w.e, Address::generate(&w.e)]);
    assert_eq!(r, Err(Ok(err(RecoveryError::NotAGuardian))));
}

#[test]
fn proposta_com_aprovador_repetido_recusa() {
    let w = setup(2);
    let r = w.policy().try_propose(&w.account, &RULE_ID, &w.key_a, &vec![&w.e, w.gov.clone(), w.gov.clone()]);
    assert_eq!(r, Err(Ok(err(RecoveryError::DuplicateApprover))));
}

#[test]
fn proposta_de_chave_delegated_recusa() {
    let w = setup(2);
    let r = w.policy().try_propose(&w.account, &RULE_ID, &Signer::Delegated(Address::generate(&w.e)), &w.both());
    assert_eq!(r, Err(Ok(err(RecoveryError::SignerMustBeExternal))));
}

#[test]
fn proposta_sem_instalacao_recusa() {
    let w = setup(2);
    let r = w.policy().try_propose(&w.account, &99, &w.key_a, &w.both());
    assert_eq!(r, Err(Ok(err(RecoveryError::NotInstalled))));
}

#[test]
fn nova_proposta_substitui_e_reinicia_o_relogio() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS - 1);
    w.policy().propose(&w.account, &RULE_ID, &w.key_b, &w.both());
    w.wait(1);
    // a espera recomeçou com a B, e a A não existe mais
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_b), &w.both_signers()), Err(err(RecoveryError::TooEarly)));
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::KeyMismatch)));
}

// ── enforce ──

#[test]
fn execucao_antes_da_espera_recusa() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS - 1);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::TooEarly)));
}

#[test]
fn execucao_depois_da_espera_passa_e_consome_a_proposta() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Ok(()));
    assert_eq!(w.policy().get_proposal(&w.account, &RULE_ID), None);
    // replay: a mesma execução de novo não passa
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::NoProposal)));
}

#[test]
fn execucao_sem_proposta_recusa() {
    let w = setup(2);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::NoProposal)));
}

#[test]
fn execucao_de_outra_chave_recusa() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_b), &w.both_signers()), Err(err(RecoveryError::KeyMismatch)));
}

#[test]
fn execucao_com_um_guardiao_ou_nenhum_recusa() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS);
    let so_gov = vec![&w.e, Signer::Delegated(w.gov.clone())];
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &so_gov), Err(err(RecoveryError::InsufficientApprovals)));
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &Vec::new(&w.e)), Err(err(RecoveryError::InsufficientApprovals)));
    // assinatura de fora da regra não conta
    let estranho = vec![&w.e, Signer::Delegated(w.gov.clone()), Signer::Delegated(Address::generate(&w.e))];
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &estranho), Err(err(RecoveryError::InsufficientApprovals)));
}

#[test]
fn guardiao_nao_faz_outra_coisa() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.wait(MIN_DELAY_SECS);
    let ok = w.both_signers();
    // funções da própria conta
    for f in ["execute", "remove_signer", "remove_context_rule", "upgrade", "add_signer", "add_policy", "remove_policy"] {
        assert_eq!(w.enforce(&w.call_ctx(&w.account, f), &ok), Err(err(RecoveryError::ActionNotAllowed)), "{f}");
    }
    // outro contrato (USDC, venda) — a conta já recusa pelo tipo da regra; aqui é a 2ª trava
    let usdc = Address::generate(&w.e);
    assert_eq!(w.enforce(&w.call_ctx(&usdc, "transfer"), &ok), Err(err(RecoveryError::ActionNotAllowed)));
    assert_eq!(w.enforce(&w.call_ctx(&usdc, "add_context_rule"), &ok), Err(err(RecoveryError::ActionNotAllowed)));
    // add_context_rule com forma diferente da proposta
    let um = vec![&w.e, w.key_a.clone()];
    let cases = [
        w.add_rule_ctx(ContextRuleType::CallContract(usdc.clone()), None, um.clone(), Map::new(&w.e)),
        w.add_rule_ctx(ContextRuleType::Default, Some(123), um.clone(), Map::new(&w.e)),
        w.add_rule_ctx(ContextRuleType::Default, None, vec![&w.e, w.key_a.clone(), w.key_b.clone()], Map::new(&w.e)),
        w.add_rule_ctx(ContextRuleType::Default, None, um.clone(), map![&w.e, (usdc.clone(), 1u32.into_val(&w.e))]),
    ];
    for c in cases.iter() {
        assert_eq!(w.enforce(c, &ok), Err(err(RecoveryError::ActionNotAllowed)));
    }
    // nada disso consumiu a proposta
    assert!(w.policy().get_proposal(&w.account, &RULE_ID).is_some());
}

// ── janela de execução ──

#[test]
fn execucao_exatamente_no_fim_da_janela_passa() {
    let w = setup(2);
    let p = w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.e.ledger().set_timestamp(p.expires_at);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Ok(()));
}

#[test]
fn execucao_um_segundo_depois_da_janela_recusa() {
    let w = setup(2);
    let p = w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.e.ledger().set_timestamp(p.expires_at + 1);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::Expired)));
    // vencida não é consumida às escondidas: segue lá até ser substituída ou cancelada
    assert_eq!(w.policy().get_proposal(&w.account, &RULE_ID), Some(p));
}

#[test]
fn proposta_nova_depois_de_vencida_reabre_o_ciclo() {
    let w = setup(2);
    let p = w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.e.ledger().set_timestamp(p.expires_at + 1);
    let p2 = w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::TooEarly)));
    w.e.ledger().set_timestamp(p2.executable_at);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Ok(()));
}

#[cfg(feature = "mainnet")]
#[test]
fn mainnet_janela_e_trinta_dias() {
    assert_eq!(EXECUTION_WINDOW_SECS, 2_592_000);
    assert_eq!(EXECUTION_WINDOW_SECS, PRODUCTION_EXECUTION_WINDOW_SECS);
}

// ── cancel / uninstall ──

#[test]
fn dono_cancela_e_a_execucao_morre() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.policy().cancel(&w.account, &RULE_ID);
    // quem autorizou o cancel foi a CONTA (regra do dono), não um guardião
    let auths = w.e.auths();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].0, w.account);
    w.wait(MIN_DELAY_SECS);
    assert_eq!(w.enforce(&w.add_key_ctx(&w.key_a), &w.both_signers()), Err(err(RecoveryError::NoProposal)));
    assert_eq!(w.policy().try_cancel(&w.account, &RULE_ID), Err(Ok(err(RecoveryError::NoProposal))));
}

#[test]
fn uninstall_apaga_parametros_e_proposta() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    w.policy().uninstall(&w.rule, &w.account);
    assert_eq!(w.policy().get_params(&w.account, &RULE_ID), None);
    assert_eq!(w.policy().get_proposal(&w.account, &RULE_ID), None);
    assert_eq!(w.policy().try_uninstall(&w.rule, &w.account), Err(Ok(err(RecoveryError::NotInstalled))));
}

#[test]
fn carteiras_nao_se_misturam() {
    let w = setup(2);
    w.policy().propose(&w.account, &RULE_ID, &w.key_a, &w.both());
    let outra = Address::generate(&w.e);
    assert_eq!(w.policy().get_proposal(&outra, &RULE_ID), None);
    assert_eq!(w.policy().try_cancel(&outra, &RULE_ID), Err(Ok(err(RecoveryError::NoProposal))));
}

#[test]
fn view_de_versao_e_piso() {
    let w = setup(1);
    assert_eq!(w.policy().version(), 2);
    assert_eq!(w.policy().min_delay(), MIN_DELAY_SECS);
    assert_eq!(w.policy().execution_window(), EXECUTION_WINDOW_SECS);
}
