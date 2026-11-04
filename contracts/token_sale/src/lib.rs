#![no_std]
//! token_sale v11 (06/10/2026) = v10 + as regras da Res. CVM 88 no modo ESCROW:
//!
//!   - **Desistência** (`withdraw_commitment`): o investidor recebe de volta TODO o que pôs, enquanto
//!     `agora < última compra dele + janela` (`Config.withdraw_window_secs`; 0 = sem desistência),
//!     inclusive depois do prazo da oferta. Quem desiste devolve a vaga: `buy` já usa `alvo − sold`,
//!     nenhum passo extra reabre a vaga.
//!   - **Fechamento espera as janelas**: `close` exige `sold ≥ mínimo`, `agora ≥ fim da última janela`
//!     e (`sold == alvo` OU `agora ≥ prazo`); `fail` exige prazo E última janela vencidos e `sold < mínimo`.
//!   - **Mínimo ≥ 2/3** (`Config.two_thirds_floor`): `set_active` recusa `3 × min < 2 × alvo`.
//!   - **Suspensão** (`suspend` / `resume_sale`, art. 7º §1º): bloqueia `buy`, abre a revogação de TODOS
//!     até `revoke_until_ts` (o backend calcula o 5º dia útil; o contrato só compara timestamp).
//!   - **`emergency_pauser`** + pausa, desenho do v5 dos outros três contratos (mesmos símbolos de
//!     evento). A pausa PARA os prazos: `deadline_ts`, a janela de cada investidor e a revogação são
//!     estendidos pelo tempo pausado, em O(1) (`PauseAcc` + `PausedSince`; ver `pause_total`).
//!
//! O modo DIRECT segue como no v10. Layout novo (`LAYOUT_VERSION` 2: `Config` ganhou dois campos) —
//! instância NOVA: uma oferta v10 viva fica no WASM em que nasceu (um `upgrade` de v10 para v11
//! quebraria a leitura do `Config` antigo; não é um caminho suportado).
//!
//! ── Histórico: v10 — venda primária de RWA, andaime de SALA LIMPA (24/08/2026) ──
//!
//! Decisão do Pedro (24/08): o v9.1 (`6f3576c5…`, nunca deployado) virou a ESPECIFICAÇÃO —
//! mesma máquina de estados, ratificada por ele e provada na testnet (20/20) — e o andaime
//! foi reescrito conforme o desenho de sala limpa (rodada 4 da auditoria externa):
//!
//!   - **`upgrade()` EXISTE — e é a válvula da Ledger** (`lib.rs:503`). O andaime nasceu
//!     SEM ele em 24/08 ("o código não muda com dinheiro dentro", por ausência do entry
//!     point), e o Pedro REVOGOU essa decisão no mesmo dia, depois do red-team achar o
//!     deadlock "fechável mas infechável": é justamente com USDC escrowado e a rodada
//!     presa que a Ledger precisa poder trocar o código. Só `admin`, que em mainnet é a
//!     Ledger (2-de-3), nunca hot wallet. `cancel()` → refunds → instância nova segue
//!     sendo a saída normal; o upgrade é a de emergência.
//!     [corrigido 07/09: este cabeçalho ainda dizia "NÃO EXISTE" e contradizia a própria
//!      linha 503 — mesma prosa errada que a PENDENCIAS 182 corrigiu no MANIFEST.txt,
//!      mas só lá. Ver `deploy/mainnet-wasm/MANIFEST.txt:103`.]
//!   - **Estado único**: `Status` (Raising/Closed/Failed/Canceled) + structs `Config`
//!     (imutável) e `Terms` (ajustável) — nada de chaves soltas onde ausência é valor.
//!   - **`__constructor`** atômico com o deploy (Protocolo 22) — sem janela de init.
//!   - **Prazo em `timestamp()`** — sequence deriva do block time, que o protocolo retuna.
//!   - **Preço em campo ÚNICO**: `price` = stroops de USDC por 1 título inteiro
//!     (paridade = 10_000_000). Orientação no nome; compra com resto ≠ 0 é REJEITADA
//!     (mata a classe inteira do bug de inversão — PEND 158 — e torna `claim` exato).
//!   - **Sem campo de fee**: o investidor não paga nada, por construção (decisão 22/08).
//!   - Eventos via `#[contractevent]`.
//!
//! Regras de produto (ratificadas 23/08): dinheiro retido até a rodada resolver; fecha cheia
//! a qualquer hora OU no prazo com captado ≥ piso; abaixo do piso no prazo, falha e cada
//! investidor se reembolsa sozinho; entre piso e total, só no prazo; só carteira AUTORIZADA
//! no SAC entra; congelado retém compra, entrega e devolução; admin (chave quente) só
//! cancela/congela/ajusta ticket-teto — nenhum caminho até o dinheiro; piso e prazo
//! imutáveis por AUSÊNCIA de setter. Modo `direct` (contínuo) no mesmo binário, opt-in.
//!
//! Instâncias são imutáveis: uma oferta viva fica no WASM em que nasceu, para sempre.
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, BytesN, Env,
};

const CONTRACT_VERSION: u32 = 11;
const LAYOUT_VERSION: u32 = 2;
const STROOP: i128 = 10_000_000;
const TTL_THRESHOLD: u32 = 518_400;
const TTL_EXTEND: u32 = 518_400;

pub const MODE_DIRECT: u32 = 0;
pub const MODE_ESCROW: u32 = 1;

pub const STATUS_RAISING: u32 = 0;
pub const STATUS_CLOSED: u32 = 1;
pub const STATUS_FAILED: u32 = 2;
pub const STATUS_CANCELED: u32 = 3;

