import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  await screen.findAllByText("first.model");
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  expect(calls).toHaveBeenLastCalledWith(first.next_cursor, {});
  fireEvent.click(screen.getByRole("button", { name: "Filters" }));
  fireEvent.click(screen.getByRole("checkbox", { name: "Writes" }));
  await screen.findAllByText("write.model");
  expect(calls).toHaveBeenLastCalledWith(null, { writes_only: true });
  await act(async () => { stale.resolve({ events: [event("stale.model")], next_cursor: null, complete: true }); });
  expect(screen.queryByText("stale.model")).toBeNull();
  expect(screen.getAllByText("write.model").length).toBeGreaterThan(0);
});

const envs = ["A", "B"].map((name) => ({
  id: `env-${name}`, name, url: `https://${name}.test`, database: name, login: "fixture",
  protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only" as const, has_secret: true, tags: [],
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

/** Models shown in the list (the detail pane repeats the selected one). */
const listed = () => [...document.querySelectorAll(".activity-item strong")].map((el) => el.textContent);

it("filters by environment from the filter panel and from the detail, shows it as a chip and clears back to all", async () => {
  const calls = fakeCore(rows);
  render(<ActivityPage envs={envs} refreshKey={0} />);
  await waitFor(() => expect(listed()).toEqual(["res.partner", "res.users", "res.country"]));
  fireEvent.click(screen.getByRole("button", { name: "Filters" }));
  fireEvent.click(screen.getByRole("radio", { name: "B" }));
  await waitFor(() => expect(listed()).toEqual(["res.users"]));
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: "env-B" });
  expect(screen.getByRole("button", { name: "Remove filter B" })).toBeTruthy();

  fireEvent.click(screen.getByRole("radio", { name: "All environments" }));
  await waitFor(() => expect(listed()).toHaveLength(3));
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: undefined });
  fireEvent.click(within(screen.getByRole("region", { name: "Details for read:res.partner" })).getByRole("button", { name: "A" }));
  await waitFor(() => expect(listed()).toEqual(["res.partner", "res.country"]));
  expect(calls).toHaveBeenLastCalledWith(null, { environment_id: "env-A" });
  fireEvent.click(screen.getByRole("button", { name: "Remove filter A" }));
  await waitFor(() => expect(listed()).toHaveLength(3));
});

it("sends the trimmed search text and shows the matching rows, or an empty state when none match", async () => {
  const calls = fakeCore(rows);
  render(<ActivityPage envs={envs} refreshKey={0} />);
  await screen.findAllByText("res.users");
  const search = screen.getByRole("searchbox", { name: "Search activity" });
  fireEvent.change(search, { target: { value: "  Country " } });
  await waitFor(() => expect(listed()).toEqual(["res.country"]));
  expect(calls).toHaveBeenLastCalledWith(null, { q: "Country" });

  fireEvent.change(search, { target: { value: "no-match" } });
  await screen.findByText("No events match the filters.");
  expect(calls).toHaveBeenLastCalledWith(null, { q: "no-match" });
  expect(listed()).toEqual([]);
  fireEvent.click(within(screen.getByRole("listbox", { name: "Activity" })).getByRole("button", { name: "Clear filters" }));
  await waitFor(() => expect(listed()).toHaveLength(3));
});

it("warns when part of the history could not be read and shows what was recovered", async () => {
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [event("recovered.model")], next_cursor: null, complete: false });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findAllByText("recovered.model");
  expect(screen.getByText(/Part of the history could not be read/)).toBeTruthy();
});

it("does not warn while the history is complete", async () => {
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [event("full.model")], next_cursor: null, complete: true });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findAllByText("full.model");
  expect(screen.queryByText(/Parte del historial/)).toBeNull();
});

