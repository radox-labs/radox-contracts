// node --test scripts/test/registry.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  cargoFeatures, checkAnchor, checkCoreLists, checkDoc, checkFeatures, checkRepo, entries, isContractStrkey, parseAnchor, parseDoc,
  query, readCoreLists, validateSchema,
} from "../lib/registry.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const REG_FILE = path.join(ROOT, "deployments", "registry.json");
const CUSTODY_DOC = path.join(ROOT, "contracts", "custody_registry", "DEPLOYMENTS.md");
const real = () => JSON.parse(fs.readFileSync(REG_FILE, "utf8"));
const H = (c) => c.repeat(64);
const ID = (c) => "C" + c.repeat(55);
const entry = (o = {}) => ({ wasmHash: H("a"), contractId: null, version: 1, features: "testnet", status: "live", source: "test", ...o });
const reg = (networks) => ({ schema: 1, networks });

// Independent strkey encoder (SEP-23): version byte 2 << 3 ('C'), 32-byte payload, CRC16-XModem little-endian, base32 no padding.
function encodeContract(payload) {
  const data = [2 << 3, ...payload];
  let crc = 0;
  for (const b of data) {
    for (let i = 7; i >= 0; i--) {
      const bit = ((b >> i) & 1) ^ ((crc >> 15) & 1);
      crc = (crc << 1) & 0xffff;
      if (bit) crc ^= 0x1021;
    }
  }
  const bytes = [...data, crc & 0xff, crc >> 8];
  const A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = 0, acc = 0, out = "";
  for (const b of bytes) {
    acc = (acc << 8) | b; bits += 8;
    while (bits >= 5) { out += A[(acc >> (bits - 5)) & 31]; bits -= 5; }
  }
  if (bits > 0) out += A[(acc << (5 - bits)) & 31];
  return out;
}
const SID = (n) => encodeContract(Array.from({ length: 32 }, (_, i) => (n * 31 + i) & 0xff));
const flip = (str, pos, alphabet) => str.slice(0, pos) + alphabet[(alphabet.indexOf(str[pos]) + 1) % alphabet.length] + str.slice(pos + 1);
const HEXA = "0123456789abcdef";
const B32A = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

test("the committed registry and docs pass", () => {
  const { errors, summary } = checkRepo(ROOT);
  assert.deepEqual(errors, []);
  assert.ok(summary.counts.testnet > 0 && summary.counts.mainnet > 0);
  assert.ok(summary.docs.includes("contracts/custody_registry/DEPLOYMENTS.md"));
});

test("the committed registry parses and every entry cites a source", () => {
  const all = entries(JSON.parse(fs.readFileSync(REG_FILE, "utf8")));
  assert.ok(all.length > 0);
  for (const e of all) assert.match(e.source, /\S{8,}/, `${e.network}.${e.crate}[${e.index}]`);
});

