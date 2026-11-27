// node --test scripts/test/caatinga-artifacts.test.mjs  (needs `npm ci` for the @caatinga/core tests)
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { CRATES, cargoFeatures, entries } from "../lib/registry.mjs";
import {
  PER_OFFER, PER_OFFER_CRATES, UNKNOWN_DATE, checkArtifacts, generateArtifacts, renderArtifacts, sourceDate, sourcePath, wasmPath,
} from "../lib/caatinga-artifacts.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const REG_FILE = path.join(ROOT, "deployments", "registry.json");
const ART_FILE = path.join(ROOT, "caatinga.artifacts.json");
const real = () => JSON.parse(fs.readFileSync(REG_FILE, "utf8"));
const committed = () => fs.readFileSync(ART_FILE, "utf8");
const run = (script, args, cwd = ROOT) => spawnSync(process.execPath, [path.join(ROOT, "scripts", script), ...args], { cwd, encoding: "utf8" });
const TESTNET_BUILD = ["--no-default-features", "--features", "testnet"];

async function caatingaCore() {
  const p = path.join(ROOT, "node_modules", "@caatinga", "core", "dist", "index.js");
  assert.ok(fs.existsSync(p), "@caatinga/core is not installed: run `npm ci` in the repo root");
  return import(pathToFileURL(p).href);
}

test("the committed caatinga.artifacts.json is exactly the one generated from the registry", () => {
  assert.deepEqual(checkArtifacts(real(), committed()), []);
  const res = run("gen-caatinga-artifacts.mjs", ["--check"]);
  assert.equal(res.status, 0, res.stderr);
});

test("the generated file passes the @caatinga/core 3.9.2 schema, current version", async () => {
  const core = await caatingaCore();
  const raw = JSON.parse(committed());
  const parsed = core.CaatingaArtifactsSchema.safeParse(raw);
  assert.ok(parsed.success, JSON.stringify(parsed.error?.issues));
  assert.equal(raw.version, core.CURRENT_ARTIFACTS_SCHEMA_VERSION);
  // the extra fields (version: the Core's env mirror) are in the file even if caatinga drops them on parse
  for (const n of Object.values(raw.networks)) for (const c of Object.values(n.contracts)) assert.equal(typeof c.version, "number");
});

test("caatinga.config.ts: the 6 crates, paths of the generator, testnet flags exactly where Cargo.toml declares testnet", async () => {
  const core = await caatingaCore();
  const cfg = await core.loadConfig({ cwd: ROOT });
  assert.deepEqual(Object.keys(cfg.contracts).sort(), [...CRATES].sort());
  assert.deepEqual(Object.keys(cfg.networks).sort(), ["mainnet", "testnet"]);
  assert.equal(cfg.defaultNetwork, "testnet");
  for (const crate of CRATES) {
    const c = cfg.contracts[crate];
    assert.equal(c.path, sourcePath(crate), crate);
    assert.equal(c.wasm, wasmPath(crate), crate);
    const feats = cargoFeatures(fs.readFileSync(path.join(ROOT, "contracts", crate, "Cargo.toml"), "utf8"));
    assert.deepEqual(c.buildFeatures, feats.includes("testnet") ? TESTNET_BUILD : [], crate);
    assert.ok(!c.buildFeatures.includes("mainnet"), crate);
  }
  // independent of Cargo.toml: the list the task fixes
  const pinned = CRATES.filter((c) => cfg.contracts[c].buildFeatures.length > 0).sort();
  assert.deepEqual(pinned, ["maturity_settlement", "recovery_policy", "token_sale", "yield_distributor"]);
});

test("primary per (network, crate): the Core singletons and the env mirror of radox-core", () => {
  const n = generateArtifacts(real()).networks;
  const pick = (net, crate) => [n[net].contracts[crate].contractId, n[net].contracts[crate].wasmHash.slice(0, 8), n[net].contracts[crate].version];
  assert.deepEqual(pick("testnet", "yield_distributor"), ["CBIDF7ZWOR2DZLPXKW553YOJP6FUWFSDG4UBYB25T7AIH2AQRZTSLXWC", "11887707", 5]);
  assert.deepEqual(pick("mainnet", "yield_distributor"), ["CDTXAGKIABYIKCRBD5CBEDYLVYZEEL5R5P4JT25AS4QHQFY2UGLTIMNX", "88509c72", 4]);
  assert.deepEqual(pick("testnet", "transfer_agent"), ["CAF4JIUZBLTQCSL2ZOX4ZEZ7FQDCGVBDVCBXRAJJRM6RW77BBX44SFC7", "3870748b", 5]);
  assert.deepEqual(pick("testnet", "token_sale"), [PER_OFFER, "cda244f3", 10]);
  assert.deepEqual(pick("mainnet", "token_sale"), [PER_OFFER, "524ede1c", 7]);
  assert.deepEqual(pick("testnet", "maturity_settlement"), [PER_OFFER, "45f7b6fd", 5]);
  assert.deepEqual(pick("mainnet", "maturity_settlement"), [PER_OFFER, "0f07a598", 3]);
  assert.deepEqual(pick("testnet", "recovery_policy"), ["CAJKIIHP3R64TIIVCXZ2FDYSFUZFND3J6OYEQGL2HXDIHFURJ53O44C5", "6fed6f77", 2]);
  assert.deepEqual(pick("testnet", "custody_registry"), ["CCBJVXMJYWPHTDJHDFPRGKPGXGEP24JOPJAMUS5C3BMZJTRTWR2H4LII", "8230ed93", 1]);
  assert.deepEqual(pick("mainnet", "custody_registry"), ["CDV7YI7KYTDH4LPOLVUVRTFERJKYOWBBT6KS56ZX3V7EOGDBWS7LD4UP", "8230ed93", 1]);
  assert.deepEqual(Object.keys(n.mainnet.contracts).sort(), ["custody_registry", "maturity_settlement", "token_sale", "yield_distributor"]);
});

