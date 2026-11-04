/**
 * 7.3 — Validação on-chain do token_sale v8 (escrow) em TESTNET.
 *
 * Prova o que teste unitário NÃO consegue provar:
 *   A. O caminho do dinheiro com o USDC CANÔNICO de testnet (build --features
 *      testnet, F-006 ATIVO — o mock de SAC dos unit tests não exercita isso).
 *   B. Falha por prazo + refund por PULL devolvendo só o principal.
 *   C. MIGRAÇÃO REAL v7 → v8 in-place (o gate D14): instância nascida no wasm
 *      v7 de testnet, upgrade(), e o estado tem que sobreviver — nenhum dos 3
 *      contratos tinha teste de troca real de WASM até hoje.
 *
 * TESTNET APENAS. Roda de dentro do container (chaves + SDK lá dentro):
 *   docker cp ... && docker exec stellar_backend sh -c 'cd /app/backend && node --import tsx testnet-escrow-v8.mjs'
 */
import {
    Keypair, Asset, Operation, TransactionBuilder, Networks, BASE_FEE,
    Contract, Address, nativeToScVal, scValToNative, rpc, Horizon, xdr, StrKey, hash,
} from '@stellar/stellar-sdk';
import { readFileSync } from 'node:fs';
import crypto from 'node:crypto';

const RPC = new rpc.Server('https://soroban-testnet.stellar.org');
const HORIZON = new Horizon.Server('https://horizon-testnet.stellar.org');
const PASSPHRASE = Networks.TESTNET;
const USDC_SAC = 'CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA';
const USDC_ISSUER = 'GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5';
const V7_WASM_HASH = '6b4a1906'; // prefixo registrado na caatinga (testnet)
const U = 1_000_000n; // 0,1 USDC — escala reduzida p/ caber no orcamento de testnet
const say = (...a) => console.log(...a);
const ok = (cond, msg) => { if (!cond) { console.error('❌ FALHOU:', msg); process.exitCode = 1; } else say('   ✓', msg); };

// Fonte de USDC: OPERATIONS (unica chave com valor no dev). O papel de
// 'treasury' do contrato e uma conta descartavel, so para observar a fee.
const funder = Keypair.fromSecret(process.env.OPERATIONS_SECRET_KEY);
const treasury = Keypair.random();
say('Funder (USDC):', funder.publicKey());

// ── helpers ──────────────────────────────────────────────────────────────
async function submit(kp, op, { fee = '2000000' } = {}) {
    const acct = await RPC.getAccount(kp.publicKey());
    let tx = new TransactionBuilder(acct, { fee, networkPassphrase: PASSPHRASE })
        .addOperation(op).setTimeout(60).build();
    const sim = await RPC.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim)) throw new Error('sim: ' + sim.error);
    if (sim.result || sim.transactionData) tx = rpc.assembleTransaction(tx, sim).build();
    tx.sign(kp);
    const sent = await RPC.sendTransaction(tx);
    if (sent.status === 'ERROR') throw new Error('send: ' + JSON.stringify(sent.errorResult));
    for (let i = 0; i < 30; i++) {
        await new Promise(r => setTimeout(r, 1500));
        const got = await RPC.getTransaction(sent.hash);
        if (got.status === 'SUCCESS') return got;
        if (got.status === 'FAILED') throw new Error('tx FAILED: ' + sent.hash);
    }
    throw new Error('timeout ' + sent.hash);
}

/** Chama método do contrato; devolve o valor nativo. `expectErr` inverte a asserção. */
async function call(kp, contractId, method, args, { expectErr = false } = {}) {
    try {
        const got = await submit(kp, new Contract(contractId).call(method, ...args));
        const ret = got.returnValue ? scValToNative(got.returnValue) : null;
        if (expectErr) { console.error(`❌ ${method} deveria ter falhado e passou`); process.exitCode = 1; }
        return ret;
    } catch (e) {
        if (expectErr) return { rejected: e.message.slice(0, 90) };
        throw e;
    }
}

async function view(contractId, method, args = []) {
    const acct = await RPC.getAccount(funder.publicKey());
    const tx = new TransactionBuilder(acct, { fee: BASE_FEE, networkPassphrase: PASSPHRASE })
        .addOperation(new Contract(contractId).call(method, ...args)).setTimeout(30).build();
    const sim = await RPC.simulateTransaction(tx);
    if (rpc.Api.isSimulationError(sim)) throw new Error(`view ${method}: ${sim.error}`);
    return sim.result?.retval ? scValToNative(sim.result.retval) : null;
}

