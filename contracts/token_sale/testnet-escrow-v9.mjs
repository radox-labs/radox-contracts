/**
 * v9 on-chain em TESTNET (build --features testing: gate F-006 OFF, USDC = ativo de teste).
 * Prova na rede real o que o unit test nao prova: a assinatura de 14 args do create(),
 * o fechamento no PISO depois do prazo, a sobra sacavel, o claim, o fail/refund abaixo
 * do piso e a trava do upgrade() com dinheiro dentro. Contas descartaveis via friendbot.
 *
 * TESTNET APENAS. Roda de qualquer maquina com o SDK do backend (sem chaves da Radox):
 *   cd backend && stellar contract build  # (default = testing) em contracts/token_sale
 *   WASM=../contracts/token_sale/target/wasm32v1-none/release/token_sale.wasm \
 *     node --input-type=module < ../contracts/token_sale/testnet-escrow-v9.mjs
 * Rodado 23/08/2026: ver CEREMONY_CSLEASE1_TERMS.md (secao 'Validacao on-chain').
 */
import { Keypair, Asset, Operation, TransactionBuilder, Networks, BASE_FEE, Contract, Address, nativeToScVal, scValToNative, rpc, Horizon } from '@stellar/stellar-sdk';
import { readFileSync } from 'node:fs';
import crypto from 'node:crypto';
const RPC = new rpc.Server('https://soroban-testnet.stellar.org');
const HORIZON = new Horizon.Server('https://horizon-testnet.stellar.org');
const P = Networks.TESTNET; const U = 1_000_000n; // 0,1 unidade (7 decimais)
const WASM = process.argv[2] ?? process.env.WASM;
const say = (...a) => console.log(...a); let fails = 0;
const ok = (c, m) => { if (!c) { fails++; console.error('❌ FALHOU:', m); } else say('   ✓', m); };
async function submit(kp, op, { fee = '2000000' } = {}) {
  const acct = await RPC.getAccount(kp.publicKey());
  let tx = new TransactionBuilder(acct, { fee, networkPassphrase: P }).addOperation(op).setTimeout(60).build();
  const sim = await RPC.simulateTransaction(tx);
  if (rpc.Api.isSimulationError(sim)) throw new Error('sim: ' + sim.error);
  if (sim.result || sim.transactionData) tx = rpc.assembleTransaction(tx, sim).build();
  tx.sign(kp);
  const sent = await RPC.sendTransaction(tx);
  if (sent.status === 'ERROR') throw new Error('send: ' + JSON.stringify(sent.errorResult));
  for (let i = 0; i < 30; i++) { await new Promise(r => setTimeout(r, 1500)); const got = await RPC.getTransaction(sent.hash);
    if (got.status === 'SUCCESS') return got; if (got.status === 'FAILED') throw new Error('tx FAILED: ' + sent.hash); }
  throw new Error('timeout ' + sent.hash);
}
async function call(kp, id, method, args, { expectErr = false } = {}) {
  try { const got = await submit(kp, new Contract(id).call(method, ...args)); const ret = got.returnValue ? scValToNative(got.returnValue) : null;
    if (expectErr) { fails++; console.error(`❌ ${method} deveria ter falhado e passou`); } return ret; }
  catch (e) { if (expectErr) return { rejected: e.message.slice(0, 120) }; throw e; }
}
async function view(kp, id, method, args = []) {
  const acct = await RPC.getAccount(kp.publicKey());
  const tx = new TransactionBuilder(acct, { fee: BASE_FEE, networkPassphrase: P }).addOperation(new Contract(id).call(method, ...args)).setTimeout(30).build();
  const sim = await RPC.simulateTransaction(tx); if (rpc.Api.isSimulationError(sim)) throw new Error(`view ${method}: ${sim.error}`);
  return sim.result?.retval ? scValToNative(sim.result.retval) : null;
}
const addr = (a) => new Address(a).toScVal(), i128 = (v) => nativeToScVal(v, { type: 'i128' }), u32 = (v) => nativeToScVal(v, { type: 'u32' });
async function friendbot(kp) { await fetch(`https://friendbot.stellar.org?addr=${kp.publicKey()}`); await new Promise(r => setTimeout(r, 2000)); }
async function classicTx(kp, ops) { const acct = await HORIZON.loadAccount(kp.publicKey()); const tx = new TransactionBuilder(acct, { fee: '100000', networkPassphrase: P });
  ops.forEach(o => tx.addOperation(o)); const built = tx.setTimeout(60).build(); built.sign(kp);
  try { return await HORIZON.submitTransaction(built); } catch (e) { throw new Error('HORIZON ' + JSON.stringify(e?.response?.data?.extras?.result_codes ?? e.message)); } }
