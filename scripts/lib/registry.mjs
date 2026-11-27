// deployments/registry.json: schema check, DEPLOYMENTS.md cross-check, queries.
// No dependencies; plain node (>= 18).
import fs from "node:fs";
import path from "node:path";

export const NETWORKS = ["testnet", "mainnet"];
export const CRATES = [
  "token_sale",
  "maturity_settlement",
  "transfer_agent",
  "yield_distributor",
  "recovery_policy",
  "custody_registry",
];
export const FEATURES = ["testnet", "mainnet", "testing"];
export const STATUSES = ["live", "legacy"];
const ENTRY_KEYS = ["wasmHash", "contractId", "version", "features", "status", "source"];

const HEX64 = /^[0-9a-f]{64}$/;
const CONTRACT_ID = /^C[A-Z2-7]{55}$/;
// Tokens inside prose: bounded so a longer run or a G... address never matches.
const HEX64_IN_TEXT = /(?<![0-9a-fA-F])[0-9a-f]{64}(?![0-9a-fA-F])/g;
const CID_IN_TEXT = /(?<![A-Z2-7])C[A-Z2-7]{55}(?![A-Z2-7])/g;
// Abbreviated hash as the docs write it: `8230ed93…1342` (or with "...").
const SHORT_HEX_IN_TEXT = /(?<![0-9a-fA-F])([0-9a-f]{8})(?:…|\.\.\.)([0-9a-f]{4})(?![0-9a-fA-F])/g;
const NETWORK_HEADING = /^#{2,}\s+(testnet|mainnet)\b/i;
const ANY_HEADING = /^#{1,6}\s/;

const isObj = (v) => v !== null && typeof v === "object" && !Array.isArray(v);

/** Flat list of entries with their network and crate. */
export function entries(reg) {
  const out = [];
  for (const [network, crates] of Object.entries(reg?.networks ?? {})) {
    if (!isObj(crates)) continue;
    for (const [crate, list] of Object.entries(crates)) {
      if (!Array.isArray(list)) continue;
      list.forEach((e, i) => out.push({ network, crate, index: i, ...e }));
    }
  }
  return out;
}

/** Schema and invariants. Returns a list of error strings (empty = ok). */
export function validateSchema(reg) {
  const errs = [];
  if (!isObj(reg)) return ["registry: top level must be an object"];
  for (const k of Object.keys(reg)) if (!["schema", "networks"].includes(k)) errs.push(`registry: unknown top-level key "${k}"`);
  if (reg.schema !== 1) errs.push(`registry: "schema" must be 1 (got ${JSON.stringify(reg.schema)})`);
  if (!isObj(reg.networks)) return [...errs, 'registry: "networks" must be an object'];

  for (const [network, crates] of Object.entries(reg.networks)) {
    if (!NETWORKS.includes(network)) errs.push(`networks.${network}: unknown network (allowed: ${NETWORKS.join(", ")})`);
    if (!isObj(crates)) { errs.push(`networks.${network}: must be an object of crates`); continue; }
    for (const [crate, list] of Object.entries(crates)) {
      const at = `networks.${network}.${crate}`;
      if (!CRATES.includes(crate)) errs.push(`${at}: unknown crate (allowed: ${CRATES.join(", ")})`);
      if (!Array.isArray(list) || list.length === 0) { errs.push(`${at}: must be a non-empty array`); continue; }
      list.forEach((e, i) => {
        const p = `${at}[${i}]`;
        if (!isObj(e)) { errs.push(`${p}: must be an object`); return; }
        for (const k of Object.keys(e)) if (!ENTRY_KEYS.includes(k)) errs.push(`${p}: unknown key "${k}"`);
        for (const k of ENTRY_KEYS) if (!(k in e)) errs.push(`${p}: missing "${k}" (unknown values are null, not absent)`);
        if (typeof e.wasmHash !== "string" || !HEX64.test(e.wasmHash)) errs.push(`${p}.wasmHash: must be 64 lowercase hex chars (got ${JSON.stringify(e.wasmHash)})`);
        if (e.contractId !== null && (typeof e.contractId !== "string" || !CONTRACT_ID.test(e.contractId))) errs.push(`${p}.contractId: must be null or a C... strkey of 56 chars (got ${JSON.stringify(e.contractId)})`);
        if (e.version !== null && !(Number.isInteger(e.version) && e.version >= 0)) errs.push(`${p}.version: must be null or a non-negative integer (got ${JSON.stringify(e.version)})`);
        if (e.features !== null && !FEATURES.includes(e.features)) errs.push(`${p}.features: must be null or one of ${FEATURES.join("|")} (got ${JSON.stringify(e.features)})`);
        if (!STATUSES.includes(e.status)) errs.push(`${p}.status: must be one of ${STATUSES.join("|")} (got ${JSON.stringify(e.status)})`);
        if (typeof e.source !== "string" || e.source.trim() === "") errs.push(`${p}.source: must be a non-empty string`);
        // A wasm without the canonical mainnet USDC gate must never be listed as running on mainnet.
        if (network === "mainnet" && (e.features === "testnet" || e.features === "testing")) errs.push(`${p}: features "${e.features}" on mainnet`);
      });
    }
  }
  if (errs.length) return errs;

  // Cross-entry invariants.
  const all = entries(reg);
  const seenId = new Map();
  const seenCode = new Map();
  const byHash = new Map();
  for (const e of all) {
    const p = `networks.${e.network}.${e.crate}[${e.index}]`;
    if (e.contractId) {
      const k = `${e.network}:${e.contractId}`;
      if (seenId.has(k)) errs.push(`${p}: contractId ${e.contractId} already listed at ${seenId.get(k)}`);
      else seenId.set(k, p);
    } else {
      const k = `${e.network}:${e.crate}:${e.wasmHash}`;
      if (seenCode.has(k)) errs.push(`${p}: wasm ${e.wasmHash} without contractId already listed at ${seenCode.get(k)}`);
      else seenCode.set(k, p);
    }
    // Same bytes = same crate, version and build features, on any network.
    const prev = byHash.get(e.wasmHash);
    if (!prev) { byHash.set(e.wasmHash, { ...e, p }); continue; }
    if (prev.crate !== e.crate) errs.push(`${p}: wasm ${e.wasmHash} is also listed under crate ${prev.crate} (${prev.p})`);
    for (const k of ["version", "features"]) {
      if (prev[k] !== null && e[k] !== null && prev[k] !== e[k]) errs.push(`${p}.${k}: ${JSON.stringify(e[k])} disagrees with ${prev.p} (${JSON.stringify(prev[k])}) for the same wasm ${e.wasmHash}`);
    }
  }
  return errs;
}