const usdcOf = async (addr) => BigInt(await view(USDC_SAC, 'balance', [new Address(addr).toScVal()]) ?? 0);
const addr = (a) => new Address(a).toScVal();
const i128 = (v) => nativeToScVal(v, { type: 'i128' });
const u32 = (v) => nativeToScVal(v, { type: 'u32' });

async function friendbot(kp) {
    await fetch(`https://friendbot.stellar.org?addr=${kp.publicKey()}`);
    await new Promise(r => setTimeout(r, 2000));
}

async function classicTx(kp, ops) {
    const acct = await HORIZON.loadAccount(kp.publicKey());
    const tx = new TransactionBuilder(acct, { fee: '100000', networkPassphrase: PASSPHRASE });
    ops.forEach(o => tx.addOperation(o));
    const built = tx.setTimeout(60).build();
    built.sign(kp);
    try { return await HORIZON.submitTransaction(built); }
    catch (e) {
        const rc = e?.response?.data?.extras?.result_codes;
        throw new Error('HORIZON ' + JSON.stringify(rc ?? e?.response?.data?.title ?? e.message));
    }
}

async function uploadWasm(kp, path) {
    const wasm = readFileSync(path);
    const got = await submit(kp, Operation.uploadContractWasm({ wasm }));
    return Buffer.from(scValToNative(got.returnValue)).toString('hex');
}

async function deployFrom(kp, wasmHashHex) {
    const salt = crypto.randomBytes(32);
    const got = await submit(kp, Operation.createCustomContract({
        address: new Address(kp.publicKey()),
        wasmHash: Buffer.from(wasmHashHex, 'hex'),
        salt,
    }));
    return scValToNative(got.returnValue);
}

// ── SETUP ────────────────────────────────────────────────────────────────
say('\n═══ SETUP ═══');
const seller = Keypair.random();      // admin+seller do sale e emissor do token de teste
const buyerA = Keypair.random();
const buyerB = Keypair.random();
const company = Keypair.random();
await Promise.all([friendbot(seller), friendbot(buyerA), friendbot(buyerB), friendbot(company), friendbot(treasury)]);
say('contas criadas:', [seller, buyerA, buyerB, company].map(k => k.publicKey().slice(0, 6)).join(' '));

const CODE = 'TV8' + Math.floor(Math.random() * 900 + 100);
const asset = new Asset(CODE, seller.publicKey());
const usdc = new Asset('USDC', USDC_ISSUER);

// trustlines: USDC para compradores/company, token de teste para o seller-side
for (const kp of [buyerA, buyerB, company, treasury]) {
    await classicTx(kp, [Operation.changeTrust({ asset: usdc })]);
}
say('trustlines USDC ok');

// USDC do treasury para os compradores (10 cada — sobra folga nos 166)
await classicTx(funder, [
    Operation.payment({ destination: buyerA.publicKey(), asset: usdc, amount: '1' }),
    Operation.payment({ destination: buyerB.publicKey(), asset: usdc, amount: '1' }),
]);
say('USDC distribuido: A=1 B=1');

// SAC do token de teste + mint para o seller
await submit(seller, Operation.createStellarAssetContract({ asset }));
const tokenSac = asset.contractId(PASSPHRASE);
say('SAC do token:', tokenSac.slice(0, 10) + '…');
await classicTx(seller, [Operation.changeTrust({ asset })]).catch(() => {});
// Compradores precisam de trustline do security token para RECEBER no claim.
// (Em producao o onboarding ja patrocina a trustline; aqui e manual.)
for (const kp of [buyerA, buyerB]) await classicTx(kp, [Operation.changeTrust({ asset })]);
say('trustlines do token de teste ok');

// ── UPLOAD v8 ────────────────────────────────────────────────────────────
say('\n═══ UPLOAD v8 ═══');
const v8Hash = await uploadWasm(seller, '/app/backend/token_sale_v8.wasm');
say('v8 wasm hash:', v8Hash.slice(0, 16));

// ═════════════════════════════════════════════════════════════════════════
say('\n═══ A. ESCROW HAPPY PATH (USDC canônico, F-006 ativo) ═══');
const saleA = await deployFrom(seller, v8Hash);
say('instância:', saleA.slice(0, 10) + '…');

