import type { Environment, ShellResult } from "../ipc";

/** An Odoo environment for tests; `overrides` set what a test cares about. */
export const envFixture = (overrides: Partial<Environment> = {}): Environment => ({
  id: "fixture", name: "Fixture", url: "https://example.test", database: "fixture-db", login: "fixture-user",
  protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true, tags: [], ...overrides,
});
/** An environment named after its database, at https://<name>.test. */
export const envNamed = (name: string, overrides: Partial<Environment> = {}) =>
  envFixture({ id: name, name, url: `https://${name}.test`, database: name, login: "fixture", ...overrides });
/** What the core answers to a Terminal call. */
export const shellResult = (data: unknown, write = false): ShellResult => ({ data, write, upstream_ms: 1, upstream_bytes: 2 });