// Each mutation must be refused, alone, on a registry that is otherwise valid.
const BAD = [
  ["schema 2", (r) => { r.schema = 2; }, /"schema" must be 1/],
  ["extra top-level key", (r) => { r.extra = 1; }, /unknown top-level key/],
  ["unknown network", (r) => { r.networks.futurenet = { token_sale: [entry()] }; }, /unknown network/],
  ["unknown crate", (r) => { r.networks.testnet.foo = [entry()]; }, /unknown crate/],
  ["empty crate list", (r) => { r.networks.testnet.token_sale = []; }, /non-empty array/],
  ["uppercase hash", (r) => { r.networks.testnet.token_sale[0].wasmHash = H("A"); }, /wasmHash/],
  ["63-char hash", (r) => { r.networks.testnet.token_sale[0].wasmHash = "a".repeat(63); }, /wasmHash/],
  ["G address as contractId", (r) => { r.networks.testnet.token_sale[0].contractId = "G" + "A".repeat(55); }, /contractId/],
  ["short contractId", (r) => { r.networks.testnet.token_sale[0].contractId = "CBIDF7ZW"; }, /contractId/],
  ["PER-OFFER sentinel", (r) => { r.networks.testnet.token_sale[0].contractId = "PER-OFFER"; }, /contractId/],
  ["version as string", (r) => { r.networks.testnet.token_sale[0].version = "7"; }, /version/],
  ["negative version", (r) => { r.networks.testnet.token_sale[0].version = -1; }, /version/],
  ["fractional version", (r) => { r.networks.testnet.token_sale[0].version = 1.5; }, /version/],
  ["unknown features", (r) => { r.networks.testnet.token_sale[0].features = "default"; }, /features/],
  ["features as array", (r) => { r.networks.testnet.token_sale[0].features = ["testnet"]; }, /features/],
  ["unknown status", (r) => { r.networks.testnet.token_sale[0].status = "staged"; }, /status/],
  ["missing key", (r) => { delete r.networks.testnet.token_sale[0].version; }, /missing "version"/],
  ["extra key", (r) => { r.networks.testnet.token_sale[0].deployedAt = "x"; }, /unknown key "deployedAt"/],
  ["empty source", (r) => { r.networks.testnet.token_sale[0].source = " "; }, /source/],
  ["testnet build on mainnet", (r) => { r.networks.mainnet.token_sale[0].features = "testnet"; }, /features "testnet" on mainnet/],
  ["testing build on mainnet", (r) => { r.networks.mainnet.token_sale[0].features = "testing"; }, /features "testing" on mainnet/],
  ["duplicate contractId", (r) => { r.networks.testnet.custody_registry.push(entry({ wasmHash: H("c"), contractId: SID(1), features: null })); }, /already listed/],
  ["duplicate code-only entry", (r) => { r.networks.testnet.token_sale.push(entry()); }, /without contractId already listed/],
  ["same wasm, other version", (r) => { Object.assign(r.networks.mainnet.token_sale[0], { wasmHash: H("a"), features: null, version: 2 }); }, /disagrees/],
  ["same wasm, other features", (r) => { r.networks.mainnet.token_sale[0].wasmHash = H("a"); }, /features: "mainnet" disagrees/],
  ["same wasm, other crate", (r) => { r.networks.testnet.yield_distributor = [entry({ wasmHash: H("a"), features: "testnet" })]; }, /also listed under crate/],
];

const base = () => reg({
  testnet: {
    token_sale: [entry()],
    custody_registry: [entry({ wasmHash: H("b"), contractId: SID(1), features: null })],
  },
  mainnet: { token_sale: [entry({ features: "mainnet", status: "live" })] },
});

test("the base fixture is valid", () => {
  // same wasm on two networks with different features would fail; the base keeps them apart
  const r = base();
  r.networks.mainnet.token_sale[0].wasmHash = H("d");
  assert.deepEqual(validateSchema(r), []);
});

for (const [name, mutate, re] of BAD) {
  test(`schema refuses: ${name}`, () => {
    const r = base();
    r.networks.mainnet.token_sale[0].wasmHash = H("d");
    mutate(r);
    const errs = validateSchema(r);
    assert.ok(errs.some((e) => re.test(e)), `expected ${re} in ${JSON.stringify(errs)}`);
  });
}

test("schema refuses a broken field in every real entry (family sweep)", () => {
  const n = entries(real()).length;
  for (const field of ["wasmHash", "contractId", "version", "features", "status", "source"]) {
    for (let i = 0; i < n; i++) {
      const r = real();
      const e = entries(r)[i];
      r.networks[e.network][e.crate][e.index][field] = field === "version" ? "x" : field === "source" ? "" : "BAD";
      assert.ok(validateSchema(r).length > 0, `${field} of ${e.network}.${e.crate}[${e.index}] not checked`);
    }
  }
});

