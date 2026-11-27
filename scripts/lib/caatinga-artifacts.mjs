// caatinga.artifacts.json is GENERATED from deployments/registry.json (never edited by hand).
// Format: @caatinga/core 3.9.2 artifacts schema v2. No dependencies; plain node (>= 18).
import { entries } from "./registry.mjs";

export const ARTIFACTS_FILE = "caatinga.artifacts.json";
export const PROJECT = "radox";
export const ARTIFACTS_SCHEMA_VERSION = 2;
// Deployed per offer at runtime by the Core: no single contract ID (radox-core convention).
export const PER_OFFER = "PER-OFFER";
export const PER_OFFER_CRATES = ["token_sale", "maturity_settlement"];
// The schema requires a datetime; registry entries only cite dates in `source`.
export const UNKNOWN_DATE = "1970-01-01T00:00:00.000Z";

export const sourcePath = (crate) => `./contracts/${crate}`;
export const wasmPath = (crate) => `./contracts/${crate}/target/wasm32v1-none/release/${crate}.wasm`;

/** First standalone YYYY-MM-DD in `source` (after a space or "(", not inside a file name), as a datetime; UNKNOWN_DATE if none. */
export function sourceDate(source) {
  const m = /(?<=[\s(])(20\d\d-[01]\d-[0-3]\d)(?=[\s),;]|$)/.exec(source ?? "");
  return m && !Number.isNaN(Date.parse(m[1])) ? `${m[1]}T00:00:00.000Z` : UNKNOWN_DATE;
}

/**
 * caatinga keeps one artifact per (network, contract). Selection among the `live` entries of a crate:
 * the first one (registry order) with a contractId, else the first one. The other live entries go to `otherLive`.
 */
export function pickPrimary(live) {
  return live.find((e) => e.contractId !== null) ?? live[0];
}

/** The artifacts object for a registry (assumed schema-valid). Throws on a crate that cannot be represented. */
export function generateArtifacts(reg) {
  const networks = {};
  for (const network of Object.keys(reg.networks)) {
    const byCrate = new Map();
    for (const e of entries(reg)) {
      if (e.network !== network || e.status !== "live") continue;
      if (!byCrate.has(e.crate)) byCrate.set(e.crate, []);
      byCrate.get(e.crate).push(e);
    }
    const contracts = {};
    for (const [crate, live] of byCrate) {
      const p = pickPrimary(live);
      if (p.contractId === null && !PER_OFFER_CRATES.includes(crate)) {
        throw new Error(`networks.${network}.${crate}: no live entry has a contractId, and the crate is not per-offer`);
      }
      contracts[crate] = {
        contractId: p.contractId ?? PER_OFFER,
        wasmHash: p.wasmHash,
        deployedAt: sourceDate(p.source),
        sourcePath: sourcePath(crate),
        wasmPath: wasmPath(crate),
        dependencies: [],
        resolvedDeployArgs: {},
        // Extra fields (ignored by caatinga): contract version (the Core's env mirror), build feature, the other live entries as in the registry (contractId may be null).
        version: p.version,
        features: p.features,
        otherLive: live
          .filter((e) => e !== p)
          .map((e) => ({ wasmHash: e.wasmHash, contractId: e.contractId, version: e.version, features: e.features })),
      };
    }
    if (Object.keys(contracts).length) networks[network] = { contracts, dependencyGraph: {} };
  }
  return { version: ARTIFACTS_SCHEMA_VERSION, project: PROJECT, networks };
}

/** Exact text of the generated file (2-space JSON + newline). */
export function renderArtifacts(reg) {
  return JSON.stringify(generateArtifacts(reg), null, 2) + "\n";
}

/** Errors if `text` (the committed file, or null if missing) is not byte-equal to the generated one. */
export function checkArtifacts(reg, text, name = ARTIFACTS_FILE) {
  let want;
  try {
    want = renderArtifacts(reg);
  } catch (err) {
    return [`${name}: cannot generate from deployments/registry.json (${err.message})`];
  }
  const fix = "run: node scripts/gen-caatinga-artifacts.mjs";
  if (text === null) return [`${name}: missing (${fix})`];
  const got = text.replace(/\r\n/g, "\n");
  if (got === want) return [];
  const a = got.split("\n"), b = want.split("\n");
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  return [`${name}:${i + 1}: differs from the file generated from deployments/registry.json (${fix}); got ${JSON.stringify(a[i] ?? "<eof>")}, want ${JSON.stringify(b[i] ?? "<eof>")}`];
}
