#!/usr/bin/env node
// Validates deployments/registry.json and cross-checks it with every contracts/<crate>/DEPLOYMENTS.md.
// Usage: node scripts/check-registry.mjs [--root <repo>] [--registry <file>] [--core <radox-core checkout>]
// --core also checks that `live` equals what the Core accepts (YIELD/AGENT_WASM_ACCEPTED and the shared singletons).
import path from "node:path";
import { fileURLToPath } from "node:url";
import { checkCoreLists, checkRepo, readCoreLists } from "./lib/registry.mjs";
import fs from "node:fs";

const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const root = path.resolve(opt("--root") ?? path.join(path.dirname(fileURLToPath(import.meta.url)), ".."));
const registry = opt("--registry") ? path.resolve(opt("--registry")) : undefined;

const { errors, summary } = checkRepo(root, registry);
const core = opt("--core");
if (core && summary) {
  try {
    const reg = JSON.parse(fs.readFileSync(registry ?? path.join(root, "deployments", "registry.json"), "utf8"));
    errors.push(...checkCoreLists(reg, readCoreLists(path.resolve(core))));
    summary.docs.push(`core lists (${core})`);
  } catch (err) {
    errors.push(`--core ${core}: ${err.message}`);
  }
}
if (errors.length) {
  console.error(`check-registry: FAIL (${errors.length} problem${errors.length === 1 ? "" : "s"})`);
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
const nets = Object.entries(summary.counts).map(([n, c]) => `${n} ${c}`).join(", ");
console.log(`check-registry: ok (entries: ${nets}; docs checked: ${summary.docs.join(", ") || "none"})`);