test("parseDoc: sections, CRLF, abbreviations; no G addresses or longer hex runs", () => {
  const text = [
    "# t",
    `wasm \`${H("a")}\` and \`aaaaaaaa…aaaa\` and aaaaaaaa...aaaa`,
    "## mainnet",
    `| contract | \`${ID("B")}\` |`,
    `| admin | \`G${"A".repeat(55)}\` |`,
    "### testnet instances",
    `${ID("C")} next to a longer run ${"a".repeat(65)}`,
    "## Notes",
    `${ID("D")}`,
    "Prose: CONFIRM THE CONTRACT ID AND THE WASM HASH ON THE NETWORK BEFORE ANY CALL.",
  ].join("\r\n");
  const d = parseDoc(text);
  assert.deepEqual(d.hashes.map((t) => [t.value, t.network]), [[H("a"), null]]);
  assert.equal(d.shortHashes.length, 2);
  assert.deepEqual(d.contractIds.map((t) => [t.value, t.network, t.line]), [[ID("B"), "mainnet", 4], [ID("C"), "testnet", 7], [ID("D"), null, 9]]);
});

test("parseDoc on the real custody doc finds only real tokens", () => {
  const d = parseDoc(fs.readFileSync(CUSTODY_DOC, "utf8"));
  const r = real();
  const ids = new Set(entries(r).map((e) => e.contractId).filter(Boolean));
  assert.ok(d.contractIds.length >= 5);
  for (const t of d.contractIds) assert.ok(ids.has(t.value), `${t.value} line ${t.line}`);
  // one wasm hash and one deploy tx, nothing else
  assert.deepEqual([...new Set(d.hashes.map((t) => t.value))].sort(), [
    "8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342",
    "f481387089e395e8e64f038aee3b00a672708bec76b421b472f2dbdc3435604a",
  ]);
});

const docReg = () => reg({
  testnet: { custody_registry: [entry({ wasmHash: H("a"), contractId: ID("T"), features: null, source: `deploy tx ${H("e")}` })] },
  mainnet: { custody_registry: [entry({ wasmHash: H("a"), contractId: ID("M"), features: null })] },
});
const goodDoc = [`wasm \`${H("a")}\` (\`aaaaaaaa…aaaa\`)`, "## mainnet", ID("M"), "## testnet", ID("T"), `tx ${H("e")}`].join("\n");

test("checkDoc: a consistent doc passes", () => {
  assert.deepEqual(checkDoc(docReg(), "custody_registry", goodDoc), []);
});

const DOC_BAD = [
  ["unknown hash", goodDoc + `\n${H("f")}`, /hash f{64} is not in deployments\/registry.json/],
  ["tx of the other network", goodDoc.replace("## testnet", "## testnet\n").replace(`tx ${H("e")}`, "") + `\n## mainnet\ntx ${H("e")}`, /registered for custody_registry only on testnet/],
  ["unknown contract", goodDoc + `\n${ID("X")}`, /contract CX+ is not in/],
  ["contract under the wrong network", goodDoc.replace(`## mainnet\n${ID("M")}`, `## mainnet\n${ID("T")}`), /only on testnet/],
  ["abbreviation with no match", goodDoc + "\nbbbbbbbb…bbbb", /matches 0 wasmHash/],
  ["live contract missing", goodDoc.replace(ID("T"), ""), /live contract CT+ \(testnet\) is not in/],
  ["live wasm not cited in full", goodDoc.replace(`\`${H("a")}\` `, ""), /live wasm a{64} is not cited in full/],
];
for (const [name, text, re] of DOC_BAD) {
  test(`checkDoc refuses: ${name}`, () => {
    const errs = checkDoc(docReg(), "custody_registry", text);
    assert.ok(errs.some((e) => re.test(e)), `expected ${re} in ${JSON.stringify(errs)}`);
  });
}

test("checkDoc: a legacy entry does not have to be in the doc", () => {
  const r = docReg();
  r.networks.testnet.custody_registry.push(entry({ wasmHash: H("a"), contractId: ID("L"), features: null, status: "legacy" }));
  assert.deepEqual(checkDoc(r, "custody_registry", goodDoc), []);
});

