import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ActivityPage from "./ActivityPage";
import { ipc, type ActivityEvent, type AuditDetails, type ActivityPage as Page } from "./ipc";
import { setLanguage } from "./i18n";
import { envNamed } from "./test/fixtures";
afterEach(async () => { cleanup(); vi.restoreAllMocks(); await setLanguage("en"); });
// Rows without details unless a test provides them.
beforeEach(() => { vi.spyOn(ipc, "activityDetail").mockResolvedValue(null); });

const event = (request_id: string, write = false): ActivityEvent => ({
  request_id, at: "2026-10-16T10:00:00Z", environment_id: "env", environment_name: "A",
  operation: `${write ? "write" : "read"}:${request_id}`, outcome: "succeeded", write,
});
/** The core sends details only for the opened request; fixtures keep them on the event. */
type Fixture = ActivityEvent & { details?: AuditDetails | null };
const detailsOf = (...events: Fixture[]) =>
  vi.spyOn(ipc, "activityDetail").mockImplementation(async (id) => events.find((event) => event.request_id === id)?.details ?? null);
function deferred() {
  let resolve!: (page: Page) => void;
  const promise = new Promise<Page>((r) => { resolve = r; });
  return { promise, resolve };
}

it("loads the next page by cursor and discards a page that arrives after the filter changed", async () => {
  const first = { events: [event("first.model")], next_cursor: { at: 1, key: "request:first" } };
  const stale = deferred();
  const writes = { events: [event("write.model", true)], next_cursor: null };
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
  await act(async () => { stale.resolve({ events: [event("stale.model")], next_cursor: null }); });
  expect(screen.queryByText("stale.model")).toBeNull();
  expect(screen.getAllByText("write.model").length).toBeGreaterThan(0);
});

const envs = ["A", "B"].map((name) => envNamed(name, { id: `env-${name}` }));
const at = (env: string, model: string): ActivityEvent => ({
  ...event(model), request_id: `${env}:${model}`, environment_id: `env-${env}`, environment_name: env, operation: `read:${model}`,
});
/** Match core filtering by environment, operation and environment name. */
function fakeCore(rows: ActivityEvent[]) {
  return vi.spyOn(ipc, "activity").mockImplementation(async (_cursor, filter) => ({
    events: rows.filter((row) => (!filter.environment_id || row.environment_id === filter.environment_id)
      && (!filter.q || `${row.operation} ${row.environment_name}`.toLowerCase().includes(filter.q.toLowerCase()))),
    next_cursor: null,
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

it("shows only the redacted request kept by the core and asks for nothing else", async () => {
  const redacted = { ...event("res.partner", true), details: { request: { text: '{"values":{"password":"[redacted]"}}', truncated: true }, error: null, response: null, duration_ms: 3 }, outcome: "failed" };
  const calls = vi.spyOn(ipc, "activity").mockResolvedValue({ events: [redacted], next_cursor: null });
  detailsOf(redacted);
  render(<ActivityPage envs={[]} refreshKey={0} />);
  const detail = await screen.findByRole("region", { name: "Details for write:res.partner" });
  // Details arrive after the row is opened.
  await waitFor(() => expect(detail.textContent).toContain('"password":"[redacted]"'));
  expect(detail.textContent).toContain("Content truncated");
  // Cut JSON cannot be parsed but keeps its colors.
  expect([...detail.querySelectorAll(".hl-model")].map((el) => el.textContent)).toEqual(['"values"', '"password"']);
  expect(detail.querySelector(".hl-str")?.textContent).toBe('"[redacted]"');
  // No undo or restore actions.
  expect(screen.queryByRole("button", { name: /undo|restore/i })).toBeNull();
  expect(calls).toHaveBeenCalledTimes(1);
});

it("colors the request JSON, opens multi-line strings as a block and copies the original text", async () => {
  const text = JSON.stringify({ model: "ir.actions.server", args: [{ model_id: 7, code: "a = 1\nif a:\n    log(a)\n" }], kwargs: { context: {} } });
  const created = { ...event("ir.actions.server", true), details: { request: { text, truncated: false } } } as Fixture;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [created], next_cursor: null });
  detailsOf(created);
  const copy = vi.fn().mockResolvedValue(undefined);
  Object.assign(navigator, { clipboard: { writeText: copy } });
  render(<ActivityPage envs={[]} refreshKey={0} />);
  const detail = await screen.findByRole("region", { name: "Details for write:ir.actions.server" });
  await waitFor(() => expect(detail.querySelector(".hl-model")).toBeTruthy());
  expect([...detail.querySelectorAll(".hl-model")].map((el) => el.textContent)).toEqual(['"model"', '"args"', '"model_id"', '"code"', '"kwargs"', '"context"']);
  expect(detail.querySelector(".hl-num")?.textContent).toBe("7");
  expect(detail.querySelector(".json-block")?.textContent).toBe("a = 1\nif a:\n    log(a)");
  fireEvent.click(within(detail).getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(copy).toHaveBeenCalledWith(text));
});

it("opens a complete list of records as a table and leaves other responses as JSON", async () => {
  const read = { ...event("res.partner"), environment_name: "A", details: { response: { text: JSON.stringify([{ id: 1, name: "X" }, { id: 2, email: "y@test" }]), truncated: false } } } as Fixture;
  const count = { ...event("res.users"), request_id: "count", details: { response: { text: "3", truncated: false } } } as Fixture;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [read, count], next_cursor: null });
  detailsOf(read, count);
  const open = vi.spyOn(ipc, "openTableWindow").mockResolvedValue(undefined);
  render(<ActivityPage envs={[]} refreshKey={0} />);
  const detail = await screen.findByRole("region", { name: "Details for read:res.partner" });
  fireEvent.click(await within(detail).findByRole("button", { name: "Open as table" }));
  expect(open).toHaveBeenCalledWith({ table: { columns: ["id", "name", "email"], rows: [[1, "X", undefined], [2, undefined, "y@test"]] }, title: "res.partner · A" }, "res.partner · A");
  fireEvent.click(screen.getAllByText("res.users")[0]);
  const other = await screen.findByRole("region", { name: "Details for read:res.users" });
  expect(within(other).queryByRole("button", { name: "Open as table" })).toBeNull();
});