// ── F-006: USDC canônico por rede (mesmo gate desde o v7; PEND 159 guarda as features) ──
#[cfg(feature = "testnet")]
const USDC_SAC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
#[cfg(feature = "mainnet")]
const USDC_SAC: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";
#[cfg(all(feature = "testing", any(feature = "testnet", feature = "mainnet")))]
compile_error!("feature `testing` junto com `testnet`/`mainnet`: use --no-default-features --features testnet|mainnet");
#[cfg(all(feature = "testnet", feature = "mainnet"))]
compile_error!("features `testnet` e `mainnet` sao mutuamente exclusivas");
#[cfg(not(any(feature = "testing", feature = "testnet", feature = "mainnet")))]
compile_error!("escolha uma rede: --features testnet|mainnet (ou o default `testing` para cargo test)");

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SaleError {
    InvalidAmount = 1,
    NotActive = 2,
    WrongState = 3,
    Expired = 4,
    BelowMinTicket = 5,
    CapExceeded = 6,
    FrozenInvestor = 7,
    NotAuthorizedHolder = 8,
    ExceedsRemaining = 9,
    /// v10: compra cujo custo em USDC não é exato (tokens × price % 10^7 ≠ 0).
    RemainderNotZero = 10,
    TargetNotReached = 11,
    NotExpired = 12,
    NothingToClaim = 13,
    MinRaiseAboveTarget = 14,
    DeadlineRequired = 15,
    UnauthorizedToken = 16,
    Overflow = 17,
    /// O saldo de títulos não lastreia todas as entregas pendentes.
    TokensShort = 18,
    InvalidMode = 19,
    NotYetActivated = 20,
    TargetNotSellable = 21,
    /// v11: oferta CVM 88 com `3 × min_raise < 2 × alvo` (art. 5º, III).
    MinRaiseBelowTwoThirds = 22,
    /// v11: desistência fora da janela do investidor (e sem revogação aberta).
    WithdrawWindowClosed = 23,
    /// v11: oferta suspensa (`buy`), ou suspensão/revogação ainda aberta (`close`/`fail`).
    Suspended = 24,
    /// v11: `resume_sale` sem suspensão ativa.
    NotSuspended = 25,
    /// v11: `suspend` com `revoke_until_ts <= agora`.
    InvalidRevokeUntil = 26,
    /// v11: contrato pausado.
    ContractPaused = 27,
    /// v11: `emergency_pause`/`renounce_emergency_pauser` sem `emergency_pauser` configurado.
    NoEmergencyPauser = 28,
    /// v11: `close`/`fail` com a janela de desistência de alguém ainda aberta.
    WithdrawWindowOpen = 29,
}

/// Imutável desde o deploy. Piso (`min_raise`) e prazo (`deadline_ts`) não têm setter —
/// mudar o combinado com quem já entrou não é uma função, é `cancel()` + oferta nova.
#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub mode: u32,          // MODE_DIRECT | MODE_ESCROW
    pub admin: Address,     // chave quente: cancel / freeze / terms — nunca o dinheiro
    pub issuer: Address,    // emissora: ativa a oferta, recebe sobras de títulos
    pub sell_token: Address,
    pub buy_token: Address, // USDC canônico (F-006)
    pub company: Address,   // recebe o captado no close (escrow) ou por compra (direct)
    /// Stroops de USDC por 1 título inteiro. Paridade = 10_000_000.
    pub price: i128,
    /// Piso da rodada em stroops de USDC. 0 = exige 100% (o backend proíbe 0 no form;
    /// on-chain o 0 é o fallback mais estrito). Só tem significado em escrow.
    pub min_raise: i128,
    /// Unix timestamp (segundos). Obrigatório > 0 em escrow.
    pub deadline_ts: u64,
    pub layout_version: u32,
    /// v11: janela de desistência em segundos, medida da ÚLTIMA compra do investidor.
    /// 0 = sem desistência (emissor estrangeiro). CVM 88: o backend passa ≥ 432000 (5 dias).
    pub withdraw_window_secs: u64,
    /// v11: CVM 88 art. 5º, III — `set_active` exige `3 × min_raise ≥ 2 × alvo` (min 0 = 100% passa).
    pub two_thirds_floor: bool,
}

/// v11: carimbo da última compra do investidor — o instante e o tempo pausado acumulado nele.
/// A janela efetiva é `ts + janela + (pausa_total_agora − acc)`: a pausa não consome a janela.
#[contracttype]
#[derive(Clone)]
pub struct BuyStamp {
    pub ts: u64,
    pub acc: u64,
}

/// Ajustável pelo admin durante a vida da oferta (E23): limites de entrada, em stroops de USDC.
#[contracttype]
#[derive(Clone)]
pub struct Terms {
    pub min_ticket: i128,    // custo mínimo por compra (0 = sem mínimo)
    pub max_per_buyer: i128, // custo acumulado máximo por investidor (0 = sem teto)
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Terms,
    Status,          // u32 STATUS_* — fonte única do estado da rodada
    Active,          // bool — vendas abertas (as duas modalidades)
    Target,          // i128 títulos ofertados, congelado na 1ª ativação (escrow)
    Sold,            // i128 títulos vendidos (escrow; v11: desce na desistência)
    SoldRemaining,   // i128 títulos vendidos ainda não entregues/reembolsados
    Raised,          // i128 USDC captado retido (desce em refund e na desistência)
    Contrib(Address), // i128 USDC do investidor retido — persistent
    Spent(Address),   // i128 custo acumulado por investidor (teto) — persistent
    Frozen(Address),  // bool — persistent
    // ── v11 (chaves novas; nenhuma existente mudou de significado) ──
    LastBuy(Address), // BuyStamp da última compra do investidor (escrow, janela > 0) — persistent
    LastWindowBase,   // u64: max(ts + janela − acc) da oferta; fim da última janela = base + pausa_total
    Paused,           // bool
    PausedSince,      // u64: início da pausa corrente
    PauseAcc,         // u64: segundos pausados já fechados (soma no `resume`)
    EmergencyPauser,  // Address — opcional
    Suspended,        // bool — compra bloqueada (art. 7º §1º)
    RevokeBase,       // u64: revoke_until_ts − pausa_total na hora do `suspend` (max entre suspensões)
}