test("real custody doc: removing any live contract ID or the live wasm fails (family sweep)", () => {
  const r = real();
  const text = fs.readFileSync(CUSTODY_DOC, "utf8");
  const live = entries(r).filter((e) => e.crate === "custody_registry" && e.status === "live");
  assert.ok(live.length >= 5);
  for (const e of live) {
    assert.ok(checkDoc(r, "custody_registry", text.split(e.contractId).join("")).length > 0, e.contractId);
  }
  assert.ok(checkDoc(r, "custody_registry", text.split(live[0].wasmHash).join("")).length > 0);
});

test("real registry: dropping any custody entry cited in the doc fails", () => {
  const text = fs.readFileSync(CUSTODY_DOC, "utf8");
  const n = real().networks.testnet.custody_registry.length + real().networks.mainnet.custody_registry.length;
  for (let i = 0; i < n; i++) {
    const r = real();
    const all = entries(r).filter((e) => e.crate === "custody_registry");
    const e = all[i];
    r.networks[e.network].custody_registry.splice(e.index, 1);
    assert.ok(checkDoc(r, "custody_registry", text).length > 0, `${e.network} ${e.contractId}`);
  }
});

test("query filters by crate and network", () => {
  const r = real();
  assert.ok(query(r, "custody_registry").length >= 5);
  assert.ok(query(r, "custody_registry", "mainnet").every((e) => e.network === "mainnet"));
  assert.deepEqual(query(r, "recovery_policy", "mainnet"), []);
});

const node = process.execPath;
const run = (script, args) => spawnSync(node, [path.join(ROOT, "scripts", script), ...args], { encoding: "utf8" });

