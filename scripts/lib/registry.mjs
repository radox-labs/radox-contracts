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

const B32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/** True if `id` is a Stellar contract strkey: base32, version byte 'C' (2 << 3), 32-byte payload, CRC16-XModem. */
export function isContractStrkey(id) {
  if (typeof id !== "string" || !CONTRACT_ID.test(id)) return false;
  let bits = 0, acc = 0;
  const bytes = [];
  for (const ch of id) {
    acc = (acc << 5) | B32.indexOf(ch);
    bits += 5;
    if (bits >= 8) { bytes.push((acc >> (bits - 8)) & 0xff); bits -= 8; }
  }
  if (bytes.length !== 35 || bytes[0] !== 2 << 3) return false;
  if ((acc & ((1 << bits) - 1)) !== 0) return false; // non-canonical padding bits
  let crc = 0;
  for (const b of bytes.slice(0, 33)) {
    crc ^= b << 8;
    for (let i = 0; i < 8; i++) crc = crc & 0x8000 ? ((crc << 1) ^ 0x1021) & 0xffff : (crc << 1) & 0xffff;
  }
  return bytes[33] === (crc & 0xff) && bytes[34] === crc >> 8;
}

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
        else if (e.contractId !== null && !isContractStrkey(e.contractId)) errs.push(`${p}.contractId: ${e.contractId} fails the strkey checksum`);
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
  const idNet = new Map();
  const seenCode = new Map();
  const byHash = new Map();
  for (const e of all) {
    const p = `networks.${e.network}.${e.crate}[${e.index}]`;
    if (e.contractId) {
      const other = idNet.get(e.contractId);
      if (other && other.network !== e.network) errs.push(`${p}: contractId ${e.contractId} is also listed on ${other.network} (${other.p}); an ID belongs to one network`);
      else if (!other) idNet.set(e.contractId, { network: e.network, p });
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

const ANCHOR_HEADING = /^##\s+Registro\s*$/;
const ANCHOR_COLS = ["rede", "versão", "features", "status", "wasm", "contrato", "nota"];
const NULL_CELL = new Set(["—", "-", ""]);

/**
 * The "## Registro" table of a contracts/<crate>/DEPLOYMENTS.md: one row per registry entry.
 * Returns { rows, errors }; rows = null when the doc has no such section.
 */
export function parseAnchor(text, docName = "DEPLOYMENTS.md") {
  const lines = text.split(/\r?\n/);
  const start = lines.findIndex((l) => ANCHOR_HEADING.test(l));
  if (start < 0) return { rows: null, errors: [] };
  const errors = [];
  const rows = [];
  let header = false;
  for (let i = start + 1; i < lines.length; i++) {
    const l = lines[i].trim();
    if (/^#{1,6}\s/.test(l)) break;
    if (!l.startsWith("|")) { if (header && l !== "") break; continue; }
    const cells = l.replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim().replace(/^`(.*)`$/, "$1"));
    const at = `${docName}:${i + 1}`;
    if (!header) {
      if (cells.join("|") !== ANCHOR_COLS.join("|")) { errors.push(`${at}: anchor header must be | ${ANCHOR_COLS.join(" | ")} |`); return { rows, errors }; }
      header = true;
      continue;
    }
    if (cells.every((c) => /^:?-{3,}:?$/.test(c))) continue;
    if (cells.length !== ANCHOR_COLS.length) { errors.push(`${at}: anchor row needs ${ANCHOR_COLS.length} cells, got ${cells.length}`); continue; }
    const [network, version, features, status, wasmHash, contractId] = cells;
    const row = {
      line: i + 1,
      network,
      version: NULL_CELL.has(version) ? null : /^\d+$/.test(version) ? Number(version) : version,
      features: NULL_CELL.has(features) ? null : features,
      status,
      wasmHash,
      contractId: NULL_CELL.has(contractId) ? null : contractId,
    };
    if (!NETWORKS.includes(row.network)) errors.push(`${at}: unknown network "${network}"`);
    if (row.version !== null && typeof row.version !== "number") errors.push(`${at}: version must be an integer or —`);
    if (row.features !== null && !FEATURES.includes(row.features)) errors.push(`${at}: features must be ${FEATURES.join("|")} or —`);
    if (!STATUSES.includes(row.status)) errors.push(`${at}: status must be ${STATUSES.join("|")}`);
    if (!HEX64.test(row.wasmHash)) errors.push(`${at}: wasm must be the full 64-hex hash`);
    if (row.contractId !== null && !CONTRACT_ID.test(row.contractId)) errors.push(`${at}: contrato must be a C... strkey or —`);
    rows.push(row);
  }
  if (!header) errors.push(`${docName}: "## Registro" has no table`);
  return { rows, errors };
}

const anchorKey = (x) => [x.network, x.wasmHash, x.contractId ?? "—", x.version ?? "—", x.features ?? "—", x.status].join(" ");

/** Registry entries of a crate and the anchor rows must be the same multiset of (network, wasm, contract, version, features, status). */
export function checkAnchor(reg, crate, rows, docName = `contracts/${crate}/DEPLOYMENTS.md`) {
  const errs = [];
  const left = new Map();
  for (const r of rows) left.set(anchorKey(r), [...(left.get(anchorKey(r)) ?? []), r]);
  for (const e of entries(reg).filter((x) => x.crate === crate)) {
    const k = anchorKey(e);
    const hit = left.get(k);
    if (hit && hit.length) { hit.shift(); continue; }
    errs.push(`networks.${e.network}.${crate}[${e.index}]: no matching row in ${docName} "## Registro" (${k})`);
  }
  for (const list of left.values()) for (const r of list) errs.push(`${docName}:${r.line}: anchor row not in deployments/registry.json (${anchorKey(r)})`);
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
    const text = fs.readFileSync(d.file, "utf8");
    errors.push(...checkDoc(reg, d.crate, text, d.name));
    const a = parseAnchor(text, d.name);
    errors.push(...a.errors);
    if (a.rows) errors.push(...checkAnchor(reg, d.crate, a.rows, d.name));
  }
  // Every crate with source here, and every crate in the registry, must carry the anchor.
  const anchored = new Set(docs.filter((d) => parseAnchor(fs.readFileSync(d.file, "utf8")).rows).map((d) => d.crate));
  const need = new Set([...Object.keys(featuresByCrate), ...entries(reg).map((e) => e.crate)]);
  for (const c of [...need].sort()) if (!anchored.has(c)) errors.push(`contracts/${c}/DEPLOYMENTS.md: missing the "## Registro" anchor table`);
  const counts = Object.fromEntries(NETWORKS.map((n) => [n, entries(reg).filter((e) => e.network === n).length]));
  return { errors, summary: { counts, docs: docs.map((d) => d.name) } };
}

/** Entries for one crate, optionally one network ("what is published where"). */
export function query(reg, crate, network = null) {
  return entries(reg).filter((e) => e.crate === crate && (network === null || e.network === network));
}

/**
 * Core side of `status` (decision 27/11/2026): for the crates the Core pins by list, `live` = accepted by the Core today.
 * Reads backend/src/config/{yield,agent}Instance.js of a radox-core checkout. Core network "public" = "mainnet".
 */
export const CORE_LISTS = {
  yield_distributor: { file: "backend/src/config/yieldInstance.js", accepted: "YIELD_WASM_ACCEPTED", singleton: "YIELD_SHARED_SINGLETON" },
  transfer_agent: { file: "backend/src/config/agentInstance.js", accepted: "AGENT_WASM_ACCEPTED", singleton: "AGENT_SHARED_SINGLETON" },
};

function block(src, name) {
  const i = src.indexOf(`export const ${name} = Object.freeze({`);
  if (i < 0) return null;
  const j = src.indexOf("\n});", i);
  return j < 0 ? null : src.slice(i, j);
}

function byNetwork(text, re) {
  const out = { testnet: [], mainnet: [] };
  let net = null;
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^\s*(testnet|public)\s*:/);
    if (m) net = m[1] === "public" ? "mainnet" : "testnet";
    if (!net) continue;
    for (const x of line.matchAll(re)) out[net].push(x[1]);
  }
  return out;
}

