// Copyright 2026 the rio-miami-rail contributors
// SPDX-License-Identifier: Apache-2.0

#![cfg(test)]
extern crate std;

use super::*;
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction},
    vec, Address, Bytes, BytesN, Env, IntoVal, Symbol,
};

// head test vector, shared with scripts/verify-head.mjs --selftest
const VEC_A: &str = "GAUTZ4AOL3QBR4OAVXAECCQYQW6MZCTSQ45CUNNXYYIFQZFBGKPZVCLM";
const VEC_B: &str = "GAYOU2QZGAYOMTUHF6N3BO32ZYUEXL52IMJRBOOIRXN6MEOOOB3KERB3";
const VEC_KEY: &str = "af078925b6df1601bd50a3a310c3db646311380940c87cc4d20979f32fbc472a";
const VEC_HEAD_OPEN: &str = "4d27935042faa36a33b9c8726cdf96e8995ce7d33e0e7c0479e37fc14eaf8264";
const VEC_HEAD_HANDOVER: &str = "7f75df7c846b6e6389ec1205819f64a0fe96a6d10d385322440973ca27a129f2";

fn sha(env: &Env, s: &str) -> BytesN<32> {
    env.crypto()
        .sha256(&Bytes::from_slice(env, s.as_bytes()))
        .to_bytes()
}

fn hex(b: &BytesN<32>) -> std::string::String {
    b.to_array()
        .iter()
        .map(|x| std::format!("{:02x}", x))
        .collect()
}

struct Setup {
    env: Env,
    client: CustodyRegistryClient<'static>,
    admin: Address,
    operator: Address,
    token: Address,
    route: Vec<Address>,
    key: BytesN<32>,
}

fn setup() -> Setup {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let operator = Address::generate(&env);
    let token = Address::generate(&env);
    let id = env.register(CustodyRegistry, (&admin, &operator));
    let client = CustodyRegistryClient::new(&env, &id);
    // warehouse -> port -> carrier -> customs -> recipient
    let route = vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let key = sha(&env, "TEST1234567|BRRIO");
    Setup {
        env,
        client,
        admin,
        operator,
        token,
        route,
        key,
    }
}

fn open(s: &Setup) {
    s.client
        .open(&s.key, &s.token, &s.route, &sha(&s.env, "docs"));
}

fn hand(s: &Setup, i: u32) {
    let ev = sha(&s.env, "evidence");
    s.client.handover(
        &s.key,
        &s.route.get_unchecked(i),
        &s.route.get_unchecked(i + 1),
        &ev,
        &3,
    );
}

#[test]
fn head_vector() {
    let env = Env::default();
    let a = Address::from_str(&env, VEC_A);
    let b = Address::from_str(&env, VEC_B);
    let key = sha(&env, "TEST1234567|BRRIO");
    assert_eq!(hex(&key), VEC_KEY);
    let h0 = head_open(&env, &key, &sha(&env, "docs-root-v1"));
    assert_eq!(hex(&h0), VEC_HEAD_OPEN);
    let h1 = head_handover(&env, &h0, &a, &b, &sha(&env, "evidence-1"), 1, 3);
    assert_eq!(hex(&h1), VEC_HEAD_HANDOVER);
}

#[test]
fn head_vector_through_contract() {
    let s = setup();
    let a = Address::from_str(&s.env, VEC_A);
    let b = Address::from_str(&s.env, VEC_B);
    let c = Address::generate(&s.env);
    let route = vec![&s.env, a.clone(), b.clone(), c];
    s.client
        .open(&s.key, &s.token, &route, &sha(&s.env, "docs-root-v1"));
    assert_eq!(hex(&s.client.head(&s.key)), VEC_HEAD_OPEN);
    s.client
        .handover(&s.key, &a, &b, &sha(&s.env, "evidence-1"), &3);
    assert_eq!(hex(&s.client.head(&s.key)), VEC_HEAD_HANDOVER);
}