test("CLI check-registry: ok on the repo, exit 1 with a clear message on a broken file", () => {
  const ok = run("check-registry.mjs", []);
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /check-registry: ok/);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "registry-test-"));
  try {
    const bad = real();
    bad.networks.testnet.token_sale[0].wasmHash = "nothex";
    const f = path.join(dir, "registry.json");
    fs.writeFileSync(f, JSON.stringify(bad));
    const res = run("check-registry.mjs", ["--registry", f]);
    assert.equal(res.status, 1);
    assert.match(res.stderr, /FAIL \(1 problem\)[\s\S]*networks\.testnet\.token_sale\[0\]\.wasmHash/);
    fs.writeFileSync(f, "{");
    assert.match(run("check-registry.mjs", ["--registry", f]).stderr, /cannot read\/parse/);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("CLI caatinga query: rows, json and usage errors", () => {
  const res = run("caatinga.mjs", ["query", "yield_distributor", "--network", "mainnet"]);
  assert.equal(res.status, 0, res.stderr);
  assert.match(res.stdout, /^mainnet live\s+v4\s+mainnet 88509c72/m);
  const js = JSON.parse(run("caatinga.mjs", ["query", "custody_registry", "--json"]).stdout);
  assert.ok(js.length >= 5 && js.every((e) => e.crate === "custody_registry" && !("index" in e)));
  assert.equal(run("caatinga.mjs", ["query", "nope"]).status, 2);
  assert.equal(run("caatinga.mjs", ["query", "token_sale", "--network", "devnet"]).status, 2);
  assert.equal(run("caatinga.mjs", []).status, 2);
});

test("cargoFeatures reads the real Cargo.toml of every crate", () => {
  const read = (c) => cargoFeatures(fs.readFileSync(path.join(ROOT, "contracts", c, "Cargo.toml"), "utf8")).sort();
  for (const c of ["token_sale", "maturity_settlement", "recovery_policy"]) assert.deepEqual(read(c), ["mainnet", "testing", "testnet"], c);
  assert.deepEqual(read("yield_distributor"), ["mainnet", "testing", "testnet", "wasm-tests"]);
  assert.deepEqual(read("transfer_agent"), ["testing"]);
  assert.deepEqual(read("custody_registry"), []);
});

test("checkFeatures refuses a network feature on a crate that does not declare it (every real entry)", () => {
  const declared = { token_sale: ["testnet", "mainnet", "testing"], transfer_agent: ["testing"], custody_registry: [] };
  assert.deepEqual(checkFeatures(real(), declared), []);
  for (const e of entries(real()).filter((x) => x.crate === "transfer_agent" || x.crate === "custody_registry")) {
    const r = real();
    r.networks[e.network][e.crate][e.index].features = "testnet";
    assert.ok(checkFeatures(r, declared).some((m) => /not declared/.test(m)), `${e.crate}[${e.index}]`);
  }
});

// --- anchors, strkey, Core lists (qa r1, 27/11/2026) ---

test("the encoder in this test reproduces every real contract ID (so it is a valid oracle)", () => {
  const ids = [...new Set(entries(real()).map((e) => e.contractId).filter(Boolean))];
  assert.equal(ids.length, 14);
  for (const id of ids) assert.ok(isContractStrkey(id), id);
  assert.ok(isContractStrkey(SID(1)) && isContractStrkey(SID(2)));
});

test("strkey: every 1-character change of every real contract ID is refused", () => {
  for (const id of new Set(entries(real()).map((e) => e.contractId).filter(Boolean))) {
    for (let pos = 1; pos < id.length; pos++) assert.equal(isContractStrkey(flip(id, pos, B32A)), false, `${id} @${pos}`);
  }
  assert.equal(isContractStrkey("G" + SID(1).slice(1)), false);
});

test("schema refuses the same contractId on two networks", () => {
  const r = real();
  r.networks.testnet.recovery_policy[0].contractId = r.networks.mainnet.yield_distributor[0].contractId;
  assert.ok(validateSchema(r).some((e) => /is also listed on (testnet|mainnet)/.test(e)));
});

test("mainnet is exactly these 4 (crate, wasm, contract, version, features, status)", () => {
  const got = entries(real()).filter((e) => e.network === "mainnet").map((e) => [e.crate, e.wasmHash, e.contractId, e.version, e.features, e.status]).sort();
  assert.deepEqual(got, [
    ["custody_registry", "8230ed937f443e40054d0806c9cdd044ef341db19f0c1b3123c573adb7a41342", "CDV7YI7KYTDH4LPOLVUVRTFERJKYOWBBT6KS56ZX3V7EOGDBWS7LD4UP", 1, null, "live"],
    ["maturity_settlement", "0f07a598439581a0fc5990d4d1cbc4e2b7447109c505295ad9f7268790028cbf", null, 3, "mainnet", "live"],
    ["token_sale", "524ede1c6122b15456329564878a191a00a7062e1a8c9009deaa44eeb5a59c57", null, 7, "mainnet", "live"],
    ["yield_distributor", "88509c72c0a82812b8f110b04bcd2dd3f734a43aa4b8bb18a28160ca9b9e4203", "CDTXAGKIABYIKCRBD5CBEDYLVYZEEL5R5P4JT25AS4QHQFY2UGLTIMNX", 4, "mainnet", "live"],
  ]);
});

// Mutation sweep over the committed registry: each changed hash / ID must make the full repo check fail.
function sweep(field, alphabet, startPos) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "registry-sweep-"));
  const f = path.join(dir, "registry.json");
  let total = 0, caught = 0;
  const missed = [];
  try {
    for (const e of entries(real())) {
      if (!e[field]) continue;
      total++;
      const r = real();
      const v = e[field];
      const pos = startPos + (total * 7) % (v.length - startPos);
      r.networks[e.network][e.crate][e.index][field] = flip(v, pos, alphabet);
      fs.writeFileSync(f, JSON.stringify(r));
      if (checkRepo(ROOT, f).errors.length > 0) caught++; else missed.push(`${e.network}.${e.crate}[${e.index}]`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  return { total, caught, missed };
}

test("mutation sweep: 1 character in each wasmHash is caught (37/37)", () => {
  const s = sweep("wasmHash", HEXA, 0);
  assert.equal(s.total, 37);
  assert.deepEqual(s.missed, []);
  assert.equal(s.caught, 37);
});

test("mutation sweep: 1 character in each contractId is caught (14/14)", () => {
  const s = sweep("contractId", B32A, 1);
  assert.equal(s.total, 14);
  assert.deepEqual(s.missed, []);
  assert.equal(s.caught, 14);
});

test("mutation sweep: version, features and status of every entry are caught by the anchor", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "registry-sweep-"));
  const f = path.join(dir, "registry.json");
  try {
    for (const e of entries(real())) {
      for (const [k, v] of [["version", e.version === null ? 0 : e.version + 1], ["features", e.features === null ? "testing" : null], ["status", e.status === "live" ? "legacy" : "live"]]) {
        const r = real();
        r.networks[e.network][e.crate][e.index][k] = v;
        fs.writeFileSync(f, JSON.stringify(r));
        assert.ok(checkRepo(ROOT, f).errors.length > 0, `${k} of ${e.network}.${e.crate}[${e.index}]`);
      }
      const r = real();
      r.networks[e.network][e.crate].splice(e.index, 1);
      if (r.networks[e.network][e.crate].length === 0) delete r.networks[e.network][e.crate];
      fs.writeFileSync(f, JSON.stringify(r));
      assert.ok(checkRepo(ROOT, f).errors.length > 0, `delete ${e.network}.${e.crate}[${e.index}]`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

const anchorDoc = (rows) => ["# x", "## Registro", "", "| rede | versão | features | status | wasm | contrato | nota |", "|---|---|---|---|---|---|---|", ...rows, "", "## depois", "| a | b |"].join("\r\n");
const aReg = () => reg({ mainnet: { token_sale: [entry({ features: "mainnet" })] }, testnet: { token_sale: [entry({ wasmHash: H("b"), contractId: SID(3) })] } });
const aRows = [`| mainnet | 1 | mainnet | live | \`${H("a")}\` | — | por oferta |`, `| testnet | 1 | testnet | live | \`${H("b")}\` | \`${SID(3)}\` |  |`];

test("anchor: parse + exact match", () => {
  const a = parseAnchor(anchorDoc(aRows));
  assert.deepEqual(a.errors, []);
  assert.equal(a.rows.length, 2);
  assert.deepEqual(checkAnchor(aReg(), "token_sale", a.rows), []);
  assert.equal(parseAnchor("# no anchor").rows, null);
});

const ANCHOR_BAD = [
  ["row missing", [aRows[0]], /no matching row/],
  ["extra row", [...aRows, `| testnet | 2 | testnet | legacy | \`${H("c")}\` | — |  |`], /anchor row not in/],
  ["other version", [aRows[0].replace("| 1 |", "| 2 |"), aRows[1]], /no matching row/],
  ["other features", [aRows[0].replace("| mainnet | live", "| — | live"), aRows[1]], /no matching row/],
  ["other status", [aRows[0].replace("| live |", "| legacy |"), aRows[1]], /no matching row/],
  ["other contract", [aRows[0], aRows[1].replace(SID(3), SID(4))], /no matching row/],
  ["duplicate row", [aRows[0], aRows[0], aRows[1]], /anchor row not in/],
];
for (const [name, rows, re] of ANCHOR_BAD) {
  test(`anchor refuses: ${name}`, () => {
    const a = parseAnchor(anchorDoc(rows));
    const errs = [...a.errors, ...checkAnchor(aReg(), "token_sale", a.rows)];
    assert.ok(errs.some((e) => re.test(e)), `expected ${re} in ${JSON.stringify(errs)}`);
  });
}

test("anchor refuses malformed rows and headers", () => {
  assert.match(parseAnchor(anchorDoc([`| mainnet | 1 | mainnet | live | \`${H("a").slice(1)}\` | — | x |`])).errors.join(), /full 64-hex/);
  assert.match(parseAnchor(anchorDoc([`| devnet | 1 | mainnet | live | \`${H("a")}\` | — | x |`])).errors.join(), /unknown network/);
  assert.match(parseAnchor(anchorDoc([`| mainnet | x | mainnet | live | \`${H("a")}\` | — | x |`])).errors.join(), /version/);
  assert.match(parseAnchor(anchorDoc([`| mainnet | 1 | mainnet | live | \`${H("a")}\` | — |`])).errors.join(), /7 cells/);
  assert.match(parseAnchor("## Registro\n| network | v |\n|---|---|").errors.join(), /anchor header/);
});

test("every crate with source here carries the anchor (a crate without it fails)", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "registry-root-"));
  try {
    fs.mkdirSync(path.join(dir, "deployments"));
    fs.cpSync(path.join(ROOT, "contracts"), path.join(dir, "contracts"), { recursive: true, filter: (p) => !p.includes(`${path.sep}target`) && !p.includes("test_snapshots") && !p.includes(`${path.sep}src`) });
    fs.copyFileSync(REG_FILE, path.join(dir, "deployments", "registry.json"));
    assert.deepEqual(checkRepo(dir).errors, []);
    fs.rmSync(path.join(dir, "contracts", "recovery_policy", "DEPLOYMENTS.md"));
    assert.ok(checkRepo(dir).errors.some((e) => /recovery_policy\/DEPLOYMENTS.md: missing the "## Registro" anchor/.test(e)));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

// radox-core@c8b29bc backend/src/config/yieldInstance.js:34-45, :58-61 and agentInstance.js:25-30, :37-39.
const CORE_C8B29BC = {
  yield_distributor: {
    accepted: {
      testnet: [
        "ebb1ce41efcfb65b0f169f78f2e0132d3512124b9182b43a78de32508f1376ad",
        "118877079046c04ab8ed491c4cd812284db00c13e67bc2fd9bac5552066500bd",
        "f6fbba0082eebe0b888e1dd44585a30abe666ceaef987b194817d3c3e4c3f251",
      ],
      mainnet: ["88509c72c0a82812b8f110b04bcd2dd3f734a43aa4b8bb18a28160ca9b9e4203"],
    },
    singleton: { testnet: ["CBIDF7ZWOR2DZLPXKW553YOJP6FUWFSDG4UBYB25T7AIH2AQRZTSLXWC"], mainnet: ["CDTXAGKIABYIKCRBD5CBEDYLVYZEEL5R5P4JT25AS4QHQFY2UGLTIMNX"] },
  },
  transfer_agent: {
    accepted: { testnet: ["3870748b709f7b8531c7ebbbc975712333f61949831350ff7192f7076fc6bff1"], mainnet: [] },
    singleton: { testnet: ["CAF4JIUZBLTQCSL2ZOX4ZEZ7FQDCGVBDVCBXRAJJRM6RW77BBX44SFC7"], mainnet: [] },
  },
};

test("status: live == accepted by the Core (lists of radox-core@c8b29bc)", () => {
  assert.deepEqual(checkCoreLists(real(), CORE_C8B29BC), []);
  const r = real();
  const e = entries(r).find((x) => x.crate === "transfer_agent" && x.status === "live");
  r.networks[e.network][e.crate][e.index].status = "legacy";
  assert.ok(checkCoreLists(r, CORE_C8B29BC).some((m) => /Core accepts 3870748b/.test(m) || /singleton/.test(m)));
  const r2 = real();
  const y = entries(r2).find((x) => x.crate === "yield_distributor" && x.status === "legacy");
  r2.networks[y.network][y.crate][y.index].status = "live";
  assert.ok(checkCoreLists(r2, CORE_C8B29BC).some((m) => /not accepted by the Core/.test(m)));
});

const CORE_DIR = process.env.RADOX_CORE_DIR ?? path.join(ROOT, "..", "radox-core");
const hasCore = fs.existsSync(path.join(CORE_DIR, "backend", "src", "config", "yieldInstance.js"));
test("the Core lists read from a radox-core checkout equal the c8b29bc fixture and the registry", { skip: hasCore ? false : `no radox-core at ${CORE_DIR} (set RADOX_CORE_DIR)` }, () => {
  const lists = readCoreLists(CORE_DIR);
  assert.deepEqual(lists, CORE_C8B29BC);
  assert.deepEqual(checkCoreLists(real(), lists), []);
});

// qa r2 (27/11/2026): one anchor per file, and no anchor-shaped rows outside it.
test("anchor refuses a second \"## Registro\" section (dup_section)", () => {
  const two = anchorDoc(aRows) + "\r\n## Registro\r\n\r\n| rede | versão | features | status | wasm | contrato | nota |\r\n|---|---|---|---|---|---|---|\r\n" + aRows[0].replace("| 1 |", "| 9 |");
  assert.match(parseAnchor(two).errors.join(), /more than one "## Registro"/);
});

test("anchor refuses an anchor-shaped row outside the section (dup_row), before or after it", () => {
  const stray = aRows[0].replace("| 1 |", "| 9 |");
  assert.match(parseAnchor(anchorDoc(aRows) + "\r\n\r\nprosa\r\n" + stray).errors.join(), /anchor-shaped table row outside/);
  assert.match(parseAnchor(anchorDoc(aRows) + "\r\n## depois\r\n" + stray).errors.join(), /anchor-shaped table row outside/);
  assert.match(parseAnchor("## mainnet\n" + stray + "\n" + anchorDoc(aRows)).errors.join(), /anchor-shaped table row outside/);
  assert.match(parseAnchor("# sem âncora\n" + stray).errors.join(), /anchor-shaped table row outside/);
  // a blank line inside the section ends the table: a row after it is stray, not a second table
  assert.match(parseAnchor(anchorDoc([aRows[0], "", aRows[1]])).errors.join(), /outside/);
});

test("real docs: no stray anchor rows, and each family member (every real row) copied outside the section is refused", () => {
  for (const c of ["token_sale", "maturity_settlement", "yield_distributor", "recovery_policy", "transfer_agent", "custody_registry"]) {
    const f = path.join(ROOT, "contracts", c, "DEPLOYMENTS.md");
    const text = fs.readFileSync(f, "utf8");
    assert.deepEqual(parseAnchor(text).errors, [], c);
    const rowLines = text.split(/\r?\n/).filter((l) => /^\| (testnet|mainnet) \|/.test(l));
    assert.ok(rowLines.length > 0, c);
    for (const l of rowLines) {
      assert.ok(parseAnchor(text + "\n\n## Outra\n\n" + l).errors.some((e) => /outside/.test(e)), `${c}: ${l.slice(0, 40)}`);
      assert.ok(parseAnchor(text + "\n\n## Registro\n\n| rede | versão | features | status | wasm | contrato | nota |\n|---|---|---|---|---|---|---|\n" + l).errors.some((e) => /more than one/.test(e)), c);
    }
  }
});

test("check-registry fails on the real repo with a duplicated section or a stray row", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "registry-root-"));
  try {
    fs.mkdirSync(path.join(dir, "deployments"));
    fs.cpSync(path.join(ROOT, "contracts"), path.join(dir, "contracts"), { recursive: true, filter: (p) => !p.includes(`${path.sep}target`) && !p.includes("test_snapshots") && !p.includes(`${path.sep}src`) });
    fs.copyFileSync(REG_FILE, path.join(dir, "deployments", "registry.json"));
    const f = path.join(dir, "contracts", "yield_distributor", "DEPLOYMENTS.md");
    const orig = fs.readFileSync(f, "utf8");
    const row = orig.split(/\r?\n/).find((l) => l.startsWith("| mainnet |"));
    fs.writeFileSync(f, orig + "\n## Notas\n\n" + row.replace("| live |", "| legacy |") + "\n");
    assert.ok(checkRepo(dir).errors.some((e) => /outside/.test(e)));
    fs.writeFileSync(f, orig + "\n## Registro\n\n| rede | versão | features | status | wasm | contrato | nota |\n|---|---|---|---|---|---|---|\n" + row + "\n");
    assert.ok(checkRepo(dir).errors.some((e) => /more than one/.test(e)));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
