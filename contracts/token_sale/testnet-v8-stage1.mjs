/**
 * 7.3 — ETAPA 1 (sem saldo de USDC): tudo o que se prova sem mover dinheiro.
 *
 * Prova, contra a chain real, com o build --features testnet (F-006 ATIVO):
 *   1. create() ACEITA o USDC canônico de testnet e REJEITA qualquer outro token
 *   2. D4 — escrow sem deadline é recusado
 *   3. D1 — Target congela do supply na 1ª ativação e não recalcula
 *   4. D6 — preço trava depois do Target
 *   5. D14 — MIGRAÇÃO REAL v7 → v8 in-place: instância nascida no wasm v7 de
 *      testnet sofre upgrade() e o storage sobrevive (struct Offer intacta),
 *      lendo como MODE_DIRECT. Nenhum dos 3 contratos tinha esse teste.
 *
 * A etapa 2 (trade/close/refund) precisa de USDC de testnet numa conta que a
 * gente assine — ver relatório no fim.
 */
import {
    Keypair, Asset, Operation, TransactionBuilder, Networks, BASE_FEE,
    Contract, Address, nativeToScVal, scValToNative, rpc,
} from '@stellar/stellar-sdk';
import { readFileSync } from 'node:fs';
import crypto from 'node:crypto';

const R = new rpc.Server('https://soroban-testnet.stellar.org');
const PASSPHRASE = Networks.TESTNET;
const USDC_SAC = 'CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA';
const V7 = process.env.V7_WASM_HASH_FULL;
const U = 10_000_000n;
const say = (...a) => console.log(...a);
let fails = 0;
const ok = (c, m) => { if (c) say('   ✓', m); else { say('   ❌', m); fails++; } };

async function submit(kp, op) {
    const acct = await R.getAccount(kp.publicKey());
    let tx = new TransactionBuilder(acct, { fee: '3000000', networkPassphrase: PASSPHRASE })
        .addOperation(op).setTimeout(60).build();
    const sim = await R.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim)) throw new Error('SIM:' + sim.error);
    tx = rpc.assembleTransaction(tx, sim).build();
    tx.sign(kp);
    const sent = await R.sendTransaction(tx);
    if (sent.status === 'ERROR') throw new Error('SEND:' + JSON.stringify(sent.errorResult));
    for (let i = 0; i < 40; i++) {
        await new Promise(r => setTimeout(r, 1500));
        const g = await R.getTransaction(sent.hash);
        if (g.status === 'SUCCESS') return g;
        if (g.status === 'FAILED') throw new Error('FAILED:' + sent.hash);
    }
    throw new Error('TIMEOUT');
}
const call = async (kp, id, m, args, expectErr = false) => {
    try {
        const g = await submit(kp, new Contract(id).call(m, ...args));
        if (expectErr) { say('   ❌', `${m} deveria falhar e passou`); fails++; }
        return g.returnValue ? scValToNative(g.returnValue) : null;
    } catch (e) {
        if (expectErr) return { rejected: e.message.slice(0, 70) };
        throw e;
    }
};
const view = async (kp, id, m, args = []) => {
    const acct = await R.getAccount(kp.publicKey());
    const tx = new TransactionBuilder(acct, { fee: BASE_FEE, networkPassphrase: PASSPHRASE })
        .addOperation(new Contract(id).call(m, ...args)).setTimeout(30).build();
    const s = await R.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(s)) throw new Error('view ' + m + ': ' + s.error);
    return s.result?.retval ? scValToNative(s.result.retval) : null;
};
const A = (a) => new Address(a).toScVal();
const i128 = (v) => nativeToScVal(v, { type: 'i128' });
const u32 = (v) => nativeToScVal(v, { type: 'u32' });
const fb = async (kp) => { await fetch(`https://friendbot.stellar.org?addr=${kp.publicKey()}`); await new Promise(r => setTimeout(r, 2500)); };
const deploy = async (kp, h) => scValToNative((await submit(kp, Operation.createCustomContract({
    address: new Address(kp.publicKey()), wasmHash: Buffer.from(h, 'hex'), salt: crypto.randomBytes(32),
}))).returnValue);

say('═══ SETUP ═══');
const seller = Keypair.random();
const company = Keypair.random();
const treasury = Keypair.random();
await fb(seller);
say('seller:', seller.publicKey());

const CODE = 'TV8' + Math.floor(Math.random() * 900 + 100);
const asset = new Asset(CODE, seller.publicKey());
await submit(seller, Operation.createStellarAssetContract({ asset }));
const tokenSac = asset.contractId(PASSPHRASE);
say('token de teste:', CODE, tokenSac.slice(0, 10) + '…');

const v8 = Buffer.from(scValToNative((await submit(seller,
    Operation.uploadContractWasm({ wasm: readFileSync('/app/backend/token_sale_v8.wasm') }))).returnValue)).toString('hex');
say('v8 wasm:', v8.slice(0, 16), '\n');

