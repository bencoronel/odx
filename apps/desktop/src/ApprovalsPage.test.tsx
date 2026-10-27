import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useState } from "react";
import ApprovalsPage from "./ApprovalsPage";
import type { Pending } from "./ipc";
afterEach(cleanup);
// jsdom lacks scrollIntoView, which the page uses for keyboard selection.
Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
const request = (id: string, method: string, args: unknown, kwargs: unknown = {}, expiresIn = 600_000): Pending => ({
  id, environment_id: "env-A", environment_name: "A", model: "res.partner", method,
  args, kwargs, context: {}, created_at: new Date().toISOString(), expires_at: new Date(Date.now() + expiresIn).toISOString(),
});
type Props = Parameters<typeof ApprovalsPage>[0];
/** The app keeps the filter; this stands in for it. */
function Harness(props: Omit<Props, "filter" | "setFilter" | "openEnvironment"> & { openEnvironment?: Props["openEnvironment"] }) {
  const [filter, setFilter] = useState("");
  return <ApprovalsPage openEnvironment={vi.fn()} {...props} filter={filter} setFilter={setFilter} />;
}
const page = (pending: Pending[], deciding = new Set<string>(), decide = vi.fn(), openInShell = vi.fn(), decideMany = vi.fn()) =>
  render(<Harness envs={[]} deciding={deciding} decide={decide} decideMany={decideMany} openInShell={openInShell} pending={pending} />);

it("counts the records an unlink deletes, from positional or named ids", () => {
  const pending = [request("one", "unlink", [[7]]), request("many", "unlink", [[1, 2, 3]]), request("named", "unlink", [], { ids: [4, 5] })];
  page(pending);
  expect(screen.getByRole("button", { name: "View in Terminal" })).toBeTruthy();
  for (const [index, name] of [[1, "View 3 in Terminal"], [2, "View 2 in Terminal"]] as const) {
    fireEvent.click(screen.getAllByRole("option")[index], { detail: 1 });
    expect(screen.getByRole("button", { name })).toBeTruthy();
  }
});

it("hides empty call sections", () => {
  page([request("p", "write", [[1], { name: "x" }])]);
  expect(screen.getByText("Arguments")).toBeTruthy();
  expect(screen.queryByText("Named arguments")).toBeNull();
  expect(screen.queryByText("Context")).toBeNull();
});

