//! Mock USDC token registered AT the canonical SAC address (PENDENCIAS 159).
//!
//! WHY: the deployed WASM is built with `--no-default-features --features
//! testnet|mainnet`. Under those features `validate_canonical_usdc` (F-006) is
//! live and only accepts the ONE hardcoded SAC address per network. Every test
//! fixture builds USDC with `register_stellar_asset_contract_v2`, whose address
//! is derived from a randomly generated issuer — never the canonical constant.
//! So the suite could only ever run under `testing`, i.e. against a binary that
//! is NOT the one that ships.
//!
//! `Env::register_at` is the way out: it puts a contract at an address we
//! choose. This mock is that contract — the smallest token the three contracts
//! and their fixtures actually call.
//!
//! Included by each crate's `src/test.rs` via `#[path = "../../shared/mock_usdc.rs"]`,
//! under `#[cfg(not(feature = "testing"))]`. It never reaches a WASM build.
//!
//! FIDELITY — what this is NOT: a Stellar Asset Contract. It has no issuer, no
//! trustlines, no authorization/clawback flags, no allowances, no admin auth on
//! `mint`. It reproduces exactly what the contracts and fixtures exercise on
//! USDC: mint, balance, transfer (auth of `from`, panic on overdraw) and
//! clawback. Anything that depends on real SAC semantics stays on the real SAC
//! under `testing` — see the named exceptions in each test.rs.
//!
//! Depth matters: the mock implements the token itself rather than proxying to
//! a real SAC, so the recorded auth tree has the SAME depth as production
//! (`contract -> token.transfer`). A proxy would add a hop and break every
//! `mock_auths` / `env.auths()` assertion.

use soroban_sdk::{contract, contractimpl, Address, Env};

/// The canonical USDC SAC address the contracts validate against. Must stay
/// byte-identical to the `USDC_SAC` const in each `lib.rs` — the test
/// `mock_sits_at_the_canonical_address` in each crate asserts exactly that.
#[cfg(feature = "testnet")]
pub const CANONICAL_USDC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
#[cfg(feature = "mainnet")]
pub const CANONICAL_USDC: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";

#[contract]
pub struct MockUsdc;

fn get(env: &Env, id: &Address) -> i128 {
    env.storage().persistent().get(id).unwrap_or(0)
}

fn put(env: &Env, id: &Address, amount: i128) {
    env.storage().persistent().set(id, &amount);
}

#[contractimpl]
impl MockUsdc {
    /// SAC `mint` minus the admin auth check (no fixture tests unauthorized mint).
    pub fn mint(env: Env, to: Address, amount: i128) {
        put(&env, &to, get(&env, &to) + amount);
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        get(&env, &id)
    }

    /// SAC `transfer`: requires `from`'s auth, panics on overdraw or a negative
    /// amount. The panic is a plain host panic like the SAC's balance error —
    /// every test that expects it uses a bare `#[should_panic]`.
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        let balance = get(&env, &from);
        if amount < 0 || balance < amount {
            panic!("mock usdc: insufficient balance");
        }
        put(&env, &from, balance - amount);
        put(&env, &to, get(&env, &to) + amount);
    }

    /// SAC `clawback` minus the issuer `AUTH_CLAWBACK_ENABLED` flag and the
    /// trustline-creation ordering it implies (see CLAUDE.md, `SAC #10`).
    /// Only the maturity_settlement degenerate "USDC is also the security
    /// token" test reaches this path.
    pub fn clawback(env: Env, from: Address, amount: i128) {
        let balance = get(&env, &from);
        if amount < 0 || balance < amount {
            panic!("mock usdc: insufficient balance");
        }
        put(&env, &from, balance - amount);
    }
}