const latest = await RPC.getLatestLedger();
await call(seller, saleA, 'create', [
    addr(seller.publicKey()), addr(seller.publicKey()),
    addr(tokenSac), addr(USDC_SAC),
    addr(treasury.publicKey()), addr(company.publicKey()),
    i128(1n * U),                    // fixed_fee = $1 (custeio)
    u32(1), u32(1),                  // 1:1
    u32(latest.sequence + 2000),     // deadline real (D4)
    i128(0n), i128(0n),
    u32(1),                          // MODE_ESCROW
]);
ok(true, 'create() aceitou o USDC canônico (F-006 passou com build testnet)');

// deposita 6 tokens → Target = 6 USDC
await call(seller, tokenSac, 'mint', [addr(saleA), i128(6n * U)]);
await call(seller, saleA, 'set_active', [nativeToScVal(true)]);
const raiseA0 = await view(saleA, 'get_raise');
ok(BigInt(raiseA0[2]) === 6n * U, `Target congelado do supply: ${Number(raiseA0[2]) / 1e7} USDC`);

const compBefore = await usdcOf(company.publicKey());
const treasBefore = await usdcOf(treasury.publicKey());

await call(buyerA, saleA, 'trade', [addr(buyerA.publicKey()), i128(4n * U)]);
const afterT1 = await view(saleA, 'get_raise');
ok(BigInt(afterT1[3]) === 4n * U, `raised = ${Number(afterT1[3])/1e7} USDC após 1ª compra`);
ok((await usdcOf(company.publicKey())) === compBefore, 'empresa NÃO recebeu nada durante a captação (D13)');
ok((await usdcOf(treasury.publicKey())) === treasBefore + 1n * U, 'fee foi ao treasury NA HORA (D10)');
ok(BigInt(await view(tokenSac, 'balance', [addr(buyerA.publicKey())]) ?? 0) === 0n, 'tokens RETIDOS no contrato (D8)');
ok((await usdcOf(saleA)) === 4n * U, 'contrato custodia exatamente o principal');

await call(buyerB, saleA, 'trade', [addr(buyerB.publicKey()), i128(2n * U)]);
ok(BigInt((await view(saleA, 'get_raise'))[3]) === 6n * U, 'raised = Target após 2ª compra');

// close() permissionless — quem chama é o COMPRADOR, não o admin (D2)
await call(buyerA, saleA, 'close', []);
const raiseAfterClose = await view(saleA, 'get_raise');
ok(Number(raiseAfterClose[1]) === 1, 'estado = Closed');
ok((await usdcOf(company.publicKey())) === compBefore + 6n * U, 'empresa recebeu EXATAMENTE o Target no close');
ok((await usdcOf(saleA)) === 0n, 'nenhum dólar de investidor ficou no contrato');

await call(buyerA, saleA, 'claim_tokens', [addr(buyerA.publicKey())]);
await call(buyerB, saleA, 'claim_tokens', [addr(buyerB.publicKey())]);
ok(BigInt(await view(tokenSac, 'balance', [addr(buyerA.publicKey())]) ?? 0) === 4n * U, 'A recebeu a parte dele em tokens no claim');
ok(BigInt(await view(tokenSac, 'balance', [addr(buyerB.publicKey())]) ?? 0) === 2n * U, 'B recebeu a parte dele em tokens no claim');

// ═════════════════════════════════════════════════════════════════════════
say('\n═══ B. FALHA POR PRAZO + REFUND ═══');
const saleB = await deployFrom(seller, v8Hash);
const cur = await RPC.getLatestLedger();
await call(seller, saleB, 'create', [
    addr(seller.publicKey()), addr(seller.publicKey()),
    addr(tokenSac), addr(USDC_SAC),
    addr(treasury.publicKey()), addr(company.publicKey()),
    i128(1n * U), u32(1), u32(1),
    u32(cur.sequence + 6),           // prazo curtíssimo, de propósito
    i128(0n), i128(0n), u32(1),
]);
await call(seller, tokenSac, 'mint', [addr(saleB), i128(6n * U)]);
await call(seller, saleB, 'set_active', [nativeToScVal(true)]);

const bBefore = await usdcOf(buyerB.publicKey());
await call(buyerB, saleB, 'trade', [addr(buyerB.publicKey()), i128(2n * U)]);
ok((await usdcOf(buyerB.publicKey())) === bBefore - 3n * U, 'B pagou principal + fee');

say('   aguardando o prazo vencer…');
while ((await RPC.getLatestLedger()).sequence <= cur.sequence + 6) await new Promise(r => setTimeout(r, 4000));

const tradeLate = await call(buyerA, saleB, 'trade', [addr(buyerA.publicKey()), i128(1n * U)], { expectErr: true });
ok(!!tradeLate?.rejected, 'compra APÓS o prazo é rejeitada (janela morta)');