it("filters by period and client from the panel; the period becomes a moving since", async () => {
  const calls = vi.spyOn(ipc, "activity").mockResolvedValue({ events: [event("res.partner")], next_cursor: null });
  vi.spyOn(ipc, "clients").mockResolvedValue([{ id: "c1", label: "Claude", revoked: false }, { id: "c2", label: "Old", revoked: true }] as never);
  render(<ActivityPage envs={[]} refreshKey={0} />);
  await screen.findAllByText("res.partner");
  fireEvent.click(screen.getByRole("button", { name: "Filters" }));
  expect(screen.queryByRole("radio", { name: "Old" })).toBeNull();
  const before = Date.now();
  fireEvent.click(screen.getByRole("radio", { name: "Last hour" }));
  await waitFor(() => expect(calls.mock.lastCall?.[1].since).toBeTruthy());
  const since = Date.parse(calls.mock.lastCall![1].since!);
  expect(since).toBeGreaterThanOrEqual(before - 3_600_000 - 1000);
  expect(since).toBeLessThanOrEqual(Date.now() - 3_600_000 + 1000);
  expect(calls.mock.lastCall![1]).not.toHaveProperty("period");
  fireEvent.click(screen.getByRole("radio", { name: "Claude" }));
  await waitFor(() => expect(calls.mock.lastCall?.[1].client_id).toBe("c1"));
  expect(screen.getByRole("button", { name: "Remove filter Last hour" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Remove filter Claude" }));
  await waitFor(() => expect(calls.mock.lastCall?.[1].client_id).toBeUndefined());
});

it("localizes a structured activity error live while retaining its redacted detail", async () => {
  await setLanguage("en");
  const failed = {
    ...event("res.partner", true), outcome: "failed",
    details: { error_code: "upstream.remote", error_params: {}, error: { text: '"Access denied [redacted]"', truncated: false } },
  } as Fixture;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [failed], next_cursor: null });
  detailsOf(failed);
  render(<ActivityPage envs={[]} refreshKey={0} />);
  expect(await screen.findByText("Odoo returned an error.")).toBeTruthy();
  expect(screen.getByText("Access denied [redacted]", { exact: false })).toBeTruthy();
  await setLanguage("es");
  expect(screen.getByText("Odoo devolvió un error.")).toBeTruthy();
});

it("keeps loaded pages when the app refreshes and stops offering more past the row cap", async () => {
  const page = (from: number, count: number) => ({ events: Array.from({ length: count }, (_, i) => event(`m${from + i}`)), next_cursor: { at: from + count, key: `k${from + count}` } });
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
    details: { request: { text: JSON.stringify({ model: "res.partner", method: "search_read", args: [[["id", "=", 7]]], kwargs: { fields: ["name"], context: { lang: "es_ES" } } }), truncated: false } } } as Fixture;
  const old = { ...event("res.users", true), request_id: "old", at: new Date(now - 86_400_000).toISOString(),
    details: { request: { text: JSON.stringify({ model: "res.users", method: "write", args: [[1], {}], kwargs: {} }), truncated: false } } } as Fixture;
  vi.spyOn(ipc, "activity").mockResolvedValue({ events: [read, old], next_cursor: null });
  detailsOf(read, old);
  const openInShell = vi.fn();
  render(<ActivityPage envs={envs} refreshKey={0} openInShell={openInShell} />);
  await screen.findByText("Today");
  expect(screen.getByText("Yesterday")).toBeTruthy();
  fireEvent.click(await screen.findByRole("button", { name: "Repeat in Terminal" }));
  expect(openInShell).toHaveBeenCalledWith("A", 'call res.partner search_read [[["id","=",7]]] {"fields":["name"]}');
  fireEvent.click(screen.getAllByRole("option")[1]);
  expect(screen.queryByRole("button", { name: "Repeat in Terminal" })).toBeNull();
});

it("holds refreshed events behind a notice while the list is scrolled", async () => {
  let rows = [event("one")];
  vi.spyOn(ipc, "activity").mockImplementation(async () => ({ events: rows, next_cursor: null }));
  const { rerender } = render(<ActivityPage envs={[]} refreshKey={1} />);
  await screen.findAllByText("one");
  screen.getByRole("listbox", { name: "Activity" }).scrollTop = 100;
  rows = [event("two"), event("one")];
  rerender(<ActivityPage envs={[]} refreshKey={2} />);
  fireEvent.click(await screen.findByRole("button", { name: "1 new event" }));
  await waitFor(() => expect(listed()).toEqual(["two", "one"]));
});