// ── Eventos (#[contractevent] — andaime novo, rodada 4) ──
#[contractevent(topics = ["buy"])]
pub struct BuyEvent {
    pub investor: Address,
    pub tokens: i128,
    pub cost: i128,
}
#[contractevent(topics = ["close"])]
pub struct CloseEvent {
    pub raised: i128,
}
#[contractevent(topics = ["fail"])]
pub struct FailEvent {
    pub raised: i128,
}
#[contractevent(topics = ["cancel"])]
pub struct CancelEvent {
    pub raised: i128,
}
#[contractevent(topics = ["refund"])]
pub struct RefundEvent {
    pub investor: Address,
    pub amount: i128,
}
#[contractevent(topics = ["claim"])]
pub struct ClaimEvent {
    pub investor: Address,
    pub tokens: i128,
}
#[contractevent(topics = ["freeze"])]
pub struct FreezeEvent {
    pub investor: Address,
    pub frozen: bool,
}
#[contractevent(topics = ["terms"])]
pub struct TermsEvent {
    pub min_ticket: i128,
    pub max_per_buyer: i128,
}
#[contractevent(topics = ["active"])]
pub struct ActiveEvent {
    pub active: bool,
    pub target: i128,
}
/// v11 — desistência: o investidor recebeu `amount` de USDC e devolveu `tokens` títulos à vaga.
#[contractevent(topics = ["withdrawn"])]
pub struct WithdrawnEvent {
    pub investor: Address,
    pub amount: i128,
    pub tokens: i128,
}
/// v11 — suspensão: dado = `revoke_until_ts` pedido pelo backend (instante do relógio da rede).
#[contractevent(topics = ["suspended"], data_format = "single-value")]
pub struct SuspendedEvent {
    pub revoke_until: u64,
}
/// v11 — fim da suspensão. Dado: `true`.
#[contractevent(topics = ["resumedsl"], data_format = "single-value")]
pub struct SaleResumedEvent {
    pub resumed: bool,
}
// Pausa e pauser: MESMOS tópicos e formatos dos v5 (transfer_agent) — o indexador lê `paused`.
#[contractevent(topics = ["paused"], data_format = "single-value")]
pub struct PausedEvent {
    pub paused: bool,
}
#[contractevent(topics = ["resumed"], data_format = "single-value")]
pub struct ResumedEvent {
    pub resumed: bool,
}
#[contractevent(topics = ["emergpaus"], data_format = "single-value")]
pub struct EmergencyPausedEvent {
    pub pauser: Address,
}
#[contractevent(topics = ["pauserset"], data_format = "vec")]
pub struct PauserSetEvent {
    pub previous: Option<Address>,
    pub new: Option<Address>,
}
#[contractevent(topics = ["pauserren"], data_format = "single-value")]
pub struct PauserRenouncedEvent {
    pub pauser: Address,
}

#[contract]
pub struct TokenSale;

#[contractimpl]
impl TokenSale {
    /// Nasce pronta e válida, atômica com o deploy (Protocolo 22). Sem janela de init.
    pub fn __constructor(e: Env, config: Config) -> Result<(), SaleError> {
        if config.mode > MODE_ESCROW {
            return Err(SaleError::InvalidMode);
        }
        if config.price <= 0 {
            return Err(SaleError::InvalidAmount);
        }
        if config.min_raise < 0 || (config.mode == MODE_DIRECT && config.min_raise != 0) {
            return Err(SaleError::InvalidAmount);
        }
        // Escrow sem prazo é armadilha: dinheiro entra e nenhum evento autoriza devolver (D4).
        if config.mode == MODE_ESCROW && config.deadline_ts == 0 {
            return Err(SaleError::DeadlineRequired);
        }
        // v11: desistência e 2/3 só existem em escrow; em direct o dinheiro já foi à empresa.
        if config.mode == MODE_DIRECT && (config.withdraw_window_secs != 0 || config.two_thirds_floor) {
            return Err(SaleError::InvalidAmount);
        }
        validate_canonical_usdc(&e, &config.buy_token)?;
        config.admin.require_auth();
        let mut cfg = config.clone();
        cfg.layout_version = LAYOUT_VERSION;
        e.storage().instance().set(&DataKey::Config, &cfg);
        e.storage().instance().set(
            &DataKey::Terms,
            &Terms { min_ticket: 0, max_per_buyer: 0 },
        );
        e.storage().instance().set(&DataKey::Status, &STATUS_RAISING);
        e.storage().instance().set(&DataKey::Active, &false);
        e.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
        Ok(())
    }