// ── 1 + 2: F-006 e D4 ────────────────────────────────────────────────────
say('═══ 1-2. F-006 (USDC canônico) + D4 (deadline obrigatório) ═══');
const s1 = await deploy(seller, v8);
const cur = await R.getLatestLedger();
const argsBase = (buyToken, deadline, mode) => [
    A(seller.publicKey()), A(seller.publicKey()), A(tokenSac), A(buyToken),
    A(treasury.publicKey()), A(company.publicKey()),
    i128(1n * U), u32(1), u32(1), u32(deadline), i128(0n), i128(0n), u32(mode),
];
const fake = await call(seller, s1, 'create', argsBase(tokenSac, cur.sequence + 500, 1), true);
ok(!!fake?.rejected, 'create() REJEITA token que não é o USDC canônico (F-006 ativo no build testnet)');
const noDeadline = await call(seller, s1, 'create', argsBase(USDC_SAC, 0, 1), true);
ok(!!noDeadline?.rejected, 'create() REJEITA escrow com deadline 0 (D4 — sem prazo o refund nunca abriria)');
await call(seller, s1, 'create', argsBase(USDC_SAC, cur.sequence + 500, 1));
ok(true, 'create() ACEITA o USDC canônico de testnet');
ok(Number(await view(seller, s1, 'version')) === 8, 'contrato reporta v8');

// ── 3 + 4: Target e trava de preço ───────────────────────────────────────
say('\n═══ 3-4. Target congelado (D1) + preço travado (D6) ═══');
const r0 = await view(seller, s1, 'get_raise');
ok(Number(r0[2]) === 0, 'Target ainda 0 antes da ativação');
await call(seller, tokenSac, 'mint', [A(s1), i128(6n * U)]);
await call(seller, s1, 'set_active', [nativeToScVal(true)]);
const r1 = await view(seller, s1, 'get_raise');
ok(BigInt(r1[2]) === 6n * U, `Target congelou do supply: ${Number(r1[2]) / 1e7} USDC`);
await call(seller, tokenSac, 'mint', [A(s1), i128(3n * U)]);   // depósito extra
await call(seller, s1, 'set_active', [nativeToScVal(false)]);
await call(seller, s1, 'set_active', [nativeToScVal(true)]);
ok(BigInt((await view(seller, s1, 'get_raise'))[2]) === 6n * U, 'Target NÃO recalcula em reativação (imutável)');
const price = await call(seller, s1, 'updt_price', [u32(2), u32(1)], true);
ok(!!price?.rejected, 'updt_price REJEITADO com Target definido (D6)');

// ── 5: migração real v7 → v8 ─────────────────────────────────────────────
say('\n═══ 5. MIGRAÇÃO REAL v7 → v8 IN-PLACE (gate D14) ═══');
if (!V7) { say('   ⚠ V7_WASM_HASH_FULL ausente — pulado'); fails++; }
else {
    const s2 = await deploy(seller, V7);
    say('   instância v7:', s2.slice(0, 12) + '…');
    await call(seller, s2, 'create', [        // 13 args — assinatura v7
        A(seller.publicKey()), A(seller.publicKey()), A(tokenSac), A(USDC_SAC),
        A(treasury.publicKey()), A(company.publicKey()),
        i128(1n * U), u32(1), u32(1), u32(0), i128(0n), i128(0n),
    ]);
    ok(Number(await view(seller, s2, 'version')) === 7, 'instância nasce v7');
    await call(seller, tokenSac, 'mint', [A(s2), i128(3n * U)]);
    await call(seller, s2, 'set_active', [nativeToScVal(true)]);
    const offerV7 = await view(seller, s2, 'get_offer');
    ok(!!offerV7 && offerV7.is_active === true, 'estado v7 gravado (is_active=true)');

    await call(seller, s2, 'upgrade', [nativeToScVal(Buffer.from(v8, 'hex'), { type: 'bytes' })]);
    ok(Number(await view(seller, s2, 'version')) === 8, 'upgrade() in-place aplicou o v8');

    const offerV8 = await view(seller, s2, 'get_offer');
    ok(!!offerV8, '🔑 get_offer AINDA desserializa storage v7 sob código v8 — struct Offer intacta (regra do brick)');
    ok(offerV8.is_active === true && String(offerV8.fixed_fee) === String(1n * U),
        'campos do offer preservados byte a byte (is_active + fixed_fee)');
    const raiseV8 = await view(seller, s2, 'get_raise');
    ok(Number(raiseV8[0]) === 0 && Number(raiseV8[1]) === 0,
        'chaves novas ausentes leem como MODE_DIRECT/Raising (unwrap_or) — instância v7 segue direct');
    const closeV7 = await call(seller, s2, 'close', [], true);
    ok(!!closeV7?.rejected, 'ciclo de escrow indisponível em instância nascida v7');
    await call(seller, s2, 'set_active', [nativeToScVal(false)]);
    ok((await view(seller, s2, 'get_offer')).is_active === false, 'escrita no storage v7 funciona sob v8');
}

say('\n' + (fails ? `❌ ${fails} FALHA(S)` : '✅ TODAS AS ASSERÇÕES DA ETAPA 1 PASSARAM'));
say('\nPENDENTE (etapa 2, precisa de USDC de testnet numa conta assinável):');
say('  trade retém principal · fee ao treasury na hora · close paga o Target');
say('  claim_tokens · fail + refund devolvendo só o principal · withdraw de USDC bloqueado');
process.exit(fails ? 1 : 0);
