// Copyright 2026 the rio-miami-rail contributors
// SPDX-License-Identifier: Apache-2.0

//! custody_registry: who holds a tokenized shipment in transit, handover by handover.
//! Each handover is signed by both custodians. No money and no token move here.
//
// External Radox boundaries (prior startup implementations, not public):
// - Token issuance and burn (transaction 7 in `scripts/lifecycle.mjs`) call Radox's
//   pre-existing `transfer_agent` contract (`register_token`, `mint`, `deliver`, `redeem`),
//   which is closed-source prior work from Radox's regulated token-issuance platform.
// - Off-chain backend reads (`GET /api/custody/shipments/:key`) come from Radox's closed
//   backend (prior work, not public), which calls `CustodyRegistry::get(key)` via Soroban RPC `simulateTransaction`.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, xdr::ToXdr, Address, Bytes,
    BytesN, Env, Vec,
};

const VERSION: u32 = 1;

// persistent TTL, in ledgers (~5 s each): bump to ~30 days when under ~7 days
const TTL_THRESHOLD: u32 = 120_960;
const TTL_EXTEND_TO: u32 = 518_400;

const TAG_OPEN: u8 = 0x01;
const TAG_HANDOVER: u8 = 0x02;
const TAG_ATTEST: u8 = 0x03;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    WrongCustodian = 1,
    OutOfRoute = 2,
    NotOpen = 3,
    DuplicateKey = 4,
    UnknownSource = 5,
    Paused = 6,
    Frozen = 7,
    NotFound = 8,
    BadRoute = 9,
    NotFinal = 10,
    NotFrozen = 11,
    NoPauser = 12,
    BadAssurance = 13,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Open,
    Delivered,
    Frozen,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shipment {
    pub key: BytesN<32>,
    pub token: Address,
    pub current_custodian: Address,
    pub route: Vec<Address>,
    pub seq: u32,
    pub head: BytesN<32>,
    pub status: Status,
    pub holder: Option<Address>,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Operator,
    Paused,
    Pauser,
    Source(Address),
    Shipment(BytesN<32>),
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Opened {
    #[topic]
    pub key: BytesN<32>,
    pub token: Address,
    pub first_custodian: Address,
    pub head: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handover {
    #[topic]
    pub key: BytesN<32>,
    pub from: Address,
    pub to: Address,
    pub seq: u32,
    pub evidence_hash: BytesN<32>,
    pub assurance: u32,
    pub head: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attested {
    #[topic]
    pub key: BytesN<32>,
    pub source: Address,
    pub kind: u32,
    pub event_hash: BytesN<32>,
    pub head: BytesN<32>,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Delivered {
    #[topic]
    pub key: BytesN<32>,
    pub final_custodian: Address,
    pub holder: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusChanged {
    #[topic]
    pub key: BytesN<32>,
    pub status: Status,
}

#[contract]
pub struct CustodyRegistry;

fn be32(env: &Env, n: u32) -> Bytes {
    Bytes::from_array(env, &n.to_be_bytes())
}

fn tag(env: &Env, t: u8) -> Bytes {
    Bytes::from_array(env, &[t])
}

/// open: sha256(0x01 || key || docs_root)
pub fn head_open(env: &Env, key: &BytesN<32>, docs_root: &BytesN<32>) -> BytesN<32> {
    let mut b = tag(env, TAG_OPEN);
    b.append(&key.clone().into());
    b.append(&docs_root.clone().into());
    env.crypto().sha256(&b).to_bytes()
}

/// handover: sha256(0x02 || head || addr(from) || addr(to) || evidence_hash || be32(seq) || be32(assurance))
pub fn head_handover(
    env: &Env,
    head: &BytesN<32>,
    from: &Address,
    to: &Address,
    evidence_hash: &BytesN<32>,
    seq: u32,
    assurance: u32,
) -> BytesN<32> {
    let mut b = tag(env, TAG_HANDOVER);
    b.append(&head.clone().into());
    b.append(&from.clone().to_xdr(env));
    b.append(&to.clone().to_xdr(env));
    b.append(&evidence_hash.clone().into());
    b.append(&be32(env, seq));
    b.append(&be32(env, assurance));
    env.crypto().sha256(&b).to_bytes()
}

/// attest: sha256(0x03 || head || addr(source) || be32(kind) || event_hash || be32(seq))
pub fn head_attest(
    env: &Env,
    head: &BytesN<32>,
    source: &Address,
    kind: u32,
    event_hash: &BytesN<32>,
    seq: u32,
) -> BytesN<32> {
    let mut b = tag(env, TAG_ATTEST);
    b.append(&head.clone().into());
    b.append(&source.clone().to_xdr(env));
    b.append(&be32(env, kind));
    b.append(&event_hash.clone().into());
    b.append(&be32(env, seq));
    env.crypto().sha256(&b).to_bytes()
}

fn admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

// instance entries (admin, operator, pause) expire with the contract instance: bump on every write
fn bump_instance(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn require_not_paused(env: &Env) -> Result<(), Error> {
    if env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
    {
        return Err(Error::Paused);
    }
    Ok(())
}

fn load(env: &Env, key: &BytesN<32>) -> Result<Shipment, Error> {
    let k = DataKey::Shipment(key.clone());
    let s: Shipment = env.storage().persistent().get(&k).ok_or(Error::NotFound)?;
    env.storage()
        .persistent()
        .extend_ttl(&k, TTL_THRESHOLD, TTL_EXTEND_TO);
    Ok(s)
}

fn save(env: &Env, s: &Shipment) {
    bump_instance(env);
    let k = DataKey::Shipment(s.key.clone());
    env.storage().persistent().set(&k, s);
    env.storage()
        .persistent()
        .extend_ttl(&k, TTL_THRESHOLD, TTL_EXTEND_TO);
}

fn require_open(s: &Shipment) -> Result<(), Error> {
    match s.status {
        Status::Open => Ok(()),
        Status::Frozen => Err(Error::Frozen),
        Status::Delivered => Err(Error::NotOpen),
    }
}

#[contractimpl]
impl CustodyRegistry {
    pub fn __constructor(env: Env, admin: Address, operator: Address) {
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage().instance().set(&DataKey::Paused, &false);
        bump_instance(&env);
    }

    /// Operator opens a shipment on a fixed route; route[0] is the first custodian.
    pub fn open(
        env: Env,
        key: BytesN<32>,
        token: Address,
        route: Vec<Address>,
        docs_root: BytesN<32>,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        let operator: Address = env.storage().instance().get(&DataKey::Operator).unwrap();
        operator.require_auth();
        if env
            .storage()
            .persistent()
            .has(&DataKey::Shipment(key.clone()))
        {
            return Err(Error::DuplicateKey);
        }
        if route.len() < 2 {
            return Err(Error::BadRoute);
        }
        for i in 1..route.len() {
            if route.get_unchecked(i) == route.get_unchecked(i - 1) {
                return Err(Error::BadRoute);
            }
        }
        let first = route.get_unchecked(0);
        let head = head_open(&env, &key, &docs_root);
        let s = Shipment {
            key: key.clone(),
            token: token.clone(),
            current_custodian: first.clone(),
            route,
            seq: 0,
            head: head.clone(),
            status: Status::Open,
            holder: None,
        };
        save(&env, &s);
        Opened {
            key,
            token,
            first_custodian: first,
            head,
        }
        .publish(&env);
        Ok(())
    }

    /// Both custodians sign; `to` must be the next stop on the route. Ownership does not move.
    pub fn handover(
        env: Env,
        key: BytesN<32>,
        from: Address,
        to: Address,
        evidence_hash: BytesN<32>,
        assurance: u32,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        if !(1..=3).contains(&assurance) {
            return Err(Error::BadAssurance);
        }
        from.require_auth();
        to.require_auth();
        let mut s = load(&env, &key)?;
        require_open(&s)?;
        if from != s.current_custodian {
            return Err(Error::WrongCustodian);
        }
        let next = s.seq + 1;
        if next >= s.route.len() || s.route.get_unchecked(next) != to {
            return Err(Error::OutOfRoute);
        }
        s.seq = next;
        s.head = head_handover(&env, &s.head, &from, &to, &evidence_hash, s.seq, assurance);
        s.current_custodian = to.clone();
        save(&env, &s);
        Handover {
            key,
            from,
            to,
            seq: s.seq,
            evidence_hash,
            assurance,
            head: s.head,
        }
        .publish(&env);
        Ok(())
    }

    /// A registered event source adds an attestation to the chain; it blocks nothing.
    pub fn attest(
        env: Env,
        key: BytesN<32>,
        source: Address,
        kind: u32,
        event_hash: BytesN<32>,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        source.require_auth();
        let sk = DataKey::Source(source.clone());
        if !env.storage().persistent().has(&sk) {
            return Err(Error::UnknownSource);
        }
        env.storage()
            .persistent()
            .extend_ttl(&sk, TTL_THRESHOLD, TTL_EXTEND_TO);
        let mut s = load(&env, &key)?;
        s.head = head_attest(&env, &s.head, &source, kind, &event_hash, s.seq);
        save(&env, &s);
        Attested {
            key,
            source,
            kind,
            event_hash,
            head: s.head,
        }
        .publish(&env);
        Ok(())
    }

    /// The last custodian hands the goods to the holder. The token burn is not here.
    pub fn deliver(
        env: Env,
        key: BytesN<32>,
        final_custodian: Address,
        holder: Address,
    ) -> Result<(), Error> {
        require_not_paused(&env)?;
        final_custodian.require_auth();
        holder.require_auth();
        let mut s = load(&env, &key)?;
        require_open(&s)?;
        // every handover done (seq at the last stop), not just an address that repeats in the route
        let last_i = s.route.len() - 1;
        if s.seq != last_i
            || final_custodian != s.route.get_unchecked(last_i)
            || final_custodian != s.current_custodian
        {
            return Err(Error::NotFinal);
        }
        s.status = Status::Delivered;
        s.holder = Some(holder.clone());
        save(&env, &s);
        Delivered {
            key,
            final_custodian,
            holder,
        }
        .publish(&env);
        Ok(())
    }

    /// Admin exit for lost or seized cargo, a court order or a sanction.
    /// Allowed while paused: it is the exit a pause is for.
    pub fn freeze(env: Env, key: BytesN<32>) -> Result<(), Error> {
        admin(&env).require_auth();
        let mut s = load(&env, &key)?;
        require_open(&s)?;
        s.status = Status::Frozen;
        save(&env, &s);
        StatusChanged {
            key,
            status: Status::Frozen,
        }
        .publish(&env);
        Ok(())
    }

    pub fn unfreeze(env: Env, key: BytesN<32>) -> Result<(), Error> {
        admin(&env).require_auth();
        let mut s = load(&env, &key)?;
        if s.status != Status::Frozen {
            return Err(Error::NotFrozen);
        }
        s.status = Status::Open;
        save(&env, &s);
        StatusChanged {
            key,
            status: Status::Open,
        }
        .publish(&env);
        Ok(())
    }

    pub fn add_source(env: Env, source: Address) -> Result<(), Error> {
        require_not_paused(&env)?;
        admin(&env).require_auth();
        bump_instance(&env);
        let k = DataKey::Source(source);
        env.storage().persistent().set(&k, &true);
        env.storage()
            .persistent()
            .extend_ttl(&k, TTL_THRESHOLD, TTL_EXTEND_TO);
        Ok(())
    }

    pub fn remove_source(env: Env, source: Address) -> Result<(), Error> {
        require_not_paused(&env)?;
        admin(&env).require_auth();
        env.storage().persistent().remove(&DataKey::Source(source));
        Ok(())
    }

    pub fn pause(env: Env) {
        admin(&env).require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        bump_instance(&env);
    }

    pub fn resume(env: Env) {
        admin(&env).require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    pub fn set_pauser(env: Env, pauser: Option<Address>) {
        admin(&env).require_auth();
        match pauser {
            Some(p) => env.storage().instance().set(&DataKey::Pauser, &p),
            None => env.storage().instance().remove(&DataKey::Pauser),
        }
    }

    /// The optional pauser can stop writes; only the admin resumes.
    pub fn emergency_pause(env: Env) -> Result<(), Error> {
        let pauser: Address = env
            .storage()
            .instance()
            .get(&DataKey::Pauser)
            .ok_or(Error::NoPauser)?;
        pauser.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn get(env: Env, key: BytesN<32>) -> Result<Shipment, Error> {
        load(&env, &key)
    }

    pub fn head(env: Env, key: BytesN<32>) -> Result<BytesN<32>, Error> {
        Ok(load(&env, &key)?.head)
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    pub fn version(_env: Env) -> u32 {
        VERSION
    }
}

#[cfg(test)]
mod prop_test;
#[cfg(test)]
mod test;