#[test]
fn full_lifecycle() {
    let s = setup();
    open(&s);
    assert_eq!(
        s.env.auths()[0].0,
        s.operator,
        "open is authorised by the operator"
    );
    let g = s.client.get(&s.key);
    assert_eq!(g.seq, 0);
    assert_eq!(g.current_custodian, s.route.get_unchecked(0));
    assert_eq!(g.status, Status::Open);

    for i in 0..4 {
        hand(&s, i);
        // both custodians authorised this exact call
        let auths = s.env.auths();
        let from = s.route.get_unchecked(i);
        let to = s.route.get_unchecked(i + 1);
        for who in [&from, &to] {
            assert!(
                auths.iter().any(|(a, inv)| a == who
                    && matches!(&inv.function, AuthorizedFunction::Contract((_, f, _)) if *f == Symbol::new(&s.env, "handover"))),
                "handover {} not signed by both custodians",
                i
            );
        }
        let g = s.client.get(&s.key);
        assert_eq!(g.seq, i + 1);
        assert_eq!(g.current_custodian, to);
    }

    let recipient = s.route.get_unchecked(4);
    let holder = Address::generate(&s.env);
    s.client.deliver(&s.key, &recipient, &holder);
    let auths = s.env.auths();
    assert!(auths.iter().any(|(a, _)| *a == recipient));
    assert!(auths.iter().any(|(a, _)| *a == holder));
    assert_eq!(s.client.get(&s.key).status, Status::Delivered);
    // token address untouched: ownership never moves here
    assert_eq!(s.client.get(&s.key).token, s.token);
}

#[test]
fn head_recompute_matches_chain() {
    let s = setup();
    let docs = sha(&s.env, "docs");
    s.client.open(&s.key, &s.token, &s.route, &docs);
    let mut h = head_open(&s.env, &s.key, &docs);
    let src = Address::generate(&s.env);
    s.client.add_source(&src);
    for i in 0..4u32 {
        let ev = sha(&s.env, "evidence");
        let (f, t) = (s.route.get_unchecked(i), s.route.get_unchecked(i + 1));
        s.client.handover(&s.key, &f, &t, &ev, &2);
        h = head_handover(&s.env, &h, &f, &t, &ev, i + 1, 2);
        if i == 1 {
            let e = sha(&s.env, "gate-in");
            s.client.attest(&s.key, &src, &7, &e);
            h = head_attest(&s.env, &h, &src, 7, &e, 2);
        }
    }
    assert_eq!(s.client.head(&s.key), h);
    // attest does not move seq
    assert_eq!(s.client.get(&s.key).seq, 4);
}

#[test]
fn refuses_duplicate_key() {
    let s = setup();
    open(&s);
    let r = s
        .client
        .try_open(&s.key, &s.token, &s.route, &sha(&s.env, "docs"));
    assert_eq!(r, Err(Ok(Error::DuplicateKey)));
}

#[test]
fn refuses_bad_route() {
    let s = setup();
    let one = vec![&s.env, Address::generate(&s.env)];
    assert_eq!(
        s.client
            .try_open(&s.key, &s.token, &one, &sha(&s.env, "docs")),
        Err(Ok(Error::BadRoute))
    );
    let a = Address::generate(&s.env);
    let twice = vec![&s.env, a.clone(), a];
    assert_eq!(
        s.client
            .try_open(&s.key, &s.token, &twice, &sha(&s.env, "docs")),
        Err(Ok(Error::BadRoute))
    );
}

#[test]
fn refuses_wrong_custodian() {
    let s = setup();
    open(&s);
    let ev = sha(&s.env, "evidence");
    // route[1] is not the current custodian
    let r = s.client.try_handover(
        &s.key,
        &s.route.get_unchecked(1),
        &s.route.get_unchecked(2),
        &ev,
        &3,
    );
    assert_eq!(r, Err(Ok(Error::WrongCustodian)));
}