test("every live entry is in the artifacts exactly once (primary or otherLive), no legacy entry is (family)", () => {
  const n = generateArtifacts(real()).networks;
  const key = (net, crate, e) => `${net}|${crate}|${e.wasmHash}|${e.contractId === PER_OFFER ? null : e.contractId}|${e.version}|${e.features}`;
  const got = [];
  for (const [net, { contracts }] of Object.entries(n)) {
    for (const [crate, a] of Object.entries(contracts)) {
      got.push(key(net, crate, a));
      for (const o of a.otherLive) got.push(key(net, crate, o));
    }
  }
  const live = entries(real()).filter((e) => e.status === "live").map((e) => key(e.network, e.crate, e));
  assert.equal(live.length, 17);
  assert.deepEqual([...got].sort(), [...live].sort());
  // legacy entries never show up (each one is a different (network, crate, wasm, contract, version, features) from every live one)
  const legacy = entries(real()).filter((x) => x.status === "legacy").map((e) => key(e.network, e.crate, e));
  assert.equal(legacy.length, 20);
  assert.deepEqual(legacy.filter((k) => got.includes(k)), []);
});

test("deployedAt = first date in the source of the primary; every primary has one (no sentinel today)", () => {
  assert.equal(sourceDate("upload tx ab12 (2026-10-06); x 2026-11-01"), "2026-10-06T00:00:00.000Z");
  assert.equal(sourceDate("no date, tx 20261006aa and 12026-10-06"), UNKNOWN_DATE);
  assert.equal(sourceDate("bad 2026-13-45"), UNKNOWN_DATE);
  assert.equal(sourceDate("docs/Operations/PROVA_F3_MANIFESTO_2026-11-20.md:59 (x)"), UNKNOWN_DATE);
  assert.equal(sourceDate("(testnet, demo, deployed 2026-10-25); y"), "2026-10-25T00:00:00.000Z");
  // every real source: the picked date is one written as a date in prose, never one inside a file name
  let picked = 0;
  for (const e of entries(real())) {
    const d = sourceDate(e.source);
    if (d === UNKNOWN_DATE) { assert.ok(!/[\s(]20\d\d-\d\d-\d\d[\s),;]/.test(e.source), e.source); continue; }
    picked++;
    assert.ok(new RegExp(`[\\s(]${d.slice(0, 10)}[\\s),;]`).test(e.source), e.source);
  }
  assert.equal(picked, 15); // the 6 PROVA_F3_MANIFESTO_2026-11-20.md entries (and the undated ones) get none
  for (const net of Object.values(generateArtifacts(real()).networks)) {
    for (const [crate, a] of Object.entries(net.contracts)) assert.notEqual(a.deployedAt, UNKNOWN_DATE, crate);
  }
});

test("mutation sweep: every 1-character change at every position of the artifacts file is caught", () => {
  const text = committed();
  const reg = real();
  const missed = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\n") continue;
    const c = text[i] === "x" ? "y" : "x";
    if (checkArtifacts(reg, text.slice(0, i) + c + text.slice(i + 1)).length === 0) missed.push(i);
  }
  assert.deepEqual(missed, []);
});

