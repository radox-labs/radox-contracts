#!/usr/bin/env node
// Validates deployments/registry.json and cross-checks it with every contracts/<crate>/DEPLOYMENTS.md.
// Usage: node scripts/check-registry.mjs [--root <repo>] [--registry <file>]
import path from "node:path";
import { fileURLToPath } from "node:url";
import { checkRepo } from "./lib/registry.mjs";

const args = process.argv.slice(2);
const opt = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const root = path.resolve(opt("--root") ?? path.join(path.dirname(fileURLToPath(import.meta.url)), ".."));
const registry = opt("--registry") ? path.resolve(opt("--registry")) : undefined;

const { errors, summary } = checkRepo(root, registry);
if (errors.length) {
  console.error(`check-registry: FAIL (${errors.length} problem${errors.length === 1 ? "" : "s"})`);
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
const nets = Object.entries(summary.counts).map(([n, c]) => `${n} ${c}`).join(", ");
console.log(`check-registry: ok (entries: ${nets}; docs checked: ${summary.docs.join(", ") || "none"})`);