#[test]
fn refuses_out_of_route() {
    let s = setup();
    open(&s);
    let ev = sha(&s.env, "evidence");
    // skipping a stop
    let r = s.client.try_handover(
        &s.key,
        &s.route.get_unchecked(0),
        &s.route.get_unchecked(2),
        &ev,
        &3,
    );
    assert_eq!(r, Err(Ok(Error::OutOfRoute)));
    // a stranger
    let r = s.client.try_handover(
        &s.key,
        &s.route.get_unchecked(0),
        &Address::generate(&s.env),
        &ev,
        &3,
    );
    assert_eq!(r, Err(Ok(Error::OutOfRoute)));
    // past the end of the route
    for i in 0..4 {
        hand(&s, i);
    }
    let r = s.client.try_handover(
        &s.key,
        &s.route.get_unchecked(4),
        &s.route.get_unchecked(0),
        &ev,
        &3,
    );
    assert_eq!(r, Err(Ok(Error::OutOfRoute)));
}

#[test]
fn refuses_unknown_shipment() {
    let s = setup();
    assert_eq!(s.client.try_get(&s.key), Err(Ok(Error::NotFound)));
}

#[test]
fn refuses_early_or_wrong_delivery() {
    let s = setup();
    open(&s);
    let holder = Address::generate(&s.env);
    let last = s.route.get_unchecked(4);
    // goods still at the warehouse
    assert_eq!(
        s.client.try_deliver(&s.key, &last, &holder),
        Err(Ok(Error::NotFinal))
    );
    for i in 0..4 {
        hand(&s, i);
    }
    // not the last custodian
    let other = s.route.get_unchecked(3);
    assert_eq!(
        s.client.try_deliver(&s.key, &other, &holder),
        Err(Ok(Error::NotFinal))
    );
    s.client.deliver(&s.key, &last, &holder);
    // after delivery nothing moves
    assert_eq!(
        s.client.try_deliver(&s.key, &last, &holder),
        Err(Ok(Error::NotOpen))
    );
    let ev = sha(&s.env, "evidence");
    assert_eq!(
        s.client
            .try_handover(&s.key, &last, &s.route.get_unchecked(0), &ev, &3),
        Err(Ok(Error::NotOpen))
    );
    assert_eq!(s.client.try_freeze(&s.key), Err(Ok(Error::NotOpen)));
}

#[test]
fn freeze_blocks_then_unfreeze_resumes() {
    let s = setup();
    open(&s);
    s.client.freeze(&s.key);
    assert_eq!(s.env.auths()[0].0, s.admin);
    assert_eq!(s.client.get(&s.key).status, Status::Frozen);
    let ev = sha(&s.env, "evidence");
    let r = s.client.try_handover(
        &s.key,
        &s.route.get_unchecked(0),
        &s.route.get_unchecked(1),
        &ev,
        &3,
    );
    assert_eq!(r, Err(Ok(Error::Frozen)));
    let holder = Address::generate(&s.env);
    assert_eq!(
        s.client
            .try_deliver(&s.key, &s.route.get_unchecked(4), &holder),
        Err(Ok(Error::Frozen))
    );
    s.client.unfreeze(&s.key);
    assert_eq!(s.client.try_unfreeze(&s.key), Err(Ok(Error::NotFrozen)));
    hand(&s, 0);
    assert_eq!(s.client.get(&s.key).seq, 1);
}

#[test]
fn attest_needs_registered_source() {
    let s = setup();
    open(&s);
    let src = Address::generate(&s.env);
    let e = sha(&s.env, "event");
    assert_eq!(
        s.client.try_attest(&s.key, &src, &1, &e),
        Err(Ok(Error::UnknownSource))
    );
    s.client.add_source(&src);
    let before = s.client.head(&s.key);
    s.client.attest(&s.key, &src, &1, &e);
    assert_ne!(s.client.head(&s.key), before);
    s.client.remove_source(&src);
    assert_eq!(
        s.client.try_attest(&s.key, &src, &1, &e),
        Err(Ok(Error::UnknownSource))
    );
}