/** { crate: { accepted: {testnet: [hash], mainnet: [hash]}, singleton: {testnet: [id], mainnet: [id]} } } from Core sources. */
export function readCoreLists(coreDir) {
  const out = {};
  for (const [crate, l] of Object.entries(CORE_LISTS)) {
    const src = fs.readFileSync(path.join(coreDir, l.file), "utf8");
    const acc = block(src, l.accepted);
    const one = block(src, l.singleton);
    if (!acc || !one) throw new Error(`${l.file}: ${l.accepted}/${l.singleton} not found`);
    out[crate] = {
      accepted: byNetwork(acc, /hash:\s*'([0-9a-f]{64})'/g),
      singleton: byNetwork(one, /'(C[A-Z2-7]{55})'/g),
    };
  }
  return out;
}

/** live entries == accepted hashes, and each Core singleton is a live entry of that crate and network. */
export function checkCoreLists(reg, lists) {
  const errs = [];
  for (const [crate, l] of Object.entries(lists)) {
    for (const network of NETWORKS) {
      const mine = entries(reg).filter((e) => e.crate === crate && e.network === network);
      const live = new Set(mine.filter((e) => e.status === "live").map((e) => e.wasmHash));
      const acc = new Set(l.accepted[network]);
      for (const h of acc) if (!live.has(h)) errs.push(`${network}.${crate}: Core accepts ${h} but the registry has no live entry for it`);
      for (const h of live) if (!acc.has(h)) errs.push(`${network}.${crate}: live wasm ${h} is not accepted by the Core`);
      for (const id of l.singleton[network]) {
        if (!mine.some((e) => e.contractId === id && e.status === "live")) errs.push(`${network}.${crate}: Core singleton ${id} is not a live entry`);
      }
    }
  }
  return errs;
}
