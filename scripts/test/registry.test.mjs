// node --test scripts/test/registry.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { cargoFeatures, checkDoc, checkFeatures, checkRepo, entries, parseDoc, query, validateSchema } from "../lib/registry.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const REG_FILE = path.join(ROOT, "deployments", "registry.json");
const CUSTODY_DOC = path.join(ROOT, "contracts", "custody_registry", "DEPLOYMENTS.md");
const real = () => JSON.parse(fs.readFileSync(REG_FILE, "utf8"));
const H = (c) => c.repeat(64);
const ID = (c) => "C" + c.repeat(55);
const entry = (o = {}) => ({ wasmHash: H("a"), contractId: null, version: 1, features: "testnet", status: "live", source: "test", ...o });
const reg = (networks) => ({ schema: 1, networks });

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
  ["duplicate contractId", (r) => { r.networks.testnet.custody_registry.push(entry({ wasmHash: H("c"), contractId: ID("A"), features: null })); }, /already listed/],
  ["duplicate code-only entry", (r) => { r.networks.testnet.token_sale.push(entry()); }, /without contractId already listed/],
  ["same wasm, other version", (r) => { Object.assign(r.networks.mainnet.token_sale[0], { wasmHash: H("a"), features: null, version: 2 }); }, /disagrees/],
  ["same wasm, other features", (r) => { r.networks.mainnet.token_sale[0].wasmHash = H("a"); }, /features: "mainnet" disagrees/],
  ["same wasm, other crate", (r) => { r.networks.testnet.yield_distributor = [entry({ wasmHash: H("a"), features: "testnet" })]; }, /also listed under crate/],
];

const base = () => reg({
  testnet: {
    token_sale: [entry()],
    custody_registry: [entry({ wasmHash: H("b"), contractId: ID("A"), features: null })],
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