await call(buyerA, saleB, 'fail', []);                // permissionless
ok(Number((await view(saleB, 'get_raise'))[1]) === 2, 'estado = Failed');
await call(buyerA, saleB, 'refund', [addr(buyerB.publicKey())]);  // terceiro dispara, dinheiro vai ao dono
ok((await usdcOf(buyerB.publicKey())) === bBefore - 1n * U, 'B recuperou o PRINCIPAL; a fee não volta (D10)');
ok((await usdcOf(saleB)) === 0n, 'escrow zerado');
const refundAgain = await call(buyerA, saleB, 'refund', [addr(buyerB.publicKey())], { expectErr: true });
ok(!!refundAgain?.rejected, 'segundo refund rejeitado (NothingToClaim)');

const wdrwUsdc = await call(seller, saleB, 'withdraw', [addr(USDC_SAC), i128(1n * U)], { expectErr: true });
ok(!!wdrwUsdc?.rejected, 'withdraw de USDC rejeitado mesmo em Failed (bomba PEND 66 desarmada)');

// ═════════════════════════════════════════════════════════════════════════
say('\n═══ C. MIGRAÇÃO REAL v7 → v8 IN-PLACE (gate D14) ═══');
const v7Full = process.env.V7_WASM_HASH_FULL;
if (!v7Full) {
    say('   ⚠ V7_WASM_HASH_FULL não informado — pulando C.');
} else {
    const saleC = await deployFrom(seller, v7Full);
    say('instância v7:', saleC.slice(0, 10) + '…');
    await call(seller, saleC, 'create', [        // v7: 13 args, SEM raise_mode
        addr(seller.publicKey()), addr(seller.publicKey()),
        addr(tokenSac), addr(USDC_SAC),
        addr(treasury.publicKey()), addr(company.publicKey()),
        i128(1n * U), u32(1), u32(1), u32(0), i128(0n), i128(0n),
    ]);
    await call(seller, tokenSac, 'mint', [addr(saleC), i128(3n * U)]);
    await call(seller, saleC, 'set_active', [nativeToScVal(true)]);
    const verBefore = await view(saleC, 'version');
    ok(Number(verBefore) === 7, 'instância roda v7 antes do upgrade');

    const compBeforeC = await usdcOf(company.publicKey());
    await call(buyerA, saleC, 'trade', [addr(buyerA.publicKey()), i128(1n * U)]);
    ok((await usdcOf(company.publicKey())) === compBeforeC + 1n * U, 'trade v7 paga a empresa NA HORA (direct)');

    await call(seller, saleC, 'upgrade', [nativeToScVal(Buffer.from(v8Hash, 'hex'), { type: 'bytes' })]);
    ok(Number(await view(saleC, 'version')) === 8, 'upgrade in-place aplicou o v8');

    const offerAfter = await view(saleC, 'get_offer');
    ok(!!offerAfter, 'get_offer AINDA desserializa o storage v7 (struct Offer intacta — regra do brick)');
    const raiseC = await view(saleC, 'get_raise');
    ok(Number(raiseC[0]) === 0, 'instância v7 lê como MODE_DIRECT (unwrap_or)');

    const compBefore2 = await usdcOf(company.publicKey());
    await call(buyerA, saleC, 'trade', [addr(buyerA.publicKey()), i128(1n * U)]);
    ok((await usdcOf(company.publicKey())) === compBefore2 + 1n * U, 'trade direct segue idêntico DEPOIS do upgrade');
    const closeOnV7 = await call(seller, saleC, 'close', [], { expectErr: true });
    ok(!!closeOnV7?.rejected, 'ciclo de escrow indisponível em instância nascida v7');
}

say('\n═══ DEVOLVENDO TROCO AO FUNDER ═══');
for (const [nome, kp] of [['A', buyerA], ['B', buyerB], ['company', company], ['treasury', treasury]]) {
    try {
        const bal = await usdcOf(kp.publicKey());
        if (bal > 0n) {
            const amt = (Number(bal) / 1e7).toFixed(7);
            await classicTx(kp, [Operation.payment({ destination: funder.publicKey(), asset: usdc, amount: amt })]);
            say(`   ${nome}: ${amt} USDC devolvidos`);
        }
    } catch (e) { say(`   ${nome}: nao devolveu (${e.message.slice(0, 50)})`); }
}

say('\n' + (process.exitCode ? '❌ HOUVE FALHAS' : '✅ TODAS AS ASSERÇÕES PASSARAM'));
process.exit(process.exitCode ? 1 : 0);
