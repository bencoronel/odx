#!/usr/bin/env node
// Counts patterns we are phasing out, per file, against scripts/ratchet-baseline.txt.
// New code cannot add any; when old code drops some, the baseline must shrink with it.
// `node scripts/ratchet.mjs` checks; `--update` rewrites the baseline, refusing any increase; `--init` seeds it.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const baselinePath = `${root}scripts/ratchet-baseline.txt`;

// Test code may unwrap and cast freely. Cut at the test module: a lone #[cfg(test)] item can sit above production code.
const rustSource = (path, text) => path.startsWith("crates/") || path.startsWith("apps/")
  ? !/(^|\/)tests\//.test(path) && text.split(/#\[cfg\(test\)\]\s*mod\s/)[0] : false;
const tsSource = (path, text) => path.startsWith("apps/desktop/src/") && !/\.test\.tsx?$|test-setup\.ts$/.test(path) && text;

export const rules = [
  { name: "rust-unwrap", ext: ".rs", source: rustSource, pattern: /(?<!\.lock\(\)\s*)\.(?:unwrap\(\)|expect\()/g,
    // A poisoned std Mutex is already a panic elsewhere; unwrapping `.lock()` is the idiom, not a shortcut.
    fix: "return or handle the error; keep unwrap/expect for invariants in tests" },
  { name: "rust-allow", ext: ".rs", source: rustSource, pattern: /#!?\[allow\(/g,
    fix: "fix the lint, or allow it once in [workspace.lints] with the reason" },
  { name: "ts-any", ext: ".ts", source: tsSource, pattern: /\bas any\b|:\s*any\b|<any>|@ts-ignore|@ts-nocheck/g,
    fix: "use a checked type or `unknown` with narrowing" },
];

export function count(rule, path, text) {
  const source = path.endsWith(rule.ext) || (rule.ext === ".ts" && path.endsWith(".tsx")) ? rule.source(path, text) : false;
  return source ? (source.match(rule.pattern) ?? []).length : 0;
}

/** Map of "rule\tpath" to count, skipping zeros. */
export function collect(files, read) {
  const counts = new Map();
  for (const path of files) {
    const text = read(path);
    for (const rule of rules) {
      const n = count(rule, path, text);
      if (n) counts.set(`${rule.name}\t${path}`, n);
    }
  }
  return counts;
}

export const parse = (text) => new Map(text.split("\n").filter((line) => line && !line.startsWith("#"))
  .map((line) => { const [rule, path, n] = line.split("\t"); return [`${rule}\t${path}`, Number(n)]; }));

/** Lines that grew (errors) and lines that shrank without a baseline update (stale). */
export function compare(baseline, current) {
  const grew = [], stale = [];
  for (const [key, n] of current) if (n > (baseline.get(key) ?? 0)) grew.push(`${key.replace("\t", " ")}: ${baseline.get(key) ?? 0} -> ${n}`);
  for (const [key, n] of baseline) if ((current.get(key) ?? 0) < n) stale.push(`${key.replace("\t", " ")}: ${n} -> ${current.get(key) ?? 0}`);
  return { grew, stale };
}

const format = (counts) => "# rule\tpath\tcount — may only shrink; regenerate with `node scripts/ratchet.mjs --update`.\n"
  + [...counts].sort(([a], [b]) => a.localeCompare(b)).map(([key, n]) => `${key}\t${n}\n`).join("");

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const files = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], { cwd: root, encoding: "utf8" })
    .split("\n").filter((path) => /\.(rs|tsx?)$/.test(path) && existsSync(`${root}${path}`));
  const current = collect(files, (path) => readFileSync(`${root}${path}`, "utf8"));
  const baseline = existsSync(baselinePath) ? parse(readFileSync(baselinePath, "utf8")) : new Map();
  const { grew, stale } = compare(baseline, current);
  const init = process.argv.includes("--init") && !existsSync(baselinePath);
  for (const line of init ? [] : grew) {
    const rule = rules.find((r) => line.startsWith(`${r.name} `));
    console.error(`ratchet: ${line} (${rule.fix})`);
  }
  if (init) {
    writeFileSync(baselinePath, format(current));
    console.log(`ratchet: baseline created with ${current.size} entries`);
  } else if (process.argv.includes("--update")) {
    if (grew.length) process.exit(1);
    writeFileSync(baselinePath, format(current));
    console.log(`ratchet: baseline updated (${stale.length} entries shrank)`);
  } else if (grew.length || stale.length) {
    for (const line of stale) console.error(`ratchet: ${line} — run \`node scripts/ratchet.mjs --update\` to lock in the improvement`);
    process.exit(1);
  }
}