test("artifacts with an entry more or less are caught (every contract and every otherLive)", () => {
  const reg = real();
  const base = JSON.parse(committed());
  const variants = [];
  for (const [net, { contracts }] of Object.entries(base.networks)) {
    for (const crate of Object.keys(contracts)) {
      const less = structuredClone(base);
      delete less.networks[net].contracts[crate];
      variants.push([`- ${net}.${crate}`, less]);
      contracts[crate].otherLive.forEach((_, i) => {
        const v = structuredClone(base);
        v.networks[net].contracts[crate].otherLive.splice(i, 1);
        variants.push([`- ${net}.${crate}.otherLive[${i}]`, v]);
      });
      const more = structuredClone(base);
      more.networks[net].contracts[crate].otherLive.push({ wasmHash: "a".repeat(64), contractId: null, version: 1, features: null });
      variants.push([`+ ${net}.${crate}.otherLive`, more]);
    }
    for (const crate of CRATES.filter((c) => !(c in contracts))) {
      const more = structuredClone(base);
      more.networks[net].contracts[crate] = structuredClone(Object.values(contracts)[0]);
      variants.push([`+ ${net}.${crate}`, more]);
    }
  }
  const noMain = structuredClone(base);
  delete noMain.networks.mainnet;
  variants.push(["- mainnet", noMain]);
  assert.equal(variants.length, 30);
  const missed = variants.filter(([, v]) => checkArtifacts(reg, JSON.stringify(v, null, 2) + "\n").length === 0).map(([n]) => n);
  assert.deepEqual(missed, []);
});

test("registry changed without regenerating: every field of every live entry, and a live entry more or less, is caught", () => {
  const text = committed();
  const missed = [];
  let n = 0;
  const flip = (s, abc) => s.slice(0, 5) + abc[(abc.indexOf(s[5]) + 1) % abc.length] + s.slice(6);
  for (const e of entries(real()).filter((x) => x.status === "live")) {
    const muts = {
      wasmHash: (x) => { x.wasmHash = flip(x.wasmHash, "0123456789abcdef"); },
      version: (x) => { x.version += 1; },
      features: (x) => { x.features = x.features === null ? "testnet" : null; },
      status: (x) => { x.status = "legacy"; },
      removed: null,
      ...(e.contractId ? { contractId: (x) => { x.contractId = flip(x.contractId, "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"); } } : {}),
    };
    for (const [name, m] of Object.entries(muts)) {
      const r = real();
      const list = r.networks[e.network][e.crate];
      if (m) m(list[e.index]); else list.splice(e.index, 1);
      if (list.length === 0) delete r.networks[e.network][e.crate];
      n++;
      if (checkArtifacts(r, text).length === 0) missed.push(`${e.network}.${e.crate}[${e.index}].${name}`);
    }
  }
  for (const e of entries(real()).filter((x) => x.status === "legacy")) {
    const r = real();
    r.networks[e.network][e.crate][e.index].status = "live";
    n++;
    if (checkArtifacts(r, text).length === 0) missed.push(`${e.network}.${e.crate}[${e.index}] legacy->live`);
  }
  assert.equal(n, 17 * 5 + 10 + 20);
  assert.deepEqual(missed, []);
});

test("generator refuses a non per-offer crate whose live entries have no contractId", () => {
  const r = { schema: 1, networks: { testnet: { recovery_policy: [{ wasmHash: "a".repeat(64), contractId: null, version: 1, features: "testnet", status: "live", source: "t" }] } } };
  assert.throws(() => generateArtifacts(r), /recovery_policy: no live entry has a contractId/);
  assert.match(checkArtifacts(r, "{}")[0], /cannot generate/);
  const e = r.networks.testnet.recovery_policy[0];
  for (const crate of PER_OFFER_CRATES) {
    assert.equal(generateArtifacts({ schema: 1, networks: { testnet: { [crate]: [e] } } }).networks.testnet.contracts[crate].contractId, PER_OFFER);
  }
});

test("CLI: check-registry fails on a drifted or missing artifacts file; gen writes the same bytes", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "artifacts-test-"));
  try {
    const f = path.join(dir, "caatinga.artifacts.json");
    fs.writeFileSync(f, committed().replace('"version": 10', '"version": 11'));
    let res = run("check-registry.mjs", ["--artifacts", f]);
    assert.equal(res.status, 1);
    assert.match(res.stderr, /FAIL \(1 problem\)[\s\S]*caatinga\.artifacts\.json:\d+: differs[\s\S]*gen-caatinga-artifacts/);
    res = run("check-registry.mjs", ["--artifacts", path.join(dir, "nope.json")]);
    assert.equal(res.status, 1);
    assert.match(res.stderr, /caatinga\.artifacts\.json: missing/);
    // gen into a scratch root: same bytes as the committed file
    fs.mkdirSync(path.join(dir, "deployments"));
    fs.copyFileSync(REG_FILE, path.join(dir, "deployments", "registry.json"));
    res = run("gen-caatinga-artifacts.mjs", ["--root", dir]);
    assert.equal(res.status, 0, res.stderr);
    // a checkout with core.autocrlf=true has CRLF on disk; the blob (and the generator) is LF
    const lf = committed().replace(/\r\n/g, "\n");
    assert.equal(fs.readFileSync(f, "utf8"), lf);
    assert.equal(renderArtifacts(real()), lf);
    assert.deepEqual(checkArtifacts(real(), lf.replace(/\n/g, "\r\n")), []);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