it("shows only the redacted request kept by the core and asks for nothing else", async () => {
  const redacted = { ...event("res.partner", true), details: { request: { text: '{"values":{"password":"[redacted]"}}', truncated: true }, error: null, response: null, duration_ms: 3 }, outcome: "failed" };
  const calls = vi.spyOn(ipc, "activity").mockResolvedValue({ events: [redacted], next_cursor: null, complete: true });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  const detail = await screen.findByRole("region", { name: "Details for write:res.partner" });
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
  expect(await screen.findByText("Odoo returned an error.")).toBeTruthy();
  expect(screen.getByText("Access denied [redacted]", { exact: false })).toBeTruthy();
  await setLanguage("es");
  expect(screen.getByText("Odoo devolvió un error.")).toBeTruthy();
});

it("keeps loaded pages when the app refreshes and stops offering more past the row cap", async () => {
  const page = (from: number, count: number) => ({ events: Array.from({ length: count }, (_, i) => event(`m${from + i}`)), next_cursor: { at: from + count, key: `k${from + count}` }, complete: true });
  const calls = vi.spyOn(ipc, "activity").mockImplementation(async (cursor) => cursor ? page(cursor.at, 500) : page(0, 500));
  const { rerender } = render(<ActivityPage envs={[]} refreshKey={1} />);
  await screen.findAllByText("m0");
  fireEvent.click(screen.getByRole("button", { name: "Load more" }));
  await screen.findByText("m999");
  rerender(<ActivityPage envs={[]} refreshKey={2} />);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(screen.getByText("m999")).toBeTruthy();
  expect(calls).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  expect(screen.getByText(/latest 1,?000 records/)).toBeTruthy();
});

it("reloads the first page when the app refreshes and no extra page is shown", async () => {
  const calls = fakeCore(rows);
  const { rerender } = render(<ActivityPage envs={[]} refreshKey={1} />);
  await screen.findAllByText("res.users");
  rerender(<ActivityPage envs={[]} refreshKey={2} />);
  await waitFor(() => expect(calls).toHaveBeenCalledTimes(2));
});

it("groups by day and offers to repeat whole, unredacted reads in the Terminal", async () => {
  const now = Date.now();
  const read = { ...at("A", "res.partner"), at: new Date(now - 5 * 60_000).toISOString(),
    details: { request: { text: JSON.stringify({ model: "res.partner", method: "search_read", args: [[["id", "=", 7]]], kwargs: { fields: ["name"], context: { lang: "es_ES" } } }), truncated: false } } } as ActivityEvent;
  const old = { ...event("res.users", true), request_id: "old", at: new Date(now - 86_400_000).toISOString(),
    details: { request: { text: JSON.stringify({ model: "res.users", method: "write", args: [[1], {}], kwargs: {} }), truncated: false } } } as ActivityEvent;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [read, old], next_cursor: null, complete: true });
  const openInShell = vi.fn();
  render(<ActivityPage envs={envs} refreshKey={0} openInShell={openInShell} />);
  await screen.findByText("Today");
  expect(screen.getByText("Yesterday")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Repeat in Terminal" }));
  expect(openInShell).toHaveBeenCalledWith("A", 'call res.partner search_read [[["id","=",7]]] {"fields":["name"]}');
  fireEvent.click(screen.getAllByRole("option")[1]);
  expect(screen.queryByRole("button", { name: "Repeat in Terminal" })).toBeNull();
});

it("holds refreshed events behind a notice while the list is scrolled", async () => {
  let rows = [event("one")];
  vi.spyOn(ipc, "activity").mockImplementation(async () => ({ events: rows, next_cursor: null, complete: true }));
  const { rerender } = render(<ActivityPage envs={[]} refreshKey={1} />);
  await screen.findAllByText("one");
  screen.getByRole("listbox", { name: "Activity" }).scrollTop = 100;
  rows = [event("two"), event("one")];
  rerender(<ActivityPage envs={[]} refreshKey={2} />);
  fireEvent.click(await screen.findByRole("button", { name: "1 new event" }));
  await waitFor(() => expect(listed()).toEqual(["two", "one"]));
});