/** Tokens a DEPLOYMENTS.md cites, each with the network section it sits in (null = none). */
export function parseDoc(text) {
  const found = { hashes: [], shortHashes: [], contractIds: [] };
  let network = null;
  text.split(/\r?\n/).forEach((line, i) => {
    const h = line.match(NETWORK_HEADING);
    if (h) network = h[1].toLowerCase();
    else if (ANY_HEADING.test(line)) network = null;
    const ln = i + 1;
    for (const m of line.matchAll(HEX64_IN_TEXT)) found.hashes.push({ value: m[0], network, line: ln });
    for (const m of line.matchAll(SHORT_HEX_IN_TEXT)) found.shortHashes.push({ prefix: m[1], suffix: m[2], network, line: ln });
    for (const m of line.matchAll(CID_IN_TEXT)) found.contractIds.push({ value: m[0], network, line: ln });
  });
  return found;
}

/**
 * Cross-check one crate's DEPLOYMENTS.md against the registry.
 * Doc -> registry: every full hash is a wasmHash of the crate or a tx/hash quoted in one of its
 * `source` fields; every abbreviated hash expands to one wasmHash; every contract ID is registered.
 * Under a "## testnet"/"## mainnet" heading the match must be on that network.
 * Registry -> doc: every live entry of the crate has its wasmHash and its contractId in the doc.
 */