it("locks only the request being decided and ignores its shortcuts", () => {
  const decide = vi.fn();
  page([request("a", "write", []), request("b", "write", [])], new Set(["a"]), decide);
  expect((screen.getByRole("button", { name: "Processing…" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.keyDown(document.body, { key: "a" });
  expect(decide).not.toHaveBeenCalled();
  fireEvent.keyDown(document.body, { key: "j" });
  fireEvent.keyDown(document.body, { key: "a" });
  expect(decide).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }), true);
});

it("explains why an expired request cannot be approved", () => {
  const decide = vi.fn();
  page([request("old", "write", [], {}, -1000)], new Set(), decide);
  const banner = () => screen.getByText(/has expired and can no longer be approved/);
  expect(banner().getAttribute("role")).toBe("status");
  fireEvent.keyDown(document.body, { key: "a" });
  expect(decide).not.toHaveBeenCalled();
  expect(banner().className).toContain("nudge");
});

it("lists written fields, opens the target records in the Terminal and keeps the raw call", () => {
  const openInShell = vi.fn();
  page([request("w", "write", [[7, 8], { name: "Ana", country_id: 68 }])], new Set(), vi.fn(), openInShell);
  expect(screen.getByRole("rowheader", { name: "name" }).nextElementSibling!.textContent).toBe("Ana");
  expect(screen.getByRole("rowheader", { name: "country_id" }).nextElementSibling!.textContent).toBe("68");
  fireEvent.click(screen.getByRole("button", { name: "View 2 in Terminal" }));
  expect(openInShell).toHaveBeenCalledWith(expect.objectContaining({ id: "w" }), "read res.partner 7,8 -f display_name,name,country_id");
  expect(screen.getByText("Raw call")).toBeTruthy();
});

it("reads deleted records with the default fields, one button whatever the count", () => {
  const openInShell = vi.fn();
  const ids = Array.from({ length: 100 }, (_, i) => i + 1);
  page([request("u", "unlink", [ids])], new Set(), vi.fn(), openInShell);
  fireEvent.click(screen.getByRole("button", { name: "View 100 in Terminal" }));
  expect(openInShell).toHaveBeenCalledWith(expect.anything(), `read res.partner ${ids.join(",")}`);
});

it("reads named create values as one table per record", () => {
  page([request("c", "create", [], { vals_list: [{ name: "A" }, { name: "B" }] })]);
  expect(screen.getByText("Record 1")).toBeTruthy();
  expect(screen.getByText("Record 2")).toBeTruthy();
  expect(screen.queryByText("Records")).toBeNull();
});

it("rejects at once with R, and asks for a reason with Shift+R", () => {
  const decide = vi.fn();
  page([request("w", "write", [[1], { name: "x" }])], new Set(), decide);
  expect(screen.queryByRole("textbox", { name: "Rejection reason" })).toBeNull();
  fireEvent.keyDown(document.body, { key: "r" });
  expect(decide).toHaveBeenLastCalledWith(expect.objectContaining({ id: "w" }), false);
  fireEvent.keyDown(document.body, { key: "R", shiftKey: true });
  const input = screen.getByRole("textbox", { name: "Rejection reason" });
  fireEvent.keyDown(input, { key: "Escape" });
  expect(screen.queryByRole("textbox", { name: "Rejection reason" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Reject with reason" }));
  expect(screen.getByRole("textbox", { name: "Rejection reason" })).toBeTruthy();
});

it("sends the typed reason when rejecting with Enter", () => {
  const decide = vi.fn();
  page([request("w", "write", [[1], { name: "x" }])], new Set(), decide);
  fireEvent.keyDown(document.body, { key: "R", shiftKey: true });
  const input = screen.getByRole("textbox", { name: "Rejection reason" });
  fireEvent.change(input, { target: { value: "wrong partner" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(decide).toHaveBeenCalledWith(expect.objectContaining({ id: "w" }), false, "wrong partner");
});

it("selects the request that takes the decided one's place", () => {
  const pending = [request("a", "write", []), request("b", "write", []), request("c", "write", [])];
  const view = page(pending);
  fireEvent.click(screen.getAllByRole("option")[1], { detail: 1 });
  view.rerender(<Harness envs={[]} deciding={new Set()} decide={vi.fn()} decideMany={vi.fn()} openInShell={vi.fn()} pending={[pending[0], pending[2]]} />);
  expect(screen.getAllByRole("option")[1].getAttribute("aria-selected")).toBe("true");
});

it("keeps the request that took the decided one's place when a new one arrives before it, and ignores held keys", () => {
  const pending = [request("a", "write", []), request("b", "write", []), request("c", "write", [])];
  const decide = vi.fn();
  const view = page(pending, new Set(), decide);
  fireEvent.click(screen.getAllByRole("option")[1], { detail: 1 });
  const props = { envs: [], deciding: new Set<string>(), decide, decideMany: vi.fn(), openInShell: vi.fn() };
  // b decided: c takes row 1; then z arrives first and c moves to row 2.
  view.rerender(<Harness {...props} pending={[pending[0], pending[2]]} />);
  view.rerender(<Harness {...props} pending={[request("z", "write", []), pending[0], pending[2]]} />);
  fireEvent.keyDown(document.body, { key: "a", repeat: true });
  expect(decide).not.toHaveBeenCalled();
  fireEvent.keyDown(document.body, { key: "a" });
  expect(decide).toHaveBeenCalledWith(pending[2], true);
});

it("filters by model, method, environment or client and keeps keys within the matches", () => {
  const decide = vi.fn();
  const sale = { ...request("s", "write", []), model: "sale.order" };
  page([request("a", "unlink", [[1]]), sale, request("c", "create", [], { vals_list: [{}] })], new Set(), decide);
  fireEvent.keyDown(document.body, { key: "/" });
  const search = screen.getByRole("searchbox", { name: "Filter requests" });
  expect(document.activeElement).toBe(search);
  fireEvent.change(search, { target: { value: "SALE" } });
  expect(screen.getAllByRole("option").map((o) => o.getAttribute("aria-label"))).toEqual(["write on sale.order, A"]);
  expect(screen.getAllByRole("status").some((s) => s.textContent?.includes("1 of 3"))).toBe(true);
  search.blur();
  fireEvent.keyDown(document.body, { key: "a" });
  expect(decide).toHaveBeenCalledWith(expect.objectContaining({ id: "s" }), true);
  fireEvent.change(search, { target: { value: "nothing" } });
  fireEvent.click(screen.getByRole("button", { name: "Clear filter" }));
  expect(screen.getAllByRole("option")).toHaveLength(3);
});


it("keeps the layout when nothing is pending and says so in the list", () => {
  page([]);
  expect(screen.getByRole("searchbox", { name: "Filter requests" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "All caught up" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Clear filter" })).toBeNull();
});

it("draws at most 20 record tables for a large create", () => {
  page([request("c", "create", [Array.from({ length: 1000 }, (_, i) => ({ name: `R${i}` }))])]);
  expect(document.querySelectorAll(".values-table")).toHaveLength(20);
  expect(screen.getByText("and 980 more records in the raw call")).toBeTruthy();
});

const ids = (calls: unknown[][]) => (calls.at(-1)![0] as Pending[]).map((p) => p.id);
const selectedIds = () => screen.getAllByRole("option").filter((o) => o.getAttribute("aria-selected") === "true").map((o) => o.getAttribute("aria-label"));

it("selects several requests with the mouse and decides them together", () => {
  const decideMany = vi.fn();
  const pending = ["a", "b", "c", "d"].map((id) => ({ ...request(id, "write", [[1], { x: 1 }]), model: `m.${id}` }));
  page(pending, new Set(), vi.fn(), vi.fn(), decideMany);
  const options = () => screen.getAllByRole("option");
  fireEvent.click(options()[0], { detail: 1 });
  fireEvent.click(options()[2], { ctrlKey: true, detail: 1 });
  expect(selectedIds()).toEqual(["write on m.a, A", "write on m.c, A"]);
  expect(screen.getByText("2 requests selected")).toBeTruthy();
  fireEvent.click(options()[3], { shiftKey: true, detail: 1 });
  expect(selectedIds()).toEqual(["write on m.c, A", "write on m.d, A"]);
  fireEvent.click(screen.getByRole("button", { name: "Approve 2" }));
  expect(ids(decideMany.mock.calls)).toEqual(["c", "d"]);
  expect(decideMany.mock.calls.at(-1)![1]).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Clear selection" }));
  expect(screen.queryByText(/requests selected/)).toBeNull();
});

it("selects several requests with the keyboard; A and R act on all of them", () => {
  const decideMany = vi.fn();
  const decide = vi.fn();
  const pending = ["a", "b", "c"].map((id) => ({ ...request(id, "write", [[1], { x: 1 }]), model: `m.${id}` }));
  page([...pending, request("old", "write", [], {}, -1000)], new Set(), decide, vi.fn(), decideMany);
  fireEvent.keyDown(document.body, { key: "ArrowDown", shiftKey: true });
  expect(selectedIds()).toEqual(["write on m.a, A", "write on m.b, A"]);
  fireEvent.keyDown(document.body, { key: "Escape" });
  fireEvent.keyDown(document.body, { key: "ArrowUp" });
  // Space marks; arrows then move without losing the marks.
  fireEvent.keyDown(document.body, { key: " " });
  fireEvent.keyDown(document.body, { key: "ArrowDown" });
  fireEvent.keyDown(document.body, { key: "ArrowDown" });
  fireEvent.keyDown(document.body, { key: " " });
  expect(selectedIds()).toEqual(["write on m.a, A", "write on m.c, A"]);
  fireEvent.keyDown(document.body, { key: "ArrowDown" });
  fireEvent.keyDown(document.body, { key: " " });
  expect(screen.getByText("3 requests selected")).toBeTruthy();
  // Expired requests are skipped when approving, not when rejecting.
  fireEvent.keyDown(document.body, { key: "a" });
  expect(ids(decideMany.mock.calls)).toEqual(["a", "c"]);
  fireEvent.keyDown(document.body, { key: "r" });
  expect(ids(decideMany.mock.calls)).toEqual(["a", "c", "old"]);
  fireEvent.keyDown(document.body, { key: "a", ctrlKey: true });
  expect(screen.getByText("4 requests selected")).toBeTruthy();
  fireEvent.keyDown(document.body, { key: "Escape" });
  expect(screen.queryByText(/requests selected/)).toBeNull();
  expect(decide).not.toHaveBeenCalled();
});


it("shows a single mark and keeps it while the cursor moves", () => {
  const decideMany = vi.fn();
  const pending = ["a", "b", "c"].map((id) => ({ ...request(id, "write", [[1], { x: 1 }]), model: `m.${id}` }));
  page(pending, new Set(), vi.fn(), vi.fn(), decideMany);
  fireEvent.click(screen.getAllByRole("option")[1], { detail: 1 });
  fireEvent.keyDown(document.body, { key: " " });
  // WebKit follows Space on a focused option with a click of detail 0; it must not clear the mark.
  fireEvent.click(screen.getAllByRole("option")[1], { detail: 0 });
  expect(selectedIds()).toEqual(["write on m.b, A"]);
  fireEvent.keyDown(document.body, { key: "ArrowDown" });
  expect(selectedIds()).toEqual(["write on m.b, A"]);
  expect(screen.getAllByRole("option")[2].className).toContain("cursor");
  // The panel and A act on the marked request, not on the one under the cursor.
  expect(screen.getByText("1 request selected")).toBeTruthy();
  fireEvent.keyDown(document.body, { key: "a" });
  expect(ids(decideMany.mock.calls)).toEqual(["b"]);  fireEvent.keyDown(document.body, { key: "ArrowUp" });
  fireEvent.keyDown(document.body, { key: " " });
  expect(screen.queryByText(/selected/)).toBeNull();
  expect(selectedIds()).toEqual(["write on m.b, A"]);
});

it("keeps the legend short and shows every shortcut on ?", () => {
  page([request("a", "write", [])]);
  const toggle = screen.getByRole("button", { name: "? Shortcuts" });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByText("Reject with reason")).toBeNull();
  fireEvent.keyDown(document.body, { key: "?" });
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  for (const text of ["Navigate", "Decide", "Select several", "Reject with reason", "Mark or unmark", "Select all shown", "Space"]) expect(screen.getByText(text)).toBeTruthy();
  fireEvent.click(toggle);
  expect(screen.queryByText("Reject with reason")).toBeNull();
});

it("spells out relational commands and marks every one that can remove records", () => {
  page([request("w", "write", [[1], { category_id: [[6, 0, [3, 4]], [4, 5], [3, 7], [2, 8], [5]], name: "x" }])]);
  const cell = screen.getByRole("rowheader", { name: "category_id" }).nextElementSibling!;
  const steps = [...cell.querySelectorAll("li")].map((li) => li.textContent);
  expect(steps).toEqual(["Replace with 3, 4 (may delete records)", "Link 5", "Unlink 7 (may delete records)", "Delete 8 (removes the record)", "Unlink all (may delete records)"]);
  // Only link is safe here; the rest can remove records, as odx-core counts them.
  expect([...cell.querySelectorAll("li")].map((li) => li.classList.contains("tone-danger"))).toEqual([true, false, true, true, true]);
});

it("opens the records in the Terminal with V", () => {
  const openInShell = vi.fn();
  page([request("w", "write", [[7, 8], { email: "a@b.c" }]), request("c", "create", [{ name: "x" }])], new Set(), vi.fn(), openInShell);
  fireEvent.keyDown(document.body, { key: "v" });
  expect(openInShell).toHaveBeenCalledWith(expect.objectContaining({ id: "w" }), "read res.partner 7,8 -f display_name,email");
  // A create has no records yet.
  fireEvent.keyDown(document.body, { key: "ArrowDown" });
  fireEvent.keyDown(document.body, { key: "v" });
  expect(openInShell).toHaveBeenCalledTimes(1);
});

it("links the environment and copies the request as a Terminal command", () => {
  const openEnvironment = vi.fn();
  const env = { id: "env-A", name: "A", url: "http://odoo.test" } as Props["envs"][number];
  render(<Harness envs={[env]} deciding={new Set()} decide={vi.fn()} decideMany={vi.fn()} openInShell={vi.fn()} openEnvironment={openEnvironment} pending={[request("w", "write", [[7], { name: "Ana" }])]} />);
  fireEvent.click(screen.getByRole("button", { name: /^A$/ }));
  expect(openEnvironment).toHaveBeenCalledWith("env-A");
  expect(screen.getByRole("button", { name: "Copy as Terminal command" })).toBeTruthy();
});

it("starts a Shift range at the target when the anchor has been decided", () => {
  const pending = ["a", "b", "c", "d", "e"].map((id) => ({ ...request(id, "write", [[1], { x: 1 }]), model: `m.${id}` }));
  const view = page(pending);
  fireEvent.click(screen.getAllByRole("option")[0], { detail: 1 });
  view.rerender(<Harness envs={[]} deciding={new Set()} decide={vi.fn()} decideMany={vi.fn()} openInShell={vi.fn()} pending={pending.slice(1)} />);
  fireEvent.click(screen.getAllByRole("option")[2], { shiftKey: true, detail: 1 });
  expect(selectedIds()).toEqual(["write on m.d, A"]);
  fireEvent.click(screen.getAllByRole("option")[3], { shiftKey: true, detail: 1 });
  expect(selectedIds()).toEqual(["write on m.d, A", "write on m.e, A"]);
});

it("unmarks the last marked request with Ctrl+click, as Space does", () => {
  const pending = ["a", "b"].map((id) => ({ ...request(id, "write", [[1], { x: 1 }]), model: `m.${id}` }));
  page(pending);
  fireEvent.click(screen.getAllByRole("option")[0], { detail: 1 });
  fireEvent.click(screen.getAllByRole("option")[1], { ctrlKey: true, detail: 1 });
  fireEvent.click(screen.getAllByRole("option")[0], { ctrlKey: true, detail: 1 });
  expect(selectedIds()).toEqual(["write on m.b, A"]);
  fireEvent.click(screen.getAllByRole("option")[1], { ctrlKey: true, detail: 1 });
  // Nothing marked: back to the plain cursor.
  expect(screen.queryByText(/selected/)).toBeNull();
});

it("builds the copied command only when copying", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  page([request("w", "write", [[7], { name: "Ana" }])]);
  const button = screen.getByRole("button", { name: "Copy as Terminal command" });
  expect(button.getAttribute("data-tip")).toBeNull();
  fireEvent.click(button);
  await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith('write res.partner 7 {"name":"Ana"}'));
});

it("creates a field rule from a write, with its effective context, and approves the request too", async () => {
  const { ipc } = await import("./ipc");
  const save = vi.spyOn(ipc, "saveRule").mockResolvedValue({} as never);
  const decide = vi.fn();
  const write = { ...request("w", "write", [[1, 2], { name: "x", email: "y" }]), context: { lang: "es_ES" } };
  page([write, request("u", "unlink", [[3]])], new Set(), decide);
  fireEvent.click(screen.getByRole("button", { name: "Create rule" }));
  expect((screen.getByRole("textbox", { name: /^Fields/ }) as HTMLInputElement).value).toBe("name, email");
  fireEvent.click(screen.getByRole("button", { name: "Create rule" }));
  await vi.waitFor(() => expect(decide).toHaveBeenCalledWith(write, true));
  expect(save).toHaveBeenCalledWith({
    environment_id: "env-A", model: "res.partner", method: "write", kind: "fields", fields: ["name", "email"], values: undefined,
    context: { lang: "es_ES" }, max_records: 2, expires_at: null,
  });
  // unlink always asks: no rule for it.
  fireEvent.click(screen.getAllByRole("option")[1], { detail: 1 });
  expect(screen.queryByRole("button", { name: "Create rule" })).toBeNull();
});
