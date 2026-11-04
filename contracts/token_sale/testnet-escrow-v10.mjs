/**
 * v10 on-chain em TESTNET (build default `testing`: F-006 OFF, USDC = ativo de teste).
 * Prova na rede real o que o unit test nao prova:
 *   - o deploy-com-__constructor(Config) com o MESMO literal nativeToScVal do backend
 *     (sorobanSale.service.js buildSaleConfigScVal) — se o mapa de tipos estiver
 *     errado, o constructor recusa aqui, nao na cerimonia;
 *   - buy() em TITULOS com preco NAO-par (0,2 USDC/titulo) e a recusa de resto != 0;
 *   - piso: fecha no captado apos o prazo; fail/refund abaixo do piso;
 *   - claim nao-par (5 titulos por 1·U de USDC), congelamento segura claim;
 *   - withdraw SEM argumento de token, sempre para o ISSUER, limitado a sobra;
 *   - isencao de min_ticket na compra que FECHA a rodada;
 *   - modo direct: entrega e pagamento imediatos; cancel recusado (InvalidMode);
 *   - VALVULA DA LEDGER: upgrade() EXISTE e e admin-only (nao-admin recusado; admin
 *     re-upgrada ao MESMO wasm com sucesso, com dinheiro dentro); updt_price()/create()
 *     NAO EXISTEM — a rede recusa por MissingValue (preco imutavel, sem create separado).
 * Fora do escopo (coberto no unit com SAC flaggeado): gate de holder autorizado.
 *
 * TESTNET APENAS. De qualquer maquina com o SDK do backend (sem chaves da Radox):
 *   cd contracts/token_sale && stellar contract build   # default = testing
 *   cd ../../backend && WASM=../contracts/token_sale/target/wasm32v1-none/release/token_sale.wasm \
 *     node --input-type=module < ../contracts/token_sale/testnet-escrow-v10.mjs
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
  catch (e) { if (expectErr) return { rejected: e.message.slice(0, 140) }; throw e; }
}
async function view(kp, id, method, args = []) {
  const acct = await RPC.getAccount(kp.publicKey());
  const tx = new TransactionBuilder(acct, { fee: BASE_FEE, networkPassphrase: P }).addOperation(new Contract(id).call(method, ...args)).setTimeout(30).build();
  const sim = await RPC.simulateTransaction(tx); if (rpc.Api.isSimulationError(sim)) throw new Error(`view ${method}: ${sim.error}`);
  return sim.result?.retval ? scValToNative(sim.result.retval) : null;
}
const addr = (a) => new Address(a).toScVal(), i128 = (v) => nativeToScVal(v, { type: 'i128' });
async function friendbot(kp) { await fetch(`https://friendbot.stellar.org?addr=${kp.publicKey()}`); await new Promise(r => setTimeout(r, 2000)); }
async function classicTx(kp, ops) { const acct = await HORIZON.loadAccount(kp.publicKey()); const tx = new TransactionBuilder(acct, { fee: '100000', networkPassphrase: P });
  ops.forEach(o => tx.addOperation(o)); const built = tx.setTimeout(60).build(); built.sign(kp);
  try { return await HORIZON.submitTransaction(built); } catch (e) { throw new Error('HORIZON ' + JSON.stringify(e?.response?.data?.extras?.result_codes ?? e.message)); } }
const bal = (kp, sac, who) => view(kp, sac, 'balance', [addr(who)]).then(v => BigInt(v ?? 0));

// ── o MESMO literal do backend (sorobanSale.service.js buildSaleConfigScVal) ──
function saleConfigScVal({ mode, admin, issuer, sellToken, buyToken, company, price, minRaise = 0n, deadlineTs = 0 }) {
  return nativeToScVal({
    mode: Number(mode), admin: new Address(admin), issuer: new Address(issuer),
    sell_token: new Address(sellToken), buy_token: new Address(buyToken), company: new Address(company),
    price: BigInt(price), min_raise: BigInt(minRaise), deadline_ts: BigInt(deadlineTs), layout_version: 0,
  }, { type: {
    mode: ['symbol', 'u32'], admin: ['symbol', 'address'], issuer: ['symbol', 'address'],
    sell_token: ['symbol', 'address'], buy_token: ['symbol', 'address'], company: ['symbol', 'address'],
    price: ['symbol', 'i128'], min_raise: ['symbol', 'i128'], deadline_ts: ['symbol', 'u64'], layout_version: ['symbol', 'u32'],
  } });
}

say('═══ SETUP ═══');
// issuerRecv = destino do withdraw (Config.issuer) SEPARADO do emissor classico do
// token: SAC transferido ao proprio emissor e BURN (saldo nao sobe), o que tornaria
// o destino improvavel de asserir — foi a falha 1/22 da primeira rodada 24/08.
const seller = Keypair.random(), buyer = Keypair.random(), company = Keypair.random(), usdcIssuer = Keypair.random(), issuerRecv = Keypair.random();
await Promise.all([seller, buyer, company, usdcIssuer, issuerRecv].map(friendbot));
const tag = Math.floor(Math.random() * 900 + 100);
const usdc = new Asset('TUSD' + tag, usdcIssuer.publicKey()), token = new Asset('TVX' + tag, seller.publicKey());
for (const kp of [buyer, company, seller]) await classicTx(kp, [Operation.changeTrust({ asset: usdc })]);
await classicTx(usdcIssuer, [Operation.payment({ destination: buyer.publicKey(), asset: usdc, amount: '10' })]);
await submit(usdcIssuer, Operation.createStellarAssetContract({ asset: usdc }));
await submit(seller, Operation.createStellarAssetContract({ asset: token }));
const USDC = usdc.contractId(P), TOKEN = token.contractId(P);
await classicTx(buyer, [Operation.changeTrust({ asset: token })]);
await classicTx(issuerRecv, [Operation.changeTrust({ asset: token })]);
say('contas + 2 SACs ok');
const wasmHash = Buffer.from(scValToNative((await submit(seller, Operation.uploadContractWasm({ wasm: readFileSync(WASM) }))).returnValue)).toString('hex');
say('v10 testing wasm:', wasmHash.slice(0, 16));

const PRICE = 2n * U; // 0,2 USDC por titulo — NAO-par de proposito
async function newSale(mode, minRaise, deadlineTs) {
  const cfg = saleConfigScVal({
    mode, admin: seller.publicKey(), issuer: issuerRecv.publicKey(), sellToken: TOKEN, buyToken: USDC,
    company: company.publicKey(), price: PRICE, minRaise, deadlineTs,
  });
  const got = await submit(seller, Operation.createCustomContract({
    address: new Address(seller.publicKey()), wasmHash: Buffer.from(wasmHash, 'hex'),
    salt: crypto.randomBytes(32), constructorArgs: [cfg],
  }));
  const id = scValToNative(got.returnValue);
  await call(seller, TOKEN, 'mint', [addr(id), i128(10n * U)]);   // 1,0 titulo depositado
  // set_active e ato do ISSUER (lib.rs:215 — ele atesta o deposito que congela o alvo);
  // com issuer separado do seller, a rodada 2/3 falhou auth aqui ate assinar com ele.
  await call(issuerRecv, id, 'set_active', [nativeToScVal(true)]); // Target congela do saldo: 10·U titulos
  return id;
}
const now = Math.floor(Date.now() / 1000);
const deadline = now + 180; // 3 min
say('agora', now, '→ prazo', deadline);
// custo(t) = t × PRICE / 1e7 → 10·U titulos custam 2·U de USDC; piso 1·U = 5·U titulos
const A = await newSale(1, 1n * U, deadline);      // escrow: acima do piso, abaixo do alvo
const B = await newSale(1, 1n * U, deadline);      // escrow: abaixo do piso
const C = await newSale(1, 1n * U, now + 3600);    // escrow: enche antes do prazo
const D = await newSale(0, 0n, 0);                 // direct
say('sales:', { A, B, C, D });
ok(Number(await view(seller, A, 'version')) === 10, 'version() == 10');
const cfgA = await view(seller, A, 'get_config');
ok(cfgA.price === PRICE && cfgA.min_raise === 1n * U, 'get_config: preco e piso gravados pelo __constructor (literal do backend aceito na rede)');
ok((await view(seller, A, 'get_round'))[2] === 10n * U, 'get_round: alvo congelado do deposito no set_active');

say('═══ A: acima do piso, abaixo do alvo ═══');
ok((await call(buyer, A, 'buy', [addr(buyer.publicKey()), i128(1n)], { expectErr: true })).rejected, 'buy(1 stroop): resto != 0 recusado (RemainderNotZero)');
await call(buyer, A, 'buy', [addr(buyer.publicKey()), i128(5n * U)]);
const rA = await view(seller, A, 'get_round');
ok(rA[3] === 5n * U && rA[4] === 1n * U && rA[5] === 5n * U, 'buy em titulos: sold=5·U, raised=1·U (5·U×0,2), pendentes=5·U');
ok((await view(seller, A, 'get_contrib', [addr(buyer.publicKey())])) === 1n * U, 'get_contrib registra o CUSTO pago (1·U)');
ok((await call(buyer, A, 'close', [], { expectErr: true })).rejected, 'close() antes do prazo recusado (TargetNotReached)');
ok((await call(seller, A, 'withdraw', [i128(1n * U)], { expectErr: true })).rejected, 'withdraw() bloqueado com a rodada aberta');
// VALVULA DA LEDGER (decisao do Pedro 24/08): upgrade() EXISTE e e admin-only.
// (a) nao-admin (buyer) e recusado por auth, MESMO com dinheiro dentro;
ok((await call(buyer, A, 'upgrade', [nativeToScVal(Buffer.from(wasmHash, 'hex'), { type: 'bytes' })], { expectErr: true })).rejected, 'upgrade() recusado para nao-admin (auth)');
// (b) o admin (=seller aqui) re-upgrada ao MESMO wasm ja instalado -> SUCESSO, com USDC escrowado dentro.
//     Prova que a valvula existe, passa a auth e nao trava com dinheiro (o deadlock morre aqui).
await call(seller, A, 'upgrade', [nativeToScVal(Buffer.from(wasmHash, 'hex'), { type: 'bytes' })]);
ok(Number(await view(seller, A, 'version')) === 10, 'admin fez upgrade com dinheiro dentro; contrato segue vivo (version 10)');
ok((await call(seller, A, 'updt_price', [i128(1n)], { expectErr: true })).rejected, 'updt_price() NAO EXISTE no v10 — preco e imutavel');

say('═══ B: abaixo do piso ═══');
await call(buyer, B, 'buy', [addr(buyer.publicKey()), i128(2n * U)]);   // 0,4·U < piso 1·U

say('═══ C: isencao de min_ticket na compra que fecha ═══');
await call(seller, C, 'set_terms', [i128(5n * U), i128(0n)]);           // min_ticket 0,5 USDC > custo total 0,2
const compBefore = await bal(seller, USDC, company.publicKey());
await call(buyer, C, 'buy', [addr(buyer.publicKey()), i128(10n * U)]);  // fecha a rodada → isento
await call(buyer, C, 'close', []);
ok((await bal(seller, USDC, company.publicKey())) - compBefore === 2n * U, 'C: encheu e fechou antes do prazo; empresa recebeu 2·U; min_ticket nao barrou a compra que FECHA');

say('═══ D: direct — entrega e pagamento imediatos ═══');
const compD = await bal(seller, USDC, company.publicKey());
await call(buyer, D, 'buy', [addr(buyer.publicKey()), i128(5n * U)]);
ok((await bal(seller, TOKEN, buyer.publicKey())) === 5n * U, 'D: titulos entregues NA COMPRA');
ok((await bal(seller, USDC, company.publicKey())) - compD === 1n * U, 'D: empresa paga NA COMPRA');
ok((await call(seller, D, 'cancel', [], { expectErr: true })).rejected, 'D: cancel() recusado em direct (InvalidMode)');

say(`… esperando o prazo (ts > ${deadline})`);
while (Math.floor(Date.now() / 1000) <= deadline + 10) await new Promise(r => setTimeout(r, 5000));

say('═══ A depois do prazo ═══');
ok((await call(seller, A, 'cancel', [], { expectErr: true })).rejected, 'A: cancel() recusado — rodada fechavel pertence aos investidores');
const cb = await bal(seller, USDC, company.publicKey());
await call(buyer, A, 'close', []);
ok((await bal(seller, USDC, company.publicKey())) - cb === 1n * U, 'A: fechou no piso; empresa recebeu o CAPTADO (1·U), nao o alvo');
ok((await call(seller, A, 'withdraw', [i128(6n * U)], { expectErr: true })).rejected, 'A: sacar mais que a sobra (5·U) recusado');
const recvTok = await bal(seller, TOKEN, issuerRecv.publicKey());
await call(seller, A, 'withdraw', [i128(5n * U)]);
ok((await bal(seller, TOKEN, issuerRecv.publicKey())) - recvTok === 5n * U, 'A: withdraw sem arg de token — a sobra chegou no ISSUER do Config (nao no admin)');
await call(seller, A, 'set_frozen', [addr(buyer.publicKey()), nativeToScVal(true)]);
ok((await call(buyer, A, 'claim', [addr(buyer.publicKey())], { expectErr: true })).rejected, 'A: congelado nao faz claim');
await call(seller, A, 'set_frozen', [addr(buyer.publicKey()), nativeToScVal(false)]);
await call(buyer, A, 'claim', [addr(buyer.publicKey())]);
ok((await bal(seller, TOKEN, buyer.publicKey())) === 10n * U, 'A: claim entregou 5·U titulos por 1·U de USDC (conversao nao-par exata; 5·U do D)');
ok((await bal(seller, TOKEN, A)) === 0n, 'A: contrato zerado (sobra + devidos = deposito)');

say('═══ B depois do prazo ═══');
ok((await call(buyer, B, 'close', [], { expectErr: true })).rejected, 'B: close() abaixo do piso recusado');
await call(buyer, B, 'fail', []);
const bb = await bal(seller, USDC, buyer.publicKey());
await call(buyer, B, 'refund', [addr(buyer.publicKey())]);
ok((await bal(seller, USDC, buyer.publicKey())) - bb === 400_000n, 'B: refund devolveu os 0,4·U inteiros');
ok((await view(seller, B, 'get_round'))[4] === 0n, 'B: raised = 0 depois do refund');

say(fails ? `\n❌ ${fails} falha(s)` : '\n✅ v10 validado na testnet: constructor do backend, buy em titulos, piso, isencao no fechamento, direct, claim nao-par, withdraw→issuer, fail/refund');
process.exit(fails ? 1 : 0);
