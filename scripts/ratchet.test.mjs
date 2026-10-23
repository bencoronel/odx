import assert from "node:assert/strict";
import { test } from "node:test";
import { collect, compare, parse } from "./ratchet.mjs";

test("counts outside test code only", () => {
  const files = {
    "crates/a/src/lib.rs": "x.unwrap(); y.expect(\"e\"); m.lock().unwrap(); m.lock().expect(\"m\"); m\n    .lock()\n    .unwrap();\n#[cfg(test)]\nmod tests { z.unwrap(); }",
    "crates/a/tests/it.rs": "x.unwrap();",
    "crates/a/src/vault.rs": "#[cfg(test)]\nuse std::fmt;\nfn open() { f.unwrap(); }\n#[cfg(test)]\nmod tests { g.unwrap(); }",
    "apps/desktop/src/A.tsx": "const a = b as any; // @ts-ignore",
    "apps/desktop/src/A.test.tsx": "const a = b as any;",
  };
  const counts = collect(Object.keys(files), (path) => files[path]);
  assert.deepEqual([...counts], [["rust-unwrap\tcrates/a/src/lib.rs", 2], ["rust-unwrap\tcrates/a/src/vault.rs", 1], ["ts-any\tapps/desktop/src/A.tsx", 2]]);
});

test("new entries and increases fail, decreases must update the baseline", () => {
  const baseline = parse("# header\nrust-unwrap\ta.rs\t2\nts-any\tb.ts\t1\n");
  const { grew, stale } = compare(baseline, new Map([["rust-unwrap\ta.rs", 3], ["rust-allow\tc.rs", 1]]));
  assert.deepEqual(grew, ["rust-unwrap a.rs: 2 -> 3", "rust-allow c.rs: 0 -> 1"]);
  assert.deepEqual(stale, ["ts-any b.ts: 1 -> 0"]);
});