#[test]
fn pause_blocks_every_write() {
    let s = setup();
    open(&s);
    let src = Address::generate(&s.env);
    s.client.add_source(&src);
    s.client.pause();
    assert!(s.client.is_paused());
    let ev = sha(&s.env, "evidence");
    let other = sha(&s.env, "OTHER0000001|BRRIO");
    assert_eq!(
        s.client.try_open(&other, &s.token, &s.route, &ev),
        Err(Ok(Error::Paused))
    );
    assert_eq!(
        s.client.try_handover(
            &s.key,
            &s.route.get_unchecked(0),
            &s.route.get_unchecked(1),
            &ev,
            &3
        ),
        Err(Ok(Error::Paused))
    );
    assert_eq!(
        s.client.try_attest(&s.key, &src, &1, &ev),
        Err(Ok(Error::Paused))
    );
    assert_eq!(
        s.client.try_deliver(
            &s.key,
            &s.route.get_unchecked(4),
            &Address::generate(&s.env)
        ),
        Err(Ok(Error::Paused))
    );
    // reads still work
    assert_eq!(s.client.get(&s.key).seq, 0);
    s.client.resume();
    hand(&s, 0);
}

#[test]
fn emergency_pauser() {
    let s = setup();
    assert_eq!(s.client.try_emergency_pause(), Err(Ok(Error::NoPauser)));
    let p = Address::generate(&s.env);
    s.client.set_pauser(&Some(p.clone()));
    s.client.emergency_pause();
    assert_eq!(s.env.auths()[0].0, p);
    assert!(s.client.is_paused());
    s.client.resume();
    assert!(!s.client.is_paused());
    s.client.set_pauser(&None);
    assert_eq!(s.client.try_emergency_pause(), Err(Ok(Error::NoPauser)));
}

// sign only as `who`; any other require_auth in the call is then missing
fn only_signs(
    env: &Env,
    id: &Address,
    who: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
) {
    env.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: who,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: id,
            fn_name,
            args,
            sub_invokes: &[],
        },
    }]);
}

fn is_auth_error<T: core::fmt::Debug, E: core::fmt::Debug>(
    r: Result<T, Result<E, soroban_sdk::InvokeError>>,
) -> bool {
    // a missing signature is a host error: Abort through a Result-returning fn, Error(Auth, ..) otherwise;
    // never one of this contract's error codes
    match r {
        Err(Err(soroban_sdk::InvokeError::Abort)) => true,
        Err(Ok(e)) => std::format!("{:?}", e).contains("Auth"),
        _ => false,
    }
}

#[test]
fn handover_without_incoming_signature_fails() {
    let s = setup();
    open(&s);
    let (a, b) = (s.route.get_unchecked(0), s.route.get_unchecked(1));
    let ev = sha(&s.env, "evidence");
    only_signs(
        &s.env,
        &s.client.address,
        &a,
        "handover",
        (&s.key, &a, &b, &ev, 3u32).into_val(&s.env),
    );
    assert!(is_auth_error(
        s.client.try_handover(&s.key, &a, &b, &ev, &3)
    ));
    assert_eq!(s.client.get(&s.key).seq, 0);
}

#[test]
fn handover_without_outgoing_signature_fails() {
    let s = setup();
    open(&s);
    let (a, b) = (s.route.get_unchecked(0), s.route.get_unchecked(1));
    let ev = sha(&s.env, "evidence");
    only_signs(
        &s.env,
        &s.client.address,
        &b,
        "handover",
        (&s.key, &a, &b, &ev, 3u32).into_val(&s.env),
    );
    assert!(is_auth_error(
        s.client.try_handover(&s.key, &a, &b, &ev, &3)
    ));
}

#[test]
fn open_needs_operator_signature() {
    let s = setup();
    let docs = sha(&s.env, "docs");
    let stranger = Address::generate(&s.env);
    only_signs(
        &s.env,
        &s.client.address,
        &stranger,
        "open",
        (&s.key, &s.token, &s.route, &docs).into_val(&s.env),
    );
    assert!(is_auth_error(
        s.client.try_open(&s.key, &s.token, &s.route, &docs)
    ));
}

