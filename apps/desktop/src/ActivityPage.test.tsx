import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import ActivityPage from "./ActivityPage";
import { ipc, type ActivityEvent, type ActivityPage as Page } from "./ipc";
import { setLanguage } from "./i18n";
afterEach(async () => { cleanup(); vi.restoreAllMocks(); await setLanguage("en"); });

const event = (request_id: string, write = false): ActivityEvent => ({
  request_id, at: "2026-10-16T10:00:00Z", environment_id: "env", environment_name: "A",
  operation: `${write ? "write" : "read"}:${request_id}`, outcome: "succeeded", write,
});
function deferred() {
  let resolve!: (page: Page) => void;
  const promise = new Promise<Page>((r) => { resolve = r; });
  return { promise, resolve };
}

it("loads the next page by cursor and discards a page that arrives after the filter changed", async () => {
  const first = { events: [event("first.model")], next_cursor: { at: 1, key: "request:first" }, complete: true };
  const stale = deferred();
  const writes = { events: [event("write.model", true)], next_cursor: null, complete: true };
  const calls = vi.spyOn(ipc, "activity")
    .mockResolvedValueOnce(first)
    .mockReturnValueOnce(stale.promise)
    .mockResolvedValueOnce(writes);
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findByText("first.model");
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  expect(calls).toHaveBeenLastCalledWith(first.next_cursor, {});
  fireEvent.click(screen.getByRole("button", { name: "Writes" }));
  await screen.findByText("write.model");
  expect(calls).toHaveBeenLastCalledWith(null, { writes_only: true });
  await act(async () => { stale.resolve({ events: [event("stale.model")], next_cursor: null, complete: true }); });
  expect(screen.queryByText("stale.model")).toBeNull();
  expect(screen.getByText("write.model")).toBeTruthy();
});

const envs = ["A", "B"].map((name) => ({
  id: `env-${name}`, name, url: `https://${name}.test`, database: name, login: "fixture",
  protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only" as const, has_secret: true,
}));
const at = (env: string, model: string): ActivityEvent => ({
  ...event(model), request_id: `${env}:${model}`, environment_id: `env-${env}`, environment_name: env, operation: `read:${model}`,
});
/** Match core filtering by environment, operation and environment name. */
function fakeCore(rows: ActivityEvent[]) {
  return vi.spyOn(ipc, "activity").mockImplementation(async (_cursor, filter) => ({
    events: rows.filter((row) => (!filter.environment_id || row.environment_id === filter.environment_id)
      && (!filter.q || `${row.operation} ${row.environment_name}`.toLowerCase().includes(filter.q.toLowerCase()))),
    next_cursor: null, complete: true,
  }));
}
const rows = [at("A", "res.partner"), at("B", "res.users"), at("A", "res.country")];

it("filters by environment from the selector and from a row, and clears back to all", async () => {
  const calls = fakeCore(rows);
  render(<ActivityPage envs={envs} refreshKey={0} />);
  await screen.findByText("res.users");
  fireEvent.click(screen.getByRole("combobox", { name: "Environment" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "B" }));
  await waitFor(() => expect(screen.queryByText("res.partner")).toBeNull());
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: "env-B" });
  expect(screen.getByText("res.users")).toBeTruthy();
  expect(screen.queryByText("res.country")).toBeNull();

  fireEvent.click(screen.getByRole("combobox", { name: "Environment" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "All environments" }));
  await screen.findByText("res.partner");
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: undefined });
  fireEvent.click(screen.getAllByRole("button", { name: "A" })[0]);
  await waitFor(() => expect(screen.queryByText("res.users")).toBeNull());
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: "env-A" });
  expect(screen.getByRole("combobox", { name: "Environment" }).textContent).toBe("A");
  expect(screen.getByText("res.partner")).toBeTruthy();
  expect(screen.getByText("res.country")).toBeTruthy();
});

it("sends the trimmed search text and shows the matching rows, or an empty state when none match", async () => {
  const calls = fakeCore(rows);
  render(<ActivityPage envs={envs} refreshKey={0} />);
  await screen.findByText("res.users");
  const search = screen.getByRole("searchbox", { name: "Search activity" });
  fireEvent.change(search, { target: { value: "  Country " } });
  await waitFor(() => expect(screen.queryByText("res.users")).toBeNull());
  expect(calls).toHaveBeenLastCalledWith(null, { q: "Country" });
  expect(screen.getByText("res.country")).toBeTruthy();
  expect(screen.queryByText("res.partner")).toBeNull();

  fireEvent.change(search, { target: { value: "no-match" } });
  await screen.findByText("No events match the filters.");
  expect(calls).toHaveBeenLastCalledWith(null, { q: "no-match" });
  expect(screen.queryByText("res.country")).toBeNull();
});

it("warns when part of the history could not be read and shows what was recovered", async () => {
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [event("recovered.model")], next_cursor: null, complete: false });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findByText("recovered.model");
  expect(screen.getByText(/Part of the history could not be read/)).toBeTruthy();
});

it("does not warn while the history is complete", async () => {
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [event("full.model")], next_cursor: null, complete: true });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findByText("full.model");
  expect(screen.queryByText(/Parte del historial/)).toBeNull();
});

it("shows only the redacted request kept by the core and asks for nothing else", async () => {
  const redacted = { ...event("res.partner", true), details: { request: { text: '{"values":{"password":"[redacted]"}}', truncated: true }, error: null, response: null, duration_ms: 3 }, outcome: "failed" };
  const calls = vi.spyOn(ipc, "activity").mockResolvedValue({ events: [redacted], next_cursor: null, complete: true });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  fireEvent.click(await screen.findByRole("button", { name: "Show details for write:res.partner" }));
  const detail = screen.getByRole("region", { name: "Details for write:res.partner" });
  expect(detail.textContent).toContain('"password":"[redacted]"');
  expect(detail.textContent).toContain("Content truncated");
  // No undo or restore actions.
  expect(screen.queryByRole("button", { name: /undo|restore/i })).toBeNull();
  expect(calls).toHaveBeenCalledTimes(1);
});

it("localizes a structured activity error live while retaining its redacted detail", async () => {
  await setLanguage("en");
  const failed = {
    ...event("res.partner", true), outcome: "failed",
    details: { error_code: "upstream.remote", error_params: {}, error: { text: '"Access denied [redacted]"', truncated: false } },
  } as ActivityEvent;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [failed], next_cursor: null, complete: true });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  fireEvent.click(await screen.findByRole("button", { name: "Show details for write:res.partner" }));
  expect(screen.getByText("Odoo returned an error.")).toBeTruthy();
  expect(screen.getByText("Access denied [redacted]", { exact: false })).toBeTruthy();
  await setLanguage("es");
  expect(screen.getByText("Odoo devolvió un error.")).toBeTruthy();
});