export function checkDoc(reg, crate, text, docName = `contracts/${crate}/DEPLOYMENTS.md`) {
  const errs = [];
  const mine = entries(reg).filter((e) => e.crate === crate);
  const onNet = (e, network) => network === null || e.network === network;
  const where = (t) => `${docName}:${t.line}${t.network ? ` (${t.network} section)` : ""}`;
  const doc = parseDoc(text);

  for (const t of doc.hashes) {
    const asWasm = mine.filter((e) => e.wasmHash === t.value);
    const asSource = mine.filter((e) => e.source.includes(t.value));
    if (asWasm.length === 0 && asSource.length === 0) errs.push(`${where(t)}: hash ${t.value} is not in deployments/registry.json for ${crate} (neither a wasmHash nor in a source)`);
    else if (![...asWasm, ...asSource].some((e) => onNet(e, t.network))) errs.push(`${where(t)}: hash ${t.value} is registered for ${crate} only on ${[...new Set([...asWasm, ...asSource].map((e) => e.network))].join(", ")}`);
  }
  for (const t of doc.shortHashes) {
    const hits = new Set(mine.filter((e) => onNet(e, t.network) && e.wasmHash.startsWith(t.prefix) && e.wasmHash.endsWith(t.suffix)).map((e) => e.wasmHash));
    if (hits.size !== 1) errs.push(`${where(t)}: abbreviated hash ${t.prefix}…${t.suffix} matches ${hits.size} wasmHash of ${crate} in the registry (expected 1)`);
  }
  for (const t of doc.contractIds) {
    const hits = mine.filter((e) => e.contractId === t.value);
    if (hits.length === 0) errs.push(`${where(t)}: contract ${t.value} is not in deployments/registry.json for ${crate}`);
    else if (!hits.some((e) => onNet(e, t.network))) errs.push(`${where(t)}: contract ${t.value} is registered for ${crate} only on ${hits.map((e) => e.network).join(", ")}`);
  }
  for (const e of mine.filter((x) => x.status === "live")) {
    const p = `networks.${e.network}.${crate}[${e.index}]`;
    if (!doc.hashes.some((t) => t.value === e.wasmHash)) errs.push(`${p}: live wasm ${e.wasmHash} is not cited in full in ${docName}`);
    if (e.contractId && !doc.contractIds.some((t) => t.value === e.contractId && onNet(e, t.network))) errs.push(`${p}: live contract ${e.contractId} (${e.network}) is not in ${docName}`);
  }
  return errs;
}

/** Feature names declared in a Cargo.toml [features] table (none = empty list). */
export function cargoFeatures(toml) {
  const out = [];
  let inFeatures = false;
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.replace(/#.*/, "").trim();
    if (/^\[.*\]$/.test(line)) { inFeatures = line === "[features]"; continue; }
    const m = inFeatures && line.match(/^([A-Za-z0-9_-]+)\s*=/);
    if (m && m[1] !== "default") out.push(m[1]);
  }
  return out;
}

/** Every non-null `features` must be a feature the crate declares (custody_registry declares none). */
export function checkFeatures(reg, featuresByCrate) {
  const errs = [];
  for (const e of entries(reg)) {
    const declared = featuresByCrate[e.crate];
    if (e.features === null || declared === undefined) continue;
    if (!declared.includes(e.features)) errs.push(`networks.${e.network}.${e.crate}[${e.index}].features: "${e.features}" is not declared in contracts/${e.crate}/Cargo.toml (declared: ${declared.join(", ") || "none"})`);
  }
  return errs;
}

/** contracts/<crate>/DEPLOYMENTS.md files present under root. */
export function findDocs(root) {
  const out = [];
  const dir = path.join(root, "contracts");
  if (!fs.existsSync(dir)) return out;
  for (const name of fs.readdirSync(dir).sort()) {
    const f = path.join(dir, name, "DEPLOYMENTS.md");
    if (fs.existsSync(f)) out.push({ crate: name, file: f, name: `contracts/${name}/DEPLOYMENTS.md` });
  }
  return out;
}

/** Full check of a repo checkout. Returns { errors, summary }. */
export function checkRepo(root, registryPath = path.join(root, "deployments", "registry.json")) {
  let reg;
  try {
    reg = JSON.parse(fs.readFileSync(registryPath, "utf8"));
  } catch (err) {
    return { errors: [`${registryPath}: cannot read/parse (${err.message})`], summary: null };
  }
  const errors = validateSchema(reg);
  if (errors.length) return { errors, summary: null };
  const featuresByCrate = {};
  for (const crate of CRATES) {
    const f = path.join(root, "contracts", crate, "Cargo.toml");
    if (fs.existsSync(f)) featuresByCrate[crate] = cargoFeatures(fs.readFileSync(f, "utf8"));
  }
  errors.push(...checkFeatures(reg, featuresByCrate));
  const docs = findDocs(root);
  for (const d of docs) {
    if (!CRATES.includes(d.crate)) { errors.push(`${d.name}: crate "${d.crate}" is not a registry crate`); continue; }
    errors.push(...checkDoc(reg, d.crate, fs.readFileSync(d.file, "utf8"), d.name));
  }
  const counts = Object.fromEntries(NETWORKS.map((n) => [n, entries(reg).filter((e) => e.network === n).length]));
  return { errors, summary: { counts, docs: docs.map((d) => d.name) } };
}

/** Entries for one crate, optionally one network ("what is published where"). */
export function query(reg, crate, network = null) {
  return entries(reg).filter((e) => e.crate === crate && (network === null || e.network === network));
}
