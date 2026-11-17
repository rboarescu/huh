// Drafts a catalog entry from a Fig completion spec (npm package @withfig/autocomplete).
// The draft lists the command's flags and arguments with Fig's man-page style
// descriptions. It is a starting point only: pick the few flags worth having and
// rewrite every `question`, `what` and `description` in plain words by hand.
//
// The spec files are downloaded code, so run this with Node's permission model:
//
//   mkdir /tmp/fig && cd /tmp/fig && npm pack @withfig/autocomplete && tar -xzf *.tgz
//   node --permission --allow-fs-read=/tmp/fig --allow-fs-read=$PWD/tools tools/seed.mjs /tmp/fig/package/build/chmod.js
//
// Note: Fig's specs for core tools often describe the macOS/BSD variants.

import { pathToFileURL } from "node:url";

const file = process.argv[2];
if (!file) {
  console.error("usage: seed.mjs <path to Fig spec .js>");
  process.exit(2);
}
const spec = (await import(pathToFileURL(file).href)).default;
if (!spec || typeof spec !== "object" || typeof spec.name !== "string") {
  console.error("this spec is generated at runtime and cannot be drafted from");
  process.exit(1);
}

const list = (x) => (x == null ? [] : Array.isArray(x) ? x : [x]);
const str = (s) => JSON.stringify(String(s ?? "").replace(/\s+/g, " ").trim());
const id = (s) => String(s).replace(/^-+/, "").replace(/[^A-Za-z0-9]+/g, "_").toLowerCase() || "x";

const flags = list(spec.options).filter((o) => list(o.args).length === 0);
const valued = list(spec.options).filter((o) => list(o.args).length > 0);
const args = list(spec.args);
const longest = (o) => list(o.name).sort((a, b) => b.length - a.length)[0];
const shortest = (o) => list(o.name).sort((a, b) => a.length - b.length)[0];

// Fig lists -a and -A separately; keep ids unique after lowercasing.
const seen = new Map();
const unique = (base) => {
  const n = (seen.get(base) ?? 0) + 1;
  seen.set(base, n);
  return n === 1 ? base : `${base}_${n}`;
};
const flagIds = flags.map((o) => unique(id(longest(o))));
const argIds = args.map((a, i) => unique(id(a.name ?? `arg${i + 1}`)));

const placeholders = [...flagIds, ...argIds].map((x) => `{${x}}`);
const out = [];
out.push(`# DRAFT from Fig spec "${spec.name}". Rewrite all wording by hand before moving into catalog/.`);
out.push(`name = ${str(spec.name)}`);
out.push(`description = ${str("TODO: " + (spec.description ?? ""))}`);
out.push(`template = ${str([spec.name, ...placeholders].join(" "))}`);
flags.forEach((o, i) => {
  out.push("", "[[flags]]", `id = ${str(flagIds[i])}`, `value = ${str(shortest(o))}`, `question = ${str("TODO: " + (o.description ?? ""))}`);
});
args.forEach((a, i) => {
  out.push("", "[[slots]]", `id = ${str(argIds[i])}`, `kind = "path"  # or "text"`, `what = ${str("TODO: " + (a.description ?? a.name ?? ""))}`);
  if (a.isOptional) out.push(`default = ""`);
});
if (valued.length) {
  out.push("", "# Options that take a value (model as a slot with `format`, e.g. format = \"-n {}\"):");
  for (const o of valued) out.push(`#   ${list(o.name).join(", ")}: ${String(o.description ?? "").replace(/\s+/g, " ")}`);
}
const subs = list(spec.subcommands);
if (subs.length) {
  out.push("", "# Subcommands (each one usually deserves its own catalog entry, e.g. name = \"git status\"):");
  for (const s of subs) out.push(`#   ${list(s.name).join(", ")}: ${String(s.description ?? "").replace(/\s+/g, " ")}`);
}
console.log(out.join("\n"));
