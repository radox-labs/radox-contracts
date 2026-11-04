// Copyright 2026 the rio-miami-rail contributors
// SPDX-License-Identifier: Apache-2.0

// Property tests: random routes and random call sequences checked against a plain model of the
// contract. Seeded generator, no extra dependency; PROP_CASES and PROP_SEED change the run.
#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, EnvTestConfig, MockAuth, MockAuthInvoke},
    xdr::ToXdr,
    Address, Bytes, BytesN, Env, IntoVal,
};
use std::vec::Vec as StdVec;

// xorshift64*: deterministic, so a failing case is replayed with its printed seed
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
    fn chance(&mut self, pct: u32) -> bool {
        self.below(100) < pct
    }
}

fn cases() -> u64 {
    std::env::var("PROP_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}
fn seed() -> u64 {
    std::env::var("PROP_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5EED_2026_1025)
}

fn rand32(env: &Env, r: &mut Rng) -> BytesN<32> {
    let mut a = [0u8; 32];
    for c in a.chunks_mut(8) {
        c.copy_from_slice(&r.next().to_be_bytes());
    }
    BytesN::from_array(env, &a)
}

// the head layout written out again from the README, not through the contract's helpers
fn fold(env: &Env, parts: &[Bytes]) -> BytesN<32> {
    let mut b = Bytes::new(env);
    for p in parts {
        b.append(p);
    }
    env.crypto().sha256(&b).to_bytes()
}
fn u32be(env: &Env, n: u32) -> Bytes {
    Bytes::from_array(env, &n.to_be_bytes())
}
fn model_open(env: &Env, key: &BytesN<32>, docs: &BytesN<32>) -> BytesN<32> {
    fold(
        env,
        &[
            Bytes::from_array(env, &[1]),
            key.clone().into(),
            docs.clone().into(),
        ],
    )
}
fn model_handover(
    env: &Env,
    h: &BytesN<32>,
    f: &Address,
    t: &Address,
    ev: &BytesN<32>,
    seq: u32,
    a: u32,
) -> BytesN<32> {
    fold(
        env,
        &[
            Bytes::from_array(env, &[2]),
            h.clone().into(),
            f.clone().to_xdr(env),
            t.clone().to_xdr(env),
            ev.clone().into(),
            u32be(env, seq),
            u32be(env, a),
        ],
    )
}
fn model_attest(
    env: &Env,
    h: &BytesN<32>,
    src: &Address,
    kind: u32,
    ev: &BytesN<32>,
    seq: u32,
) -> BytesN<32> {
    fold(
        env,
        &[
            Bytes::from_array(env, &[3]),
            h.clone().into(),
            src.clone().to_xdr(env),
            u32be(env, kind),
            ev.clone().into(),
            u32be(env, seq),
        ],
    )
}

// a route of 2..=7 stops drawn from a small pool, so a stop can come back (never twice in a row)
fn random_route(env: &Env, r: &mut Rng, pool: &[Address]) -> soroban_sdk::Vec<Address> {
    let len = 2 + r.below(6);
    let mut route = soroban_sdk::Vec::new(env);
    let mut last: Option<u32> = None;
    while route.len() < len {
        let i = r.below(pool.len() as u32);
        if Some(i) != last {
            route.push_back(pool[i as usize].clone());
            last = Some(i);
        }
    }
    route
}

struct Case {
    env: Env,
    client: CustodyRegistryClient<'static>,
    pool: StdVec<Address>,
    strangers: StdVec<Address>,
    source: Address,
    key: BytesN<32>,
    route: soroban_sdk::Vec<Address>,
    head: BytesN<32>,
}

fn new_case(r: &mut Rng) -> Case {
    // one Env per case: no snapshot file per case (thousands of files)
    let env = Env::new_with_config(EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let id = env.register(CustodyRegistry, (&admin, &operator));
    let client = CustodyRegistryClient::new(&env, &id);
    let pool: StdVec<Address> = (0..4).map(|_| Address::generate(&env)).collect();
    let strangers: StdVec<Address> = (0..2).map(|_| Address::generate(&env)).collect();
    let source = Address::generate(&env);
    client.add_source(&source);
    let key = rand32(&env, r);
    let docs = rand32(&env, r);
    let route = random_route(&env, r, &pool);
    client.open(&key, &Address::generate(&env), &route, &docs);
    let head = model_open(&env, &key, &docs);
    Case {
        env,
        client,
        pool,
        strangers,
        source,
        key,
        route,
        head,
    }
}

// Route invariants and the head chain: any sequence of handover, attest and deliver calls
// (right or wrong) leaves the contract exactly where the model says, with the model's head.
#[test]
fn prop_route_invariants_and_head_chain() {
    let mut r = Rng(seed());
    for case in 0..cases() {
        let mut c = new_case(&mut r);
        let (env, last) = (&c.env, c.route.len() - 1);
        let (mut seq, mut delivered) = (0u32, false);
        for step in 0..24 {
            let any = |r: &mut Rng| -> Address {
                if r.chance(15) {
                    c.strangers[r.below(2) as usize].clone()
                } else {
                    c.pool[r.below(4) as usize].clone()
                }
            };
            let what = r.below(10);
            if what < 7 {
                // handover: mostly the right pair, often a wrong one
                let (from, to) = if r.chance(50) && seq < last {
                    (c.route.get_unchecked(seq), c.route.get_unchecked(seq + 1))
                } else {
                    (any(&mut r), any(&mut r))
                };
                let assurance = if r.chance(85) {
                    1 + r.below(3)
                } else {
                    [0, 4, 9][r.below(3) as usize]
                };
                let ev = rand32(env, &mut r);
                // from == to: the second require_auth on one address in one call is a host auth
                // error, not one of our codes; still a refusal
                let host_error = (1..=3).contains(&assurance) && from == to;
                let want = if !(1..=3).contains(&assurance) {
                    Err(Error::BadAssurance)
                } else if host_error || delivered {
                    Err(Error::NotOpen) // unused when host_error
                } else if from != c.route.get_unchecked(seq) {
                    Err(Error::WrongCustodian)
                } else if seq >= last || to != c.route.get_unchecked(seq + 1) {
                    Err(Error::OutOfRoute)
                } else {
                    Ok(())
                };
                let got = c.client.try_handover(&c.key, &from, &to, &ev, &assurance);
                if host_error {
                    assert!(
                        matches!(got, Err(Err(_))),
                        "seed {} case {case} step {step}: self-handover not refused",
                        seed()
                    );
                } else {
                    assert_eq!(
                        got.map(|_| ()).map_err(|e| e.unwrap()),
                        want,
                        "seed {} case {case} step {step}: handover",
                        seed()
                    );
                }
                if want.is_ok() {
                    seq += 1;
                    c.head = model_handover(env, &c.head, &from, &to, &ev, seq, assurance);
                }
            } else if what < 9 {
                let (kind, ev) = (r.below(5), rand32(env, &mut r));
                c.client.attest(&c.key, &c.source, &kind, &ev); // accepted in any status, by design
                c.head = model_attest(env, &c.head, &c.source, kind, &ev, seq);
            } else {
                let fin = if r.chance(60) {
                    c.route.get_unchecked(last)
                } else {
                    any(&mut r)
                };
                let want = if delivered {
                    Err(Error::NotOpen)
                } else if seq != last || fin != c.route.get_unchecked(last) {
                    Err(Error::NotFinal)
                } else {
                    Ok(())
                };
                let got = c.client.try_deliver(&c.key, &fin, &Address::generate(env));
                assert_eq!(
                    got.map(|_| ()).map_err(|e| e.unwrap()),
                    want,
                    "seed {} case {case} step {step}: deliver",
                    seed()
                );
                delivered |= want.is_ok(); // deliver does not change the head
            }
            let s = c.client.get(&c.key);
            assert_eq!(s.seq, seq, "seed {} case {case} step {step}: seq", seed());
            assert_eq!(
                s.current_custodian,
                c.route.get_unchecked(seq),
                "seed {} case {case}: custodian",
                seed()
            );
            assert_eq!(
                s.status,
                if delivered {
                    Status::Delivered
                } else {
                    Status::Open
                }
            );
            assert_eq!(
                s.head,
                c.head,
                "seed {} case {case} step {step}: head",
                seed()
            );
            assert_eq!(c.client.head(&c.key), c.head);
        }
    }
}

// Auth: a handover goes through only when both the outgoing and the incoming custodian sign.
// Any other signer set (one of them, strangers, the operator) fails and changes nothing.
#[test]
fn prop_handover_needs_both_custodians() {
    let mut r = Rng(seed() ^ 0xA0A0);
    for case in 0..cases() {
        let c = new_case(&mut r);
        let env = &c.env;
        let stop = r.below(c.route.len() - 1); // walk to a random stop with every signature mocked
        let mut seq = 0;
        while seq < stop {
            let ev = rand32(env, &mut r);
            c.client.handover(
                &c.key,
                &c.route.get_unchecked(seq),
                &c.route.get_unchecked(seq + 1),
                &ev,
                &3,
            );
            seq += 1;
        }
        let (from, to) = (c.route.get_unchecked(seq), c.route.get_unchecked(seq + 1));
        let ev = rand32(env, &mut r);
        let args: soroban_sdk::Vec<soroban_sdk::Val> =
            (&c.key, &from, &to, &ev, 2u32).into_val(env);
        let candidates = [
            from.clone(),
            to.clone(),
            c.strangers[0].clone(),
            c.strangers[1].clone(),
        ];
        let mask = r.below(16);
        let signers: StdVec<&Address> = candidates
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, a)| a)
            .collect();
        let invoke = MockAuthInvoke {
            contract: &c.client.address,
            fn_name: "handover",
            args: args.clone(),
            sub_invokes: &[],
        };
        let mocks: StdVec<MockAuth> = signers
            .iter()
            .map(|a| MockAuth {
                address: a,
                invoke: &invoke,
            })
            .collect();
        env.mock_auths(&mocks);
        let before = c.client.get(&c.key);
        let got = c.client.try_handover(&c.key, &from, &to, &ev, &2);
        let both = mask & 0b11 == 0b11;
        env.mock_all_auths();
        let after = c.client.get(&c.key);
        if both {
            assert!(
                got.is_ok(),
                "seed {} case {case} mask {mask:04b}: both signed but refused",
                seed()
            );
            assert_eq!(after.seq, seq + 1);
        } else {
            assert!(
                got.is_err(),
                "seed {} case {case} mask {mask:04b}: accepted without both custodians",
                seed()
            );
            assert_eq!(
                (after.seq, after.head),
                (before.seq, before.head),
                "seed {} case {case}: state moved",
                seed()
            );
        }
    }
}
