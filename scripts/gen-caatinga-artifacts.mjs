#!/usr/bin/env node
// Writes caatinga.artifacts.json from deployments/registry.json (the only source). Never edit the output by hand.
// Usage: node scripts/gen-caatinga-artifacts.mjs [--check] [--root <repo>]
// --check writes nothing: exit 1 if the committed file differs from the generated one.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateSchema } from "./lib/registry.mjs";
import { ARTIFACTS_FILE, checkArtifacts, renderArtifacts } from "./lib/caatinga-artifacts.mjs";

const args = process.argv.slice(2);
const ri = args.indexOf("--root");
const root = path.resolve(ri >= 0 ? args[ri + 1] : path.join(path.dirname(fileURLToPath(import.meta.url)), ".."));
const reg = JSON.parse(fs.readFileSync(path.join(root, "deployments", "registry.json"), "utf8"));
const errs = validateSchema(reg);
if (errs.length) { console.error(`registry invalid, run scripts/check-registry.mjs:\n  - ${errs.join("\n  - ")}`); process.exit(1); }
const out = path.join(root, ARTIFACTS_FILE);

if (args.includes("--check")) {
  const diff = checkArtifacts(reg, fs.existsSync(out) ? fs.readFileSync(out, "utf8") : null);
  if (diff.length) { console.error(diff.join("\n")); process.exit(1); }
  console.log(`${ARTIFACTS_FILE}: up to date`);
} else {
  fs.writeFileSync(out, renderArtifacts(reg));
  console.log(`wrote ${ARTIFACTS_FILE}`);
}
