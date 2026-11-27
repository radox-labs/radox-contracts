#!/usr/bin/env node
// Read-only view of deployments/registry.json: what is published where.
// Usage: node scripts/caatinga.mjs query <crate> [--network testnet|mainnet] [--json]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { CRATES, NETWORKS, query, validateSchema } from "./lib/registry.mjs";

const usage = `usage: node scripts/caatinga.mjs query <crate> [--network ${NETWORKS.join("|")}] [--json]\ncrates: ${CRATES.join(", ")}`;
const args = process.argv.slice(2);
const [cmd, crate] = args;
const ni = args.indexOf("--network");
const network = ni >= 0 ? args[ni + 1] : null;
if (cmd !== "query" || !crate || crate.startsWith("--")) { console.error(usage); process.exit(2); }
if (!CRATES.includes(crate)) { console.error(`unknown crate "${crate}"\n${usage}`); process.exit(2); }
if (ni >= 0 && !NETWORKS.includes(network)) { console.error(`unknown network "${network}"\n${usage}`); process.exit(2); }

const file = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "deployments", "registry.json");
const reg = JSON.parse(fs.readFileSync(file, "utf8"));
const errs = validateSchema(reg);
if (errs.length) { console.error(`registry invalid, run scripts/check-registry.mjs:\n  - ${errs.join("\n  - ")}`); process.exit(1); }

const rows = query(reg, crate, network);
if (args.includes("--json")) {
  console.log(JSON.stringify(rows.map(({ index, ...e }) => e), null, 2));
} else if (rows.length === 0) {
  console.log(`${crate}: nothing registered${network ? ` on ${network}` : ""}`);
} else {
  const show = (v) => (v === null ? "-" : String(v));
  for (const e of rows) {
    console.log(`${e.network.padEnd(7)} ${e.status.padEnd(6)} v${show(e.version).padEnd(3)} ${show(e.features).padEnd(7)} ${e.wasmHash} ${show(e.contractId)}`);
  }
}
