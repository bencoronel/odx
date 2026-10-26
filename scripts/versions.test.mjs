import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// The release tag is checked against tauri.conf.json; Cargo and npm must say the same version.
test("Cargo, npm and Tauri declare the same app version", () => {
  const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
  const cargo = /\[workspace\.package\][^[]*?\nversion = "([^"]+)"/.exec(read("Cargo.toml"))?.[1];
  const npm = JSON.parse(read("apps/desktop/package.json")).version;
  const tauri = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json")).version;
  assert.ok(cargo, "workspace.package.version in Cargo.toml");
  assert.deepEqual({ npm, tauri }, { npm: cargo, tauri: cargo });
});