#[test]
fn deliver_needs_holder_signature() {
    let s = setup();
    open(&s);
    for i in 0..4 {
        hand(&s, i);
    }
    let last = s.route.get_unchecked(4);
    let holder = Address::generate(&s.env);
    only_signs(
        &s.env,
        &s.client.address,
        &last,
        "deliver",
        (&s.key, &last, &holder).into_val(&s.env),
    );
    assert!(is_auth_error(s.client.try_deliver(&s.key, &last, &holder)));
    assert_eq!(s.client.get(&s.key).status, Status::Open);
}

#[test]
fn admin_calls_need_admin_signature() {
    let s = setup();
    open(&s);
    let src = Address::generate(&s.env);
    // with no signature each admin call fails and changes nothing...
    s.env.mock_auths(&[]);
    assert!(s.client.try_freeze(&s.key).is_err());
    assert!(s.client.try_pause().is_err());
    assert!(s.client.try_add_source(&src).is_err());
    assert_eq!(s.client.get(&s.key).status, Status::Open);
    assert!(!s.client.is_paused());
    // ...and the same calls pass once the admin signs
    s.env.mock_all_auths();
    s.client.freeze(&s.key);
    assert_eq!(s.env.auths()[0].0, s.admin);
    s.client.add_source(&src);
    s.client.pause();
    assert_eq!(s.env.auths()[0].0, s.admin);
}

#[test]
fn repeated_address_in_route_cannot_skip_handovers() {
    let s = setup();
    let (a, b) = (Address::generate(&s.env), Address::generate(&s.env));
    let route = vec![&s.env, a.clone(), b.clone(), a.clone()];
    s.client
        .open(&s.key, &s.token, &route, &sha(&s.env, "docs"));
    let holder = Address::generate(&s.env);
    // a is route[0] and route[2]: still not final at seq 0
    assert_eq!(
        s.client.try_deliver(&s.key, &a, &holder),
        Err(Ok(Error::NotFinal))
    );
    let ev = sha(&s.env, "evidence");
    s.client.handover(&s.key, &a, &b, &ev, &3);
    s.client.handover(&s.key, &b, &a, &ev, &3);
    s.client.deliver(&s.key, &a, &holder);
    assert_eq!(s.client.get(&s.key).status, Status::Delivered);
}

#[test]
fn refuses_assurance_out_of_range() {
    let s = setup();
    open(&s);
    let (a, b) = (s.route.get_unchecked(0), s.route.get_unchecked(1));
    let ev = sha(&s.env, "evidence");
    assert_eq!(
        s.client.try_handover(&s.key, &a, &b, &ev, &0),
        Err(Ok(Error::BadAssurance))
    );
    assert_eq!(
        s.client.try_handover(&s.key, &a, &b, &ev, &4),
        Err(Ok(Error::BadAssurance))
    );
    s.client.handover(&s.key, &a, &b, &ev, &1);
}

#[test]
fn holder_recorded_on_delivery() {
    let s = setup();
    open(&s);
    assert_eq!(s.client.get(&s.key).holder, None);
    for i in 0..4 {
        hand(&s, i);
    }
    let holder = Address::generate(&s.env);
    s.client.deliver(&s.key, &s.route.get_unchecked(4), &holder);
    assert_eq!(s.client.get(&s.key).holder, Some(holder));
}

#[test]
fn pause_blocks_sources_but_not_freeze() {
    let s = setup();
    open(&s);
    s.client.pause();
    let src = Address::generate(&s.env);
    assert_eq!(s.client.try_add_source(&src), Err(Ok(Error::Paused)));
    assert_eq!(s.client.try_remove_source(&src), Err(Ok(Error::Paused)));
    // freeze is the emergency exit a pause exists for
    s.client.freeze(&s.key);
    assert_eq!(s.client.get(&s.key).status, Status::Frozen);
}

#[test]
fn version_is_one() {
    let s = setup();
    assert_eq!(s.client.version(), 1);
}