    /// Liga/desliga as vendas. Na 1ª ativação em escrow, congela o alvo a partir do
    /// depósito de títulos e valida o piso contra ele. Emissora assina.
    pub fn set_active(e: Env, active: bool) -> Result<(), SaleError> {
        let cfg = config(&e);
        cfg.issuer.require_auth();
        let mut target = target(&e);
        if active && cfg.mode == MODE_ESCROW && target == 0 {
            let balance = token::Client::new(&e, &cfg.sell_token).balance(&e.current_contract_address());
            if balance <= 0 {
                return Err(SaleError::NotYetActivated);
            }
            let target_usdc = usdc_cost(balance, cfg.price)?;
            if cfg.min_raise > target_usdc {
                return Err(SaleError::MinRaiseAboveTarget);
            }
            // v11, CVM 88 art. 5º, III: mínimo ≥ 2/3 do máximo ⇔ 3 × min ≥ 2 × alvo (em USDC).
            // min 0 = 100% do alvo e passa. A conta em inteiros: sem divisão, sem arredondar a favor.
            if cfg.two_thirds_floor
                && cfg.min_raise > 0
                && cfg.min_raise.checked_mul(3).ok_or(SaleError::Overflow)?
                    < target_usdc.checked_mul(2).ok_or(SaleError::Overflow)?
            {
                return Err(SaleError::MinRaiseBelowTwoThirds);
            }
            // Red-team 24/08 (LOW): um alvo cujo `balance*price` nao e multiplo de
            // 10^7 tem um rabo INCOMPRAVEL (buy exige resto zero) — `sold == target`
            // fica inalcançavel e a rodada nasce condenada a falhar no prazo, sem
            // ninguem perceber ate la. Rejeitar na ativacao mata a classe inteira.
            if balance.checked_mul(cfg.price).ok_or(SaleError::Overflow)? % STROOP != 0 {
                return Err(SaleError::TargetNotSellable);
            }
            target = balance;
            e.storage().instance().set(&DataKey::Target, &target);
        }
        e.storage().instance().set(&DataKey::Active, &active);
        ActiveEvent { active, target }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Compra `tokens` títulos (stroops). O custo em USDC é EXATO por construção:
    /// custo = tokens × price / 10^7, e resto ≠ 0 é rejeitado — sem poeira, sem
    /// ambiguidade de orientação, e `claim` vira aritmética exata.
    pub fn buy(e: Env, investor: Address, tokens: i128) -> Result<(), SaleError> {
        investor.require_auth();
        if tokens <= 0 {
            return Err(SaleError::InvalidAmount);
        }
        require_not_paused(&e)?;
        let cfg = config(&e);
        if !active(&e) {
            return Err(SaleError::NotActive);
        }
        if is_suspended(&e) {
            return Err(SaleError::Suspended);
        }
        if is_frozen(&e, &investor) {
            return Err(SaleError::FrozenInvestor);
        }
        let now = e.ledger().timestamp();
        // v11: o prazo é o efetivo — `deadline_ts` + o tempo que o contrato ficou pausado.
        if cfg.deadline_ts > 0 && now >= deadline_eff(&e, &cfg) {
            return Err(SaleError::Expired);
        }
        let cost = usdc_cost_exact(tokens, cfg.price)?;
        let mut closes_round = false;
        if cfg.mode == MODE_ESCROW {
            if status(&e) != STATUS_RAISING {
                return Err(SaleError::WrongState);
            }
            let tgt = target(&e);
            if tgt == 0 {
                return Err(SaleError::NotYetActivated);
            }
            let remaining = tgt - sold(&e);
            if tokens > remaining {
                return Err(SaleError::ExceedsRemaining);
            }
            closes_round = tokens == remaining;
            // Só carteira AUTORIZADA no SAC entra na rodada: em direct a transferência do
            // título já força; em escrow a entrega é no claim — sem este gate, dinheiro
            // sem KYC entraria pelo RPC e contaria para o piso (varredura 23/08).
            if !token::StellarAssetClient::new(&e, &cfg.sell_token).authorized(&investor) {
                return Err(SaleError::NotAuthorizedHolder);
            }
        }
        let terms = terms(&e);
        // D7: a compra que FECHA a rodada é isenta do ticket mínimo — senão uma cauda
        // sub-mínimo encalha e a rodada só fecha por sorte.
        if terms.min_ticket > 0 && cost < terms.min_ticket && !closes_round {
            return Err(SaleError::BelowMinTicket);
        }
        let spent = spent(&e, &investor);
        let new_spent = spent.checked_add(cost).ok_or(SaleError::Overflow)?;
        if terms.max_per_buyer > 0 && new_spent > terms.max_per_buyer {
            return Err(SaleError::CapExceeded);
        }
        set_spent(&e, &investor, new_spent);
        let usdc = token::Client::new(&e, &cfg.buy_token);
        let contract = e.current_contract_address();
        if cfg.mode == MODE_ESCROW {
            // Efeitos antes das interações.
            set_contrib(&e, &investor, contrib(&e, &investor).checked_add(cost).ok_or(SaleError::Overflow)?);
            e.storage().instance().set(&DataKey::Sold, &(sold(&e).checked_add(tokens).ok_or(SaleError::Overflow)?));
            e.storage().instance().set(
                &DataKey::SoldRemaining,
                &(sold_remaining(&e).checked_add(tokens).ok_or(SaleError::Overflow)?),
            );
            e.storage().instance().set(&DataKey::Raised, &(raised(&e).checked_add(cost).ok_or(SaleError::Overflow)?));
            stamp_buy(&e, &cfg, &investor, now);
            usdc.transfer(&investor, &contract, &cost);
        } else {
            // Direct: entrega e pagamento imediatos, atômicos.
            usdc.transfer(&investor, &contract, &cost);
            token::Client::new(&e, &cfg.sell_token).transfer(&contract, &investor, &tokens);
            usdc.transfer(&contract, &cfg.company, &cost);
        }
        BuyEvent { investor, tokens, cost }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Permissionless: fecha quando `sold ≥ mínimo`, a última janela de desistência já venceu e a
    /// rodada está cheia OU vencida (v11). Suspensa/em revogação, não fecha. Paga à empresa
    /// exatamente o CAPTADO. Exige lastro para todas as entregas pendentes.
    pub fn close(e: Env) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        require_not_paused(&e)?;
        match status(&e) {
            STATUS_CLOSED => return Ok(()), // idempotente (D15)
            STATUS_RAISING => {}
            _ => return Err(SaleError::WrongState),
        }
        if is_suspended(&e) || in_revocation(&e) {
            return Err(SaleError::Suspended);
        }
        if !floor_and_full_or_expired(&e, &cfg) {
            return Err(SaleError::TargetNotReached);
        }
        // Art. 5º §2º: o dinheiro só vai à empresa depois que ninguém mais pode desistir.
        if e.ledger().timestamp() < last_window_end_eff(&e) {
            return Err(SaleError::WithdrawWindowOpen);
        }
        let contract = e.current_contract_address();
        let sell_balance = token::Client::new(&e, &cfg.sell_token).balance(&contract);
        if sell_balance < sold_remaining(&e) {
            return Err(SaleError::TokensShort);
        }
        let amount = raised(&e);
        e.storage().instance().set(&DataKey::Status, &STATUS_CLOSED);
        e.storage().instance().set(&DataKey::Active, &false);
        e.storage().instance().set(&DataKey::Raised, &0i128);
        token::Client::new(&e, &cfg.buy_token).transfer(&contract, &cfg.company, &amount);
        CloseEvent { raised: amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Permissionless: prazo E última janela vencidos com `sold < mínimo` → falha, abre os
    /// reembolsos. Desistências depois do prazo podem derrubar a oferta abaixo do mínimo (v11).
    pub fn fail(e: Env) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        require_not_paused(&e)?;
        match status(&e) {
            STATUS_FAILED => return Ok(()), // idempotente
            STATUS_RAISING => {}
            _ => return Err(SaleError::WrongState),
        }
        if is_suspended(&e) || in_revocation(&e) {
            return Err(SaleError::Suspended);
        }
        let now = e.ledger().timestamp();
        if now < deadline_eff(&e, &cfg) {
            return Err(SaleError::NotExpired);
        }
        if now < last_window_end_eff(&e) {
            return Err(SaleError::WithdrawWindowOpen);
        }
        if floor_reached(&e, &cfg) {
            // Rodada que bateu o mínimo pertence aos investidores — a única saída é close().
            return Err(SaleError::WrongState);
        }
        e.storage().instance().set(&DataKey::Status, &STATUS_FAILED);
        e.storage().instance().set(&DataKey::Active, &false);
        FailEvent { raised: raised(&e) }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Admin: cancela uma rodada aberta — todo investidor recupera tudo; nada vai ao admin.
    /// Recusado quando a rodada já é fechável (ela pertence aos investidores): no v11 isso é o
    /// `close` aceito AGORA, então uma rodada cheia que ainda espera as janelas (ou está suspensa)
    /// PODE ser cancelada. NÃO é bloqueado pela pausa: cancelar é proteção do investidor e não
    /// move dinheiro (o `refund` é que espera o `resume`).
    pub fn cancel(e: Env) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        cfg.admin.require_auth();
        match status(&e) {
            STATUS_CANCELED => return Ok(()), // idempotente
            STATUS_RAISING => {}
            _ => return Err(SaleError::WrongState),
        }
        if closeable_now(&e, &cfg) {
            return Err(SaleError::WrongState);
        }
        e.storage().instance().set(&DataKey::Status, &STATUS_CANCELED);
        e.storage().instance().set(&DataKey::Active, &false);
        CancelEvent { raised: raised(&e) }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Permissionless; paga SEMPRE ao investidor registrado (quem chama só doa o gás).
    pub fn refund(e: Env, investor: Address) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        require_not_paused(&e)?;
        let st = status(&e);
        if st != STATUS_FAILED && st != STATUS_CANCELED {
            return Err(SaleError::WrongState);
        }
        if is_frozen(&e, &investor) {
            return Err(SaleError::FrozenInvestor); // decisão do Pedro 23/08: congelado retém tudo
        }
        let amount = contrib(&e, &investor);
        if amount <= 0 {
            return Err(SaleError::NothingToClaim);
        }
        // Efeitos antes da interação.
        set_contrib(&e, &investor, 0);
        let tokens = amount * STROOP / cfg.price; // exato: cada compra teve resto zero
        e.storage().instance().set(&DataKey::SoldRemaining, &(sold_remaining(&e) - tokens));
        e.storage().instance().set(&DataKey::Raised, &(raised(&e) - amount));
        token::Client::new(&e, &cfg.buy_token).transfer(&e.current_contract_address(), &investor, &amount);
        RefundEvent { investor, amount }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Permissionless; entrega os títulos do investidor registrado após o fechamento.
    /// Quantidade = contrib × 10^7 / price — exata porque toda compra teve resto zero.
    pub fn claim(e: Env, investor: Address) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        require_not_paused(&e)?;
        if status(&e) != STATUS_CLOSED {
            return Err(SaleError::WrongState);
        }
        if is_frozen(&e, &investor) {
            return Err(SaleError::FrozenInvestor);
        }
        let amount = contrib(&e, &investor);
        if amount <= 0 {
            return Err(SaleError::NothingToClaim);
        }
        let tokens = amount * STROOP / cfg.price;
        set_contrib(&e, &investor, 0);
        e.storage().instance().set(&DataKey::SoldRemaining, &(sold_remaining(&e) - tokens));
        token::Client::new(&e, &cfg.sell_token).transfer(&e.current_contract_address(), &investor, &tokens);
        ClaimEvent { investor, tokens }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Admin: sobras de TÍTULOS de volta à EMISSORA — nunca USDC, nunca destino livre.
    /// Em Raising ativada os títulos lastreiam a rodada; em Closed só sai o excedente
    /// acima das entregas pendentes; em Failed/Canceled os títulos estão livres.
    pub fn withdraw(e: Env, amount: i128) -> Result<(), SaleError> {
        let cfg = config(&e);
        cfg.admin.require_auth();
        require_not_paused(&e)?;
        if amount <= 0 {
            return Err(SaleError::InvalidAmount);
        }
        let contract = e.current_contract_address();
        let sell = token::Client::new(&e, &cfg.sell_token);
        if cfg.mode == MODE_ESCROW {
            match status(&e) {
                STATUS_RAISING if target(&e) > 0 => return Err(SaleError::WrongState),
                STATUS_CLOSED => {
                    let free = sell.balance(&contract) - sold_remaining(&e);
                    if amount > free {
                        return Err(SaleError::ExceedsRemaining);
                    }
                }
                _ => {}
            }
        }
        sell.transfer(&contract, &cfg.issuer, &amount);
        bump(&e);
        Ok(())
    }

    /// Admin: congela/descongela um investidor. Congelado não compra, não recebe, não
    /// é reembolsado até descongelar (decisão do Pedro, 23/08).
    pub fn set_frozen(e: Env, investor: Address, frozen: bool) -> Result<(), SaleError> {
        let cfg = config(&e);
        cfg.admin.require_auth();
        let key = DataKey::Frozen(investor.clone());
        e.storage().persistent().set(&key, &frozen);
        e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
        FreezeEvent { investor, frozen }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Admin: ajusta ticket mínimo e teto por investidor (E23). Piso e prazo NÃO — não
    /// existe função que os alcance.
    pub fn set_terms(e: Env, min_ticket: i128, max_per_buyer: i128) -> Result<(), SaleError> {
        let cfg = config(&e);
        cfg.admin.require_auth();
        if min_ticket < 0 || max_per_buyer < 0 {
            return Err(SaleError::InvalidAmount);
        }
        e.storage().instance().set(&DataKey::Terms, &Terms { min_ticket, max_per_buyer });
        TermsEvent { min_ticket, max_per_buyer }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// v11 — DESISTÊNCIA (CVM 88 art. 3º, III). O investidor (assina ele) recebe de volta TODO o
    /// USDC que pôs enquanto `agora < fim da janela dele` — `última compra + janela`, esticada pelo
    /// tempo pausado — ou, com suspensão/revogação aberta, até `revoke_until`. Vale depois do prazo
    /// da oferta: a desistência é direito do investidor, não da oferta. Desfaz a posição inteira:
    /// zera `contrib` e o `spent` (o teto por investidor volta) e desconta `sold`, `sold_remaining`
    /// e `raised` — a vaga volta à venda sem passo extra (`buy` usa `alvo − sold`).
    /// Congelado retém (mesma regra do `refund`). Só em RAISING; bloqueada pela pausa — as janelas
    /// são esticadas por ela, ninguém perde o direito.
    pub fn withdraw_commitment(e: Env, investor: Address) -> Result<(), SaleError> {
        investor.require_auth();
        let cfg = escrow_only(&e)?;
        require_not_paused(&e)?;
        if status(&e) != STATUS_RAISING {
            return Err(SaleError::WrongState);
        }
        if is_frozen(&e, &investor) {
            return Err(SaleError::FrozenInvestor);
        }
        let amount = contrib(&e, &investor);
        if amount <= 0 {
            return Err(SaleError::NothingToClaim);
        }
        if e.ledger().timestamp() >= withdraw_until(&e, &cfg, &investor) {
            return Err(SaleError::WithdrawWindowClosed);
        }
        // Efeitos antes da interação. `tokens` é exato: cada compra teve resto zero.
        let tokens = amount * STROOP / cfg.price;
        set_contrib(&e, &investor, 0);
        set_spent(&e, &investor, 0);
        e.storage().instance().set(&DataKey::Sold, &(sold(&e) - tokens));
        e.storage().instance().set(&DataKey::SoldRemaining, &(sold_remaining(&e) - tokens));
        e.storage().instance().set(&DataKey::Raised, &(raised(&e) - amount));
        token::Client::new(&e, &cfg.buy_token).transfer(&e.current_contract_address(), &investor, &amount);
        WithdrawnEvent { investor, amount, tokens }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// v11 — SUSPENSÃO (CVM 88 art. 7º §1º). Admin. Bloqueia `buy` (`Suspended`) e abre a revogação
    /// de TODOS os investidores até `revoke_until_ts` (o backend calcula o 5º dia útil; o contrato
    /// só compara timestamp). Enquanto suspensa e até `revoke_until`, `close`/`fail` são recusados.
    /// A revogação NÃO termina no `resume_sale`: é direito do aviso, não do estado — vale até
    /// `revoke_until`, esticada pela pausa. Segunda suspensão nunca encurta a anterior (máximo).
    /// Não é bloqueada pela pausa (ato de conformidade, não move dinheiro).
    pub fn suspend(e: Env, revoke_until_ts: u64) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        cfg.admin.require_auth();
        if status(&e) != STATUS_RAISING {
            return Err(SaleError::WrongState);
        }
        if is_suspended(&e) {
            return Err(SaleError::Suspended);
        }
        if revoke_until_ts <= e.ledger().timestamp() {
            return Err(SaleError::InvalidRevokeUntil);
        }
        let base = revoke_until_ts.saturating_sub(pause_total(&e));
        let atual: u64 = e.storage().instance().get(&DataKey::RevokeBase).unwrap_or(0);
        e.storage().instance().set(&DataKey::RevokeBase, &base.max(atual));
        e.storage().instance().set(&DataKey::Suspended, &true);
        SuspendedEvent { revoke_until: revoke_until_ts }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// v11 — Admin: reabre o `buy`. A revogação já aberta segue até `revoke_until` (ver `suspend`).
    pub fn resume_sale(e: Env) -> Result<(), SaleError> {
        let cfg = escrow_only(&e)?;
        cfg.admin.require_auth();
        if !is_suspended(&e) {
            return Err(SaleError::NotSuspended);
        }
        e.storage().instance().set(&DataKey::Suspended, &false);
        SaleResumedEvent { resumed: true }.publish(&e);
        bump(&e);
        Ok(())
    }

    // ── Pausa e emergency_pauser (v11; desenho dos v5 dos outros contratos) ──
    //
    // A pausa bloqueia: buy, withdraw_commitment, close, fail, refund, claim, withdraw.
    // Continuam abertos: set_frozen (conformidade), cancel (proteção do investidor, não move
    // dinheiro), suspend/resume_sale (conformidade), set_active, set_terms, upgrade, extend_ttl,
    // pause/resume do admin, as trocas do pauser e as leituras. O `Config.admin` deste contrato é
    // fixo desde o deploy (não há rotação), então "rotação não zera o pauser" vale por ausência.
    // Enquanto pausado todo prazo PARA: `pause_total` cresce com o relógio e estica `deadline_ts`,
    // as janelas e a revogação na mesma medida.

    /// Pausa. Admin.
    pub fn pause(e: Env) {
        config(&e).admin.require_auth();
        do_pause(&e);
        PausedEvent { paused: true }.publish(&e);
        bump(&e);
    }

    /// Despausa. SÓ o admin — o `emergency_pauser` não despausa. Fecha o intervalo pausado em
    /// `PauseAcc` (idempotente: despausar o que não está pausado não soma nada).
    pub fn resume(e: Env) {
        config(&e).admin.require_auth();
        if is_paused(&e) {
            let acc = pause_total(&e);
            e.storage().instance().set(&DataKey::PauseAcc, &acc);
            e.storage().instance().set(&DataKey::Paused, &false);
        }
        ResumedEvent { resumed: true }.publish(&e);
        bump(&e);
    }

    /// Define (ou remove, com `None`) o `emergency_pauser`. Admin. Trocar invalida o anterior no
    /// ato. TODA troca emite `pauserset` com (anterior, novo).
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

    /// O PRÓPRIO pauser renuncia: só ele assina (o admin remove com `set_emergency_pauser(None)`).
    /// Emite `pauserset` (anterior, None) e `pauserren`. Não mexe em pausa, admin nem dinheiro.
    pub fn renounce_emergency_pauser(e: Env) -> Result<(), SaleError> {
        let pauser = emergency_pauser_of(&e).ok_or(SaleError::NoEmergencyPauser)?;
        pauser.require_auth();
        e.storage().instance().remove(&DataKey::EmergencyPauser);
        PauserSetEvent { previous: Some(pauser.clone()), new: None }.publish(&e);
        PauserRenouncedEvent { pauser }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Pausa de emergência: SÓ o `emergency_pauser` assina e liga a MESMA pausa de `pause()`.
    /// Não despausa, não move dinheiro, não troca admin, não faz upgrade — não tem outro efeito.
    /// Idempotente: pausar o já pausado não reinicia o relógio da pausa.
    pub fn emergency_pause(e: Env) -> Result<(), SaleError> {
        let pauser = emergency_pauser_of(&e).ok_or(SaleError::NoEmergencyPauser)?;
        pauser.require_auth();
        do_pause(&e);
        PausedEvent { paused: true }.publish(&e);
        EmergencyPausedEvent { pauser }.publish(&e);
        bump(&e);
        Ok(())
    }

    /// Qualquer um pode estender o TTL da instância (cron).
    pub fn extend_ttl(e: Env) {
        e.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
    }

    /// Upgrade do WASM — VALVULA DE RECUPERACAO (decisao do Pedro 24/08, pos red-team).
    /// So `admin`, que na mainnet e a LEDGER (cold/2-de-3), NUNCA uma hot wallet.
    /// SEM lock de "dinheiro dentro" DE PROPOSITO: e exatamente quando ha USDC de
    /// investidor escrowado e um bug prende a rodada (o deadlock "fechavel mas
    /// infechavel" que o red-team achou) que a Ledger tem que poder trocar o codigo
    /// e destravar. Eu havia REMOVIDO o upgrade no v10 por iniciativa ("cancel e a
    /// valvula"); o Pedro corrigiu: a valvula e a Ledger. Ver recovery-valve-ledger-only.
    pub fn upgrade(e: Env, new_wasm_hash: BytesN<32>) -> Result<(), SaleError> {
        config(&e).admin.require_auth();
        e.deployer().update_current_contract_wasm(new_wasm_hash);
        bump(&e);
        Ok(())
    }

    // ── Views ──
    pub fn version(_e: Env) -> u32 {
        CONTRACT_VERSION
    }
    pub fn get_config(e: Env) -> Config {
        config(&e)
    }
    pub fn get_terms(e: Env) -> Terms {
        terms(&e)
    }
    /// (status, active, target em títulos, vendidos, captado retido, entregas pendentes)
    pub fn get_round(e: Env) -> (u32, bool, i128, i128, i128, i128) {
        (status(&e), active(&e), target(&e), sold(&e), raised(&e), sold_remaining(&e))
    }
    pub fn get_contrib(e: Env, investor: Address) -> i128 {
        contrib(&e, &investor)
    }
    pub fn is_investor_frozen(e: Env, investor: Address) -> bool {
        is_frozen(&e, &investor)
    }
    // ── Views v11 (leitura para o backend e para a tela; tudo já com a pausa somada) ──
    pub fn get_paused(e: Env) -> bool {
        is_paused(&e)
    }
    pub fn emergency_pauser(e: Env) -> Option<Address> {
        emergency_pauser_of(&e)
    }
    /// Alias de `emergency_pauser` (o nome do v5 dos irmãos é o de cima).
    pub fn get_emergency_pauser(e: Env) -> Option<Address> {
        emergency_pauser_of(&e)
    }
    /// Segundos pausados até agora (fechados + a pausa corrente).
    pub fn get_pause_total(e: Env) -> u64 {
        pause_total(&e)
    }
    /// Prazo efetivo = `deadline_ts` + tempo pausado. 0 se a oferta não tem prazo (direct).
    pub fn get_effective_deadline(e: Env) -> u64 {
        let cfg = config(&e);
        if cfg.deadline_ts == 0 { 0 } else { deadline_eff(&e, &cfg) }
    }
    /// Até quando o investidor pode desistir: o maior entre o fim da janela dele e o fim da
    /// revogação aberta. 0 = não pode (sem compra, janela 0, sem revogação). Só vale se tiver contrib.
    pub fn get_withdraw_until(e: Env, investor: Address) -> u64 {
        withdraw_until(&e, &config(&e), &investor)
    }
    /// Fim da última janela de desistência da oferta (0 = nenhuma compra com janela).
    pub fn get_last_window_end(e: Env) -> u64 {
        last_window_end_eff(&e)
    }
    /// (suspensa, fim da revogação) — fim 0 = nunca houve suspensão.
    pub fn get_suspension(e: Env) -> (bool, u64) {
        (is_suspended(&e), revoke_end_eff(&e))
    }
}

// ── Helpers ──

fn config(e: &Env) -> Config {
    e.storage().instance().get(&DataKey::Config).unwrap()
}
fn terms(e: &Env) -> Terms {
    e.storage().instance().get(&DataKey::Terms).unwrap()
}
fn status(e: &Env) -> u32 {
    e.storage().instance().get(&DataKey::Status).unwrap_or(STATUS_RAISING)
}
fn active(e: &Env) -> bool {
    e.storage().instance().get(&DataKey::Active).unwrap_or(false)
}
fn target(e: &Env) -> i128 {
    e.storage().instance().get(&DataKey::Target).unwrap_or(0)
}
fn sold(e: &Env) -> i128 {
    e.storage().instance().get(&DataKey::Sold).unwrap_or(0)
}
fn sold_remaining(e: &Env) -> i128 {
    e.storage().instance().get(&DataKey::SoldRemaining).unwrap_or(0)
}
fn raised(e: &Env) -> i128 {
    e.storage().instance().get(&DataKey::Raised).unwrap_or(0)
}
fn contrib(e: &Env, investor: &Address) -> i128 {
    e.storage().persistent().get(&DataKey::Contrib(investor.clone())).unwrap_or(0)
}
fn set_contrib(e: &Env, investor: &Address, v: i128) {
    let key = DataKey::Contrib(investor.clone());
    e.storage().persistent().set(&key, &v);
    e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}
fn spent(e: &Env, investor: &Address) -> i128 {
    e.storage().persistent().get(&DataKey::Spent(investor.clone())).unwrap_or(0)
}
fn set_spent(e: &Env, investor: &Address, v: i128) {
    let key = DataKey::Spent(investor.clone());
    e.storage().persistent().set(&key, &v);
    e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
}
fn is_frozen(e: &Env, investor: &Address) -> bool {
    e.storage().persistent().get(&DataKey::Frozen(investor.clone())).unwrap_or(false)
}
fn bump(e: &Env) {
    e.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
}

fn escrow_only(e: &Env) -> Result<Config, SaleError> {
    let cfg = config(e);
    if cfg.mode != MODE_ESCROW {
        return Err(SaleError::WrongState);
    }
    Ok(cfg)
}

/// custo = tokens × price / 10^7, exigindo resto ZERO (orientação inequívoca, zero poeira).
fn usdc_cost_exact(tokens: i128, price: i128) -> Result<i128, SaleError> {
    let gross = tokens.checked_mul(price).ok_or(SaleError::Overflow)?;
    if gross % STROOP != 0 {
        return Err(SaleError::RemainderNotZero);
    }
    Ok(gross / STROOP)
}
/// Como acima, sem exigir resto zero (usado para converter o alvo na ativação).
fn usdc_cost(tokens: i128, price: i128) -> Result<i128, SaleError> {
    Ok(tokens.checked_mul(price).ok_or(SaleError::Overflow)? / STROOP)
}

// ── v11: pausa, janelas, revogação ──

fn is_paused(e: &Env) -> bool {
    e.storage().instance().get(&DataKey::Paused).unwrap_or(false)
}
fn require_not_paused(e: &Env) -> Result<(), SaleError> {
    if is_paused(e) {
        return Err(SaleError::ContractPaused);
    }
    Ok(())
}
fn emergency_pauser_of(e: &Env) -> Option<Address> {
    e.storage().instance().get(&DataKey::EmergencyPauser)
}
/// Liga a pausa sem reiniciar o relógio de uma pausa que já corre.
fn do_pause(e: &Env) {
    if !is_paused(e) {
        e.storage().instance().set(&DataKey::PausedSince, &e.ledger().timestamp());
        e.storage().instance().set(&DataKey::Paused, &true);
    }
}
/// Segundos pausados até agora: os intervalos já fechados + (se pausado) o corrente. Monotônico e
/// cresce 1 a 1 com o relógio durante a pausa — por isso `base + pause_total` PARA o prazo.
fn pause_total(e: &Env) -> u64 {
    let acc: u64 = e.storage().instance().get(&DataKey::PauseAcc).unwrap_or(0);
    if is_paused(e) {
        let since: u64 = e.storage().instance().get(&DataKey::PausedSince).unwrap_or(0);
        acc.saturating_add(e.ledger().timestamp().saturating_sub(since))
    } else {
        acc
    }
}
fn deadline_eff(e: &Env, cfg: &Config) -> u64 {
    cfg.deadline_ts.saturating_add(pause_total(e))
}
fn is_suspended(e: &Env) -> bool {
    e.storage().instance().get(&DataKey::Suspended).unwrap_or(false)
}
/// Fim da revogação (esticado pela pausa); 0 se nunca houve suspensão.
fn revoke_end_eff(e: &Env) -> u64 {
    match e.storage().instance().get::<DataKey, u64>(&DataKey::RevokeBase) {
        Some(base) => base.saturating_add(pause_total(e)),
        None => 0,
    }
}
fn in_revocation(e: &Env) -> bool {
    e.ledger().timestamp() < revoke_end_eff(e)
}
fn last_buy(e: &Env, investor: &Address) -> Option<BuyStamp> {
    e.storage().persistent().get(&DataKey::LastBuy(investor.clone()))
}
/// Grava o carimbo da compra (instante + pausa acumulada) e sobe o máximo da oferta, em O(1).
/// Sem janela (0) não há o que guardar.
fn stamp_buy(e: &Env, cfg: &Config, investor: &Address, now: u64) {
    if cfg.withdraw_window_secs == 0 {
        return;
    }
    let acc = pause_total(e);
    let key = DataKey::LastBuy(investor.clone());
    e.storage().persistent().set(&key, &BuyStamp { ts: now, acc });
    e.storage().persistent().extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
    // base = ts + janela − acc; fim efetivo da janela = base + pause_total. `acc ≤ ts` sempre
    // (a pausa acumulada cabe no tempo decorrido), então o saturating_sub nunca corta de verdade.
    let base = now.saturating_add(cfg.withdraw_window_secs).saturating_sub(acc);
    let atual: u64 = e.storage().instance().get(&DataKey::LastWindowBase).unwrap_or(0);
    if base > atual {
        e.storage().instance().set(&DataKey::LastWindowBase, &base);
    }
}
/// Fim efetivo da janela do investidor: ts + janela + (pausa de agora − pausa na compra).
fn window_end_eff(e: &Env, cfg: &Config, investor: &Address) -> u64 {
    match last_buy(e, investor) {
        Some(b) => b
            .ts
            .saturating_add(cfg.withdraw_window_secs)
            .saturating_add(pause_total(e).saturating_sub(b.acc)),
        None => 0,
    }
}
fn withdraw_until(e: &Env, cfg: &Config, investor: &Address) -> u64 {
    window_end_eff(e, cfg, investor).max(revoke_end_eff(e))
}
/// Fim efetivo da última janela da oferta (0 = nenhuma compra com janela).
fn last_window_end_eff(e: &Env) -> u64 {
    match e.storage().instance().get::<DataKey, u64>(&DataKey::LastWindowBase) {
        Some(base) => base.saturating_add(pause_total(e)),
        None => 0,
    }
}

/// `sold ≥ mínimo`: o captado retido cobre o piso (piso 0 = 100% do alvo).
fn floor_reached(e: &Env, cfg: &Config) -> bool {
    let tgt = target(e);
    if tgt == 0 {
        return false;
    }
    let amount = raised(e);
    if amount <= 0 {
        return false;
    }
    let floor = if cfg.min_raise == 0 {
        match usdc_cost(tgt, cfg.price) {
            Ok(v) => v,
            Err(_) => return false,
        }
    } else {
        cfg.min_raise
    };
    amount >= floor
}

/// Condição de êxito sem as janelas: bateu o mínimo E (cheia OU vencida — prazo efetivo).
fn floor_and_full_or_expired(e: &Env, cfg: &Config) -> bool {
    floor_reached(e, cfg) && (sold(e) == target(e) || e.ledger().timestamp() >= deadline_eff(e, cfg))
}

/// O `close` seria aceito agora (ignorando só o lastro e a pausa): êxito, última janela vencida,
/// sem suspensão nem revogação. É o teste do `cancel` ("fechável pertence aos investidores").
fn closeable_now(e: &Env, cfg: &Config) -> bool {
    !is_suspended(e)
        && !in_revocation(e)
        && floor_and_full_or_expired(e, cfg)
        && e.ledger().timestamp() >= last_window_end_eff(e)
}

fn validate_canonical_usdc(_e: &Env, _buy_token: &Address) -> Result<(), SaleError> {
    #[cfg(not(feature = "testing"))]
    {
        let canonical = Address::from_str(_e, USDC_SAC);
        if *_buy_token != canonical {
            return Err(SaleError::UnauthorizedToken);
        }
    }
    Ok(())
}

// PEND 159: was `#[cfg(all(test, feature = "testing"))]` — that hid the ENTIRE
// test module (all 651 lines) from `--features testnet|mainnet` builds, so the
// F-006 canonical-USDC rejection test below could never even compile under the
// real deploy features, let alone run. test.rs itself carries `#![cfg(test)]`,
// same as the other two contracts' `mod test;` — that's the only gate needed.
#[cfg(test)]
mod test;