async function deployFrom(kp, hashHex) { const got = await submit(kp, Operation.createCustomContract({ address: new Address(kp.publicKey()), wasmHash: Buffer.from(hashHex, 'hex'), salt: crypto.randomBytes(32) })); return scValToNative(got.returnValue); }
const bal = (kp, sac, who) => view(kp, sac, 'balance', [addr(who)]).then(v => BigInt(v ?? 0));
const latestSeq = async () => (await RPC.getLatestLedger()).sequence;
async function waitPast(seq) { for (;;) { const n = await latestSeq(); if (n > seq) return n; await new Promise(r => setTimeout(r, 4000)); } }

say('═══ SETUP ═══');
const seller = Keypair.random(), buyer = Keypair.random(), company = Keypair.random(), treasury = Keypair.random(), usdcIssuer = Keypair.random();
await Promise.all([seller, buyer, company, treasury, usdcIssuer].map(friendbot));
const tag = Math.floor(Math.random() * 900 + 100);
const usdc = new Asset('TUSD' + tag, usdcIssuer.publicKey()), token = new Asset('TV9' + tag, seller.publicKey());
for (const kp of [buyer, company, treasury, seller]) await classicTx(kp, [Operation.changeTrust({ asset: usdc })]);
await classicTx(usdcIssuer, [Operation.payment({ destination: buyer.publicKey(), asset: usdc, amount: '10' })]);
await submit(usdcIssuer, Operation.createStellarAssetContract({ asset: usdc }));
await submit(seller, Operation.createStellarAssetContract({ asset: token }));
const USDC = usdc.contractId(P), TOKEN = token.contractId(P);
await classicTx(buyer, [Operation.changeTrust({ asset: token })]);
say('contas + 2 SACs ok');
const hash = Buffer.from(scValToNative((await submit(seller, Operation.uploadContractWasm({ wasm: readFileSync(WASM) }))).returnValue)).toString('hex');
say('v9 testing wasm:', hash.slice(0, 16));
async function newSale(minRaise, deadline) {
  const id = await deployFrom(seller, hash);
  await call(seller, id, 'create', [addr(seller.publicKey()), addr(seller.publicKey()), addr(TOKEN), addr(USDC), addr(treasury.publicKey()), addr(company.publicKey()),
    i128(0n), u32(1), u32(1), u32(deadline), i128(0n), i128(0n), u32(1), i128(minRaise)]);
  await call(seller, TOKEN, 'mint', [addr(id), i128(10n * U)]);       // deposita 10 tokens → alvo 10·U
  await call(seller, id, 'set_active', [nativeToScVal(true)]);
  return id;
}
const start = await latestSeq(); const deadline = start + 28; // ~2,5 min
say(`ledger ${start}, prazo ${deadline}`);
const A = await newSale(4n * U, deadline), B = await newSale(4n * U, deadline), C = await newSale(4n * U, deadline + 400);
say('sales:', { A, B, C });
ok(Number(await view(seller, A, 'version')) === 9, 'version() == 9');
ok((await view(seller, A, 'get_floor'))[0] === 4n * U, 'get_floor: piso gravado no create (14º argumento aceito na rede)');
say('═══ A: acima do piso, abaixo do alvo ═══');
await call(buyer, A, 'trade', [addr(buyer.publicKey()), i128(5n * U)]);
const fA = await view(seller, A, 'get_floor'); ok(fA[1] === 5n * U && fA[2] === 5n * U, 'owed=5, held=5 depois da compra');
ok((await call(buyer, A, 'close', [], { expectErr: true })).rejected, 'close() antes do prazo recusado (TargetNotReached)');
ok((await call(seller, A, 'upgrade', [nativeToScVal(Buffer.alloc(32, 7), { type: 'bytes' })], { expectErr: true })).rejected, 'upgrade() recusado com dinheiro dentro');
ok((await call(seller, A, 'withdraw', [addr(TOKEN), i128(1n * U)], { expectErr: true })).rejected, 'withdraw() recusado com rodada aberta');
say('═══ B: abaixo do piso ═══');
await call(buyer, B, 'trade', [addr(buyer.publicKey()), i128(3n * U)]);
say('═══ C: cheia antes do prazo ═══');
await call(buyer, C, 'trade', [addr(buyer.publicKey()), i128(10n * U)]);
const compBefore = await bal(seller, USDC, company.publicKey());
await call(buyer, C, 'close', []);
ok((await bal(seller, USDC, company.publicKey())) - compBefore === 10n * U, 'C: cheia fecha antes do prazo, empresa recebe 10');
say(`… esperando o prazo (ledger > ${deadline})`); await waitPast(deadline);
say('═══ A depois do prazo ═══');
ok((await call(seller, A, 'cancel', [], { expectErr: true })).rejected, 'A: cancel() recusado — rodada fechável pertence aos investidores');
const cb = await bal(seller, USDC, company.publicKey());
await call(buyer, A, 'close', []);
ok((await bal(seller, USDC, company.publicKey())) - cb === 5n * U, 'A: fechou no piso, empresa recebeu o CAPTADO (5), não o alvo');
ok((await view(seller, A, 'get_floor'))[2] === 0n, 'A: held = 0 depois do close');
ok((await call(seller, A, 'withdraw', [addr(TOKEN), i128(6n * U)], { expectErr: true })).rejected, 'A: sacar mais que a sobra (5) recusado');
await call(seller, A, 'withdraw', [addr(TOKEN), i128(5n * U)]);
ok((await bal(seller, TOKEN, A)) === 5n * U, 'A: a sobra (5) saiu do contrato; ficaram exatamente os 5 devidos ao investidor');
ok((await call(seller, A, 'upgrade', [nativeToScVal(Buffer.alloc(32, 7), { type: 'bytes' })], { expectErr: true })).rejected, 'A: upgrade() ainda travado com claims pendentes');
await call(buyer, A, 'claim_tokens', [addr(buyer.publicKey())]);
ok((await bal(seller, TOKEN, buyer.publicKey())) === 5n * U, 'A: investidor recebeu 5 tokens no claim');
ok((await view(seller, A, 'get_floor'))[1] === 0n, 'A: owed = 0 depois do claim');
ok((await bal(seller, TOKEN, A)) === 0n, 'A: contrato zerado de tokens depois do claim (sobra + devidos = deposito)');
say('═══ B depois do prazo ═══');
ok((await call(buyer, B, 'close', [], { expectErr: true })).rejected, 'B: close() abaixo do piso recusado');
await call(buyer, B, 'fail', []);
ok((await call(seller, B, 'upgrade', [nativeToScVal(Buffer.alloc(32, 7), { type: 'bytes' })], { expectErr: true })).rejected, 'B: upgrade() travado com refund pendente');
const bb = await bal(seller, USDC, buyer.publicKey());
await call(buyer, B, 'refund', [addr(buyer.publicKey())]);
ok((await bal(seller, USDC, buyer.publicKey())) - bb === 3n * U, 'B: refund devolveu os 3 inteiros (fee zero)');
ok((await view(seller, B, 'get_floor'))[2] === 0n, 'B: held = 0 depois do refund');
say(fails ? `\n❌ ${fails} falha(s)` : '\n✅ v9 validado na testnet: piso, sobra, claim, fail/refund, trava do upgrade');
process.exit(fails ? 1 : 0);
