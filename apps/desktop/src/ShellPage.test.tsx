import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ShellPage from "./ShellPage";
import App from "./App";
import { ipc, type Environment } from "./ipc";

vi.mock("./ipc", () => ({ ipc: {
  shellHistory: vi.fn(), shellSessions: vi.fn(async () => null), saveShellSessions: vi.fn(async () => undefined), shellHistoryAdd: vi.fn(), check: vi.fn(), shell: vi.fn(), shellExport: vi.fn(), openUrl: vi.fn(),
  status: vi.fn(), environments: vi.fn(), approvals: vi.fn(), clients: vi.fn(), serviceStatus: vi.fn(), lock: vi.fn(), unlock: vi.fn(),
} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
vi.mock("./WindowTitleBar", () => ({ WindowTitleBar: () => null }));
const envs: Environment[] = ["A", "B"].map((name) => ({
  id: name, name, url: `https://${name}.test`, database: name, login: "fixture",
  protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true, tags: [],
}));
beforeEach(() => {
  vi.resetAllMocks();
  Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: vi.fn() });
  vi.mocked(ipc.shellHistory).mockResolvedValue([]);
  vi.mocked(ipc.shellHistoryAdd).mockResolvedValue(undefined);
  vi.mocked(ipc.check).mockImplementation(async (id) => ({ environment_id: id, status: "ok", latency_ms: 1 }));
  vi.mocked(ipc.shell).mockResolvedValue({ data: [], write: false, upstream_ms: 1, upstream_bytes: 2 });
});
afterEach(() => { cleanup(); });
const command = (line: string) => {
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.change(input, { target: { value: line } });
  fireEvent.keyDown(input, { key: "Enter" });
};
const panel = () => screen.getByRole("tabpanel");
/** Like the core, require confirmation for calls outside the read catalog. */
const requireConfirmation = (reads: string[]) => vi.mocked(ipc.shell).mockImplementation(async (call) => {
  if (!call.confirmed && call.model !== "ir.model" && !reads.includes(call.method)) throw { code: "confirmation_required", message: "write requires confirmation", outcome: "not_sent" };
  return { data: true, write: !reads.includes(call.method), upstream_ms: 1, upstream_bytes: 2 };
});

it("keeps each tab's context, output and unfinished input when switching tabs and pages", async () => {
  const { rerender } = render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalled());
  command('\\context {"lang":"es_ES"}');
  const firstTab = screen.getByRole("tab");
  fireEvent.change(screen.getByRole("textbox", { name: "Command" }), { target: { value: "read res.partner 1" } });
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLInputElement).value).toBe("");
  command("\\context");
  expect(within(panel()).getByText("{}", { exact: true })).toBeTruthy();
  expect(within(panel()).queryByText("Session context set", { exact: false })).toBeNull();
  fireEvent.click(firstTab);
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLInputElement).value).toBe("read res.partner 1");
  rerender(<ShellPage envs={envs} visible={false} />);
  rerender(<ShellPage envs={envs} visible />);
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLInputElement).value).toBe("read res.partner 1");
  command("\\context");
  expect(within(panel()).getByText('{"lang":"es_ES"}', { exact: true })).toBeTruthy();
  expect(screen.getAllByRole("tab")).toHaveLength(2);
});

it("keeps a running call in its original tab while another session connects elsewhere", async () => {
  let finish!: (result: Awaited<ReturnType<typeof ipc.shell>>) => void;
  vi.mocked(ipc.shell).mockImplementation((call) => call.model === "ir.model"
    ? Promise.resolve({ data: [], write: false, upstream_ms: 1, upstream_bytes: 2 })
    : new Promise((resolve) => { finish = resolve; }));
  render(<ShellPage envs={envs} />);
  command("\\c A");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
  const firstTab = screen.getByRole("tab");
  command("read res.partner 1");
  const close = screen.getByRole("button", { name: /Close session/ }) as HTMLButtonElement;
  expect(close.disabled).toBe(true);
  fireEvent(firstTab, new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.getByRole("tab")).toBe(firstTab);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  command("\\c B");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to B/)).toBeTruthy());
  finish({ data: "result from A", write: false, upstream_ms: 1, upstream_bytes: 2 });
  await waitFor(() => expect(close.disabled).toBe(false));
  expect(within(panel()).queryByText("result from A")).toBeNull();
  fireEvent.click(firstTab);
  expect(within(panel()).getByText("result from A")).toBeTruthy();
  expect(within(panel()).queryByText(/^Connected to B/)).toBeNull();
});

it("cancels a pending confirmation instead of sending it to another environment", async () => {
  requireConfirmation([]);
  const { rerender } = render(<ShellPage envs={envs} />);
  command("\\c A");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
  command("write res.partner [1] {'name': 'Ada'}");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  rerender(<ShellPage envs={envs} connectTo="B" />);
  await waitFor(() => expect(within(panel()).getByText(/^Connected to B/)).toBeTruthy());
  expect(within(panel()).getByText("Cancelled. Nothing was sent.")).toBeTruthy();
  rerender(<ShellPage envs={envs} connectTo={null} />);
  command("y");
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "write" && call.confirmed)).toBe(false);
});

it("asks for confirmation only when the core answers confirmation_required", async () => {
  requireConfirmation(["check_access_rights"]);
  render(<ShellPage envs={envs} />);
  command("\\c A");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
  const sent = () => vi.mocked(ipc.shell).mock.calls.map(([call]) => call).filter((call) => call.model !== "ir.model");
  command("call res.partner check_access_rights ['read']");
  await waitFor(() => expect(sent()).toHaveLength(1));
  expect(screen.queryByText("Send to A? [y/N]")).toBeNull();
  command("call res.partner action_custom [[1]]");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  expect(sent()[1]).toMatchObject({ method: "action_custom", confirmed: false });
  command("y");
  await waitFor(() => expect(sent()).toHaveLength(3));
  expect(sent()[2]).toMatchObject({ environment_id: "A", method: "action_custom", confirmed: true });
});

it("supports keyboard navigation and always keeps one session after closing tabs", () => {
  render(<ShellPage envs={envs} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  const tabs = screen.getAllByRole("tab");
  fireEvent.keyDown(tabs[1], { key: "ArrowLeft" });
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  fireEvent.click(screen.getAllByRole("button", { name: /Close session/ })[0]);
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: /Close session/ }));
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  expect(screen.getByRole("textbox", { name: "Command" })).toBeTruthy();
});

it("drops all sessions and their output when the vault is locked and unlocked again", async () => {
  let status = "unlocked";
  vi.mocked(ipc.status).mockImplementation(async () => status);
  vi.mocked(ipc.environments).mockResolvedValue(envs);
  vi.mocked(ipc.approvals).mockResolvedValue([]);
  vi.mocked(ipc.clients).mockResolvedValue([]);
  vi.mocked(ipc.serviceStatus).mockResolvedValue(null as never);
  vi.mocked(ipc.lock).mockImplementation(async () => { status = "locked"; });
  vi.mocked(ipc.unlock).mockImplementation(async () => { status = "unlocked"; });
  localStorage.setItem("odx.page", "shell");
  try {
    render(<App />);
    await screen.findByRole("textbox", { name: "Command" });
    command("\\c A");
    await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
    command("\\context {\"lang\":\"es_ES\"}");
    fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
    expect(screen.getAllByRole("tab")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Lock vault" }));
    const password = await screen.findByLabelText(/^Master password/);
    expect(screen.queryByRole("tab")).toBeNull();
    fireEvent.change(password, { target: { value: "fixture-password" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await screen.findByRole("textbox", { name: "Command" });
    expect(screen.getAllByRole("tab")).toHaveLength(1);
    expect(screen.queryByText(/Connected to A|Session context set/)).toBeNull();
    expect(screen.getByRole("tab").textContent).toBe("Terminal");
    command("\\context");
    expect(within(panel()).getByText("{}", { exact: true })).toBeTruthy();
  } finally { localStorage.removeItem("odx.page"); }
});

it("uses distinct tab targets and switches to exactly one panel on click", () => {
  render(<ShellPage envs={envs} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  const tabs = screen.getAllByRole("tab");
  expect(tabs[0].id).not.toBe(tabs[1].id);
  expect(tabs[0].getAttribute("aria-controls")).not.toBe(tabs[1].getAttribute("aria-controls"));
  fireEvent.click(tabs[0]);
  expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
  expect(panel().id).toBe(tabs[0].getAttribute("aria-controls"));
  fireEvent.click(tabs[1]);
  expect(screen.getAllByRole("tabpanel")).toHaveLength(1);
  expect(panel().id).toBe(tabs[1].getAttribute("aria-controls"));
});

it("renames tabs with a double click and cancels with Escape", () => {
  render(<ShellPage envs={envs} />);
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Terminal" }));
  let name = screen.getByRole("textbox", { name: "Session name" });
  expect((name as HTMLInputElement).selectionStart).toBe(0);
  expect((name as HTMLInputElement).selectionEnd).toBe((name as HTMLInputElement).value.length);
  fireEvent.change(name, { target: { value: "  Ventas  " } });
  fireEvent.keyDown(name, { key: "Enter" });
  expect(screen.getByRole("tab", { name: "Ventas" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  fireEvent.click(screen.getByRole("tab", { name: "Ventas" }));
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Ventas" }));
  name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "Discard" } });
  fireEvent.keyDown(name, { key: "Escape" });
  expect(screen.getByRole("tab", { name: "Ventas" })).toBeTruthy();
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Ventas" }));
  name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "Contabilidad" } });
  fireEvent.blur(name);
  expect(screen.getByRole("tab", { name: "Contabilidad" })).toBeTruthy();
});

it("closes tabs with the middle mouse button and ignores right clicks", () => {
  render(<ShellPage envs={envs} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  const first = screen.getAllByRole("tab", { name: "Terminal" })[0];
  fireEvent(first, new MouseEvent("auxclick", { button: 2, bubbles: true, cancelable: true }));
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  fireEvent(first, new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.getAllByRole("tab", { name: "Terminal" })).toHaveLength(1);
  expect(screen.getByRole("tab", { name: "Terminal" })).not.toBe(first);
  const last = screen.getByRole("tab");
  fireEvent(last, new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  expect(screen.getByRole("tab")).not.toBe(last);
});

it("preserves the tab label width and icon while editing its name", () => {
  render(<ShellPage envs={envs} />);
  const tab = screen.getByRole("tab");
  vi.spyOn(tab, "getBoundingClientRect").mockReturnValue({
    width: 148, height: 34, x: 0, y: 0, top: 0, left: 0, right: 148, bottom: 34, toJSON: () => ({}),
  });
  const icon = tab.querySelector("svg path")!.getAttribute("d");
  fireEvent.doubleClick(tab);
  const input = screen.getByRole("textbox", { name: "Session name" });
  const editor = input.parentElement!;
  expect(editor.style.width).toBe("148px");
  expect(editor.querySelector("svg path")!.getAttribute("d")).toBe(icon);
  fireEvent.change(input, { target: { value: "A much longer name" } });
  expect(editor.style.width).toBe("148px");
  expect(editor.querySelector("svg")).not.toBeNull();
});

it("numbers history the same in the current tab and in a new tab loaded from the backend", async () => {
  // Like shell_history_add, skip only consecutive duplicates in the same environment.
  const stored: { at: string; environment: string | null; command: string }[] = [];
  vi.mocked(ipc.shellHistory).mockImplementation(async () => [...stored]);
  vi.mocked(ipc.shellHistoryAdd).mockImplementation(async (cmd, environment) => {
    const last = stored[stored.length - 1];
    if (last?.command !== cmd || last.environment !== (environment ?? null)) stored.push({ at: new Date().toISOString(), environment: environment ?? null, command: cmd });
  });
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalled());
  for (const line of ["\\context", "\\timing", "\\context", "\\context"]) command(line);
  await waitFor(() => expect(stored.map((h) => h.command)).toEqual(["\\context", "\\timing", "\\context"]));
  const input = () => (screen.getByRole("textbox", { name: "Command" }) as HTMLInputElement).value;
  command("!2");
  expect(input()).toBe("\\timing");
  command("!3");
  expect(input()).toBe("\\context");
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalledTimes(2));
  await waitFor(() => { command("!2"); expect(input()).toBe("\\timing"); });
  command("!3");
  expect(input()).toBe("\\context");
});

const answer = (data: (call: Parameters<typeof ipc.shell>[0]) => unknown) => vi.mocked(ipc.shell).mockImplementation(async (call) => ({ data: call.model === "ir.model" ? [] : data(call), write: false, upstream_ms: 3, upstream_bytes: 4 }));
const connectA = async () => {
  command("\\c A");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
};

it("answers uhg with read calls only, looking the user up by the environment login", async () => {
  answer((call) => {
    if (call.model === "ir.model.data") return [{ model: "res.groups", res_id: 4 }];
    if (call.method === "fields_get") return { groups_id: { type: "many2many" } };
    if (call.method === "search_count") return 1;
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("uhg base.group_system");
  await waitFor(() => expect(within(panel()).getByText("yes")).toBeTruthy());
  const calls = vi.mocked(ipc.shell).mock.calls.map(([c]) => c).filter((c) => c.model !== "ir.model");
  expect(calls.every((c) => !c.confirmed)).toBe(true);
  expect(calls.at(-1)).toMatchObject({ model: "res.users", method: "search_count", args: [[["login", "=", "fixture"], ["groups_id", "in", [4]]]] });
  expect(within(panel()).getByText(/9 ms/)).toBeTruthy();
});

it("shows metadata with XML IDs and reports a missing group as an error", async () => {
  answer((call) => {
    if (call.method === "read") return [{ id: 7, create_uid: [2, "Admin"], create_date: "2026-01-01 10:00:00", write_uid: [2, "Admin"], write_date: "2026-02-01 10:00:00" }];
    if (call.method === "get_external_id") return { 7: "base.partner_admin" };
    return [];
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("metadata res.partner 7");
  await waitFor(() => expect(within(panel()).getByText("base.partner_admin")).toBeTruthy());
  command("uhg base.missing");
  await waitFor(() => expect(within(panel()).getByText("No group with XML ID base.missing")).toBeTruthy());
});

it("renders read_group with groupby columns and the count, sorts by header and toggles each table", async () => {
  answer(() => [
    { country_id: [1, "Spain"], __count: 2, __domain: [] },
    { country_id: false, __count: 9, __domain: [] },
    { country_id: [2, "France"], __count: 5, __domain: [] },
  ]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("read_group res.partner country_id");
  await waitFor(() => expect(within(panel()).getByText("Spain (1)")).toBeTruthy());
  const table = within(panel()).getByRole("table");
  expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["country_id", "count"]);
  const firstColumn = () => within(table).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent);
  fireEvent.click(within(table).getByRole("button", { name: "Sort by country_id" }));
  expect(firstColumn()).toEqual(["France (2)", "Spain (1)", "false"]);
  fireEvent.click(within(table).getByRole("button", { name: "Sort by country_id" }));
  expect(firstColumn()).toEqual(["Spain (1)", "France (2)", "false"]);
  fireEvent.click(within(panel()).getByRole("button", { name: "Expand table" }));
  expect(within(panel()).getAllByRole("table")).toHaveLength(3);
  fireEvent.click(within(panel()).getByRole("button", { name: "Collapse table" }));
  expect(within(panel()).getAllByRole("table")).toHaveLength(1);
});

it("shows elapsed time while a call runs and drops a read discarded with Ctrl+C", async () => {
  let finish!: (result: Awaited<ReturnType<typeof ipc.shell>>) => void;
  vi.mocked(ipc.shell).mockImplementation((call) => call.model === "ir.model"
    ? Promise.resolve({ data: [], write: false, upstream_ms: 1, upstream_bytes: 2 })
    : new Promise((resolve) => { finish = resolve; }));
  render(<ShellPage envs={envs} />);
  await connectA();
  command("count res.partner");
  expect(within(panel()).getByText(/Ctrl\+C discards/)).toBeTruthy();
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.keyDown(input, { key: "c", ctrlKey: true });
  expect(within(panel()).getByText(/^Discarded\./)).toBeTruthy();
  expect(within(panel()).queryByText(/Ctrl\+C discards/)).toBeNull();
  finish({ data: 4242, write: false, upstream_ms: 1, upstream_bytes: 2 });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(within(panel()).queryByText("4242")).toBeNull();
});

it("follows ids and many2one cells as visible read commands and inspects long values", async () => {
  answer((call) => {
    if (call.method === "fields_get") return { country_id: { relation: "res.country" } };
    if (call.model === "res.country") return [{ id: 68, name: "Spain" }];
    return [{ id: 7, country_id: [68, "Spain"], comment: "x".repeat(120) }];
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f country_id,comment");
  fireEvent.click(await within(panel()).findByRole("button", { name: "Spain (68)" }));
  const typed = (line: string) => [...panel().querySelectorAll(".term-cmd")].some((l) => l.textContent?.includes(line));
  await waitFor(() => expect(typed("read res.country 68 -f *")).toBe(true));
  expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ model: "res.country", method: "read", args: [[68]], kwargs: {} });
  fireEvent.click(within(panel()).getAllByRole("button", { name: "7" })[0]);
  await waitFor(() => expect(typed("read res.partner 7 -f *")).toBe(true));
  fireEvent.click(within(panel()).getAllByRole("button", { name: /^x{80}…$/ })[0]);
  const dialog = within(panel()).getByRole("dialog", { name: "res.partner 7 · comment" });
  expect(dialog.textContent).toContain("x".repeat(120));
  fireEvent.keyDown(screen.getByRole("textbox", { name: "Command" }), { key: "Escape" });
  expect(within(panel()).queryByRole("dialog")).toBeNull();
});

it("inserts newlines with Shift+Enter, submits multi-line commands and flags syntax errors after a pause", async () => {
  answer(() => [{ id: 1, name: "Ada" }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: "search res.partner" } });
  fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
  expect(input.value).toBe("search res.partner");
  fireEvent.change(input, { target: { value: "search res.partner\n  -d [('name','=','Ada')]" } });
  expect(input.rows).toBe(2);
  fireEvent.keyDown(input, { key: "Enter" });
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "search_read", kwargs: { domain: [["name", "=", "Ada"]] } }));
  // Mid-value input (open quote or bracket) gets no hint.
  fireEvent.change(input, { target: { value: "search res.partner -d [('name','=','" } });
  await new Promise((resolve) => setTimeout(resolve, 900));
  expect(panel().querySelector(".term-hint")).toBeNull();
  fireEvent.change(input, { target: { value: "search res.partner -d {'name': 1}" } });
  await waitFor(() => expect(panel().querySelector(".term-hint")?.textContent).toMatch(/list/i), { timeout: 2000 });
  // A lone first word is left to the highlighter.
  fireEvent.change(input, { target: { value: "\\" } });
  await new Promise((resolve) => setTimeout(resolve, 900));
  expect(panel().querySelector(".term-hint")).toBeNull();
});

it("suggests the newest matching history command and accepts it with the right arrow", async () => {
  vi.mocked(ipc.shellHistory).mockResolvedValue([
    { at: "2026-10-19T10:00:00Z", command: "search res.partner -l 5" },
    { at: "2026-10-19T10:01:00Z", command: "search res.users -l 1" },
  ]);
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalled());
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: "search res." } });
  await waitFor(() => expect(panel().querySelector(".term-ghost")?.textContent).toBe("users -l 1"));
  input.setSelectionRange(input.value.length, input.value.length);
  fireEvent.keyDown(input, { key: "ArrowRight" });
  expect(input.value).toBe("search res.users -l 1");
  expect(panel().querySelector(".term-ghost")).toBeNull();
});

it("re-runs and folds commands from the output", async () => {
  answer(() => 42);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("count res.partner");
  await waitFor(() => expect(within(panel()).getByText("42")).toBeTruthy());
  // Hovering the result shows the command's actions: both live in one block.
  expect(within(panel()).getByText("42").closest(".term-group")?.contains(within(panel()).getAllByRole("button", { name: "Run again" }).at(-1)!)).toBe(true);
  fireEvent.click(within(panel()).getAllByRole("button", { name: "Fold output" }).at(-1)!);
  expect(within(panel()).queryByText("42")).toBeNull();
  expect(within(panel()).getByRole("button", { name: /lines? hidden/ })).toBeTruthy();
  fireEvent.click(within(panel()).getByRole("button", { name: "Unfold output" }));
  expect(within(panel()).getByText("42")).toBeTruthy();
  fireEvent.click(within(panel()).getAllByRole("button", { name: "Run again" }).at(-1)!);
  await waitFor(() => expect(within(panel()).getAllByText("42")).toHaveLength(2));
});

it("filters and reshapes the last table, saves ids as $name, pages back and runs aliases", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => call.method === "search_read"
    ? { data: [{ id: 3, name: "Ada", email: "ada@x" }, { id: 5, name: "Bob", email: "bob@x" }], write: false, upstream_ms: 1, upstream_bytes: 2 }
    : { data: true, write: false, upstream_ms: 1, upstream_bytes: 2 });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("\\grep ada");
  expect(within(panel()).getByText("No table on screen to filter")).toBeTruthy();
  command("\\alias pp search res.partner -f name,email");
  command("pp -o 2 -l 2");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0].kwargs).toMatchObject({ offset: 2, limit: 3, fields: ["id", "name", "email"] }));
  await waitFor(() => expect(within(panel()).getByText("2 rows", { exact: false })).toBeTruthy());
  command("\\grep ada");
  expect(within(panel()).getByText("(1 of 2 rows)")).toBeTruthy();
  command("\\set ada");
  command("\\cols email,id");
  const tables = within(panel()).getAllByRole("table");
  expect(within(tables.at(-1)!).getAllByRole("columnheader").map((th) => th.textContent)).toEqual(["email", "id"]);
  command("\\cols nope");
  expect(within(panel()).getByText(/Unknown columns: nope/)).toBeTruthy();
  command("\\prev");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0].kwargs).toMatchObject({ offset: 0 }));
  command("call res.partner check $ada");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "check", args: [3] }));
});

it("restores aliases from the vault even when no saved tab was used", async () => {
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "x", aliases: { pp: "search res.partner" }, tabs: [] }));
  render(<ShellPage envs={envs} />);
  await connectA();
  command("pp -l 1");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ model: "res.partner", method: "search_read" }));
  command("\\alias pq count res.partner");
  await waitFor(() => expect(JSON.parse(vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)?.[0] ?? "{}").aliases).toEqual({ pp: "search res.partner", pq: "count res.partner" }), { timeout: 2000 });
});

it("finds text in the output with Ctrl+F and zooms the font with Ctrl +/-", async () => {
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalled());
  command("\\l");
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.keyDown(input, { key: "f", ctrlKey: true });
  const find = within(panel()).getByRole("textbox", { name: "Search output" });
  fireEvent.change(find, { target: { value: "https://" } });
  await waitFor(() => expect(within(panel()).getByText("1 of 2")).toBeTruthy());
  fireEvent.keyDown(find, { key: "Enter" });
  expect(within(panel()).getByText("2 of 2")).toBeTruthy();
  fireEvent.keyDown(find, { key: "Escape" });
  expect(within(panel()).queryByRole("search")).toBeNull();
  const term = panel().querySelector(".term") as HTMLElement;
  const before = parseInt(term.style.fontSize);
  fireEvent.keyDown(input, { key: "+", ctrlKey: true });
  expect(parseInt(term.style.fontSize)).toBe(before + 1);
  fireEvent.keyDown(input, { key: "0", ctrlKey: true });
  expect(parseInt(term.style.fontSize)).toBe(12);
});

it("chains the previous result's ids with $_ and refuses it without a result", async () => {
  requireConfirmation(["search_read"]);
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.method === "search_read") return { data: [{ id: 3 }, { id: 5 }], write: false, upstream_ms: 1, upstream_bytes: 2 };
    if (!call.confirmed) throw { code: "confirmation_required", message: "", outcome: "not_sent" };
    return { data: true, write: true, upstream_ms: 1, upstream_bytes: 2 };
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("write res.partner $_ {'active': False}");
  expect(within(panel()).getByText("$_ needs a previous result with records")).toBeTruthy();
  command("search res.partner -f *");
  await waitFor(() => expect(within(panel()).getByText("2 rows", { exact: false })).toBeTruthy());
  expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0].kwargs).not.toHaveProperty("fields");
  command("write res.partner $_ {'active': False}");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "write", args: [[3, 5], { active: false }] }));
});

it("opens records in Odoo with Ctrl+click and saves tables to Downloads", async () => {
  vi.mocked(ipc.openUrl).mockResolvedValue(undefined);
  vi.mocked(ipc.shellExport).mockResolvedValue("/home/u/Downloads/odx-res.partner.csv");
  answer((call) => call.method === "fields_get" ? { country_id: { relation: "res.country" } } : [{ id: 7, name: "Ada, \"the\" first", country_id: [68, "Spain"] }, { id: 8, name: "=HYPERLINK(\"x\")", country_id: false }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f name,country_id");
  fireEvent.click(await within(panel()).findByRole("button", { name: "Spain (68)" }), { ctrlKey: true });
  await waitFor(() => expect(ipc.openUrl).toHaveBeenCalledWith("A", { model: "res.country", id: 68 }));
  fireEvent.click(within(panel()).getByRole("button", { name: "More formats" }));
  fireEvent.click(within(panel()).getByRole("menuitem", { name: "Save as CSV in Downloads" }));
  await waitFor(() => expect(within(panel()).getByText("Saved to /home/u/Downloads/odx-res.partner.csv")).toBeTruthy());
  const [name, content] = vi.mocked(ipc.shellExport).mock.calls[0];
  expect(name).toMatch(/^odx-res\.partner-\d{8}-\d{6}\.csv$/);
  // Values a spreadsheet would run as formulas get a leading apostrophe.
  expect(content).toBe('\uFEFFid,name,country_id\r\n7,"Ada, ""the"" first",Spain (68)\r\n8,"\'=HYPERLINK(""x"")",false');
});

it("shows Odoo's message and folds the rest of long errors", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.model === "ir.model") return { data: [], write: false, upstream_ms: 1, upstream_bytes: 2 };
    throw { code: "upstream.remote", detail: "ValidationError: Email is required\nline 2\nline 3", outcome: "failed" };
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("read res.partner 1");
  const summary = await within(panel()).findByText("Show 2 more lines");
  expect(summary.closest(".term-line")?.textContent).toContain("ValidationError: Email is required");
  expect(summary.closest("details")?.open).toBe(false);
});

it("copies tables as JSON and Markdown from the formats menu", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  answer(() => [{ id: 7, name: "Ada | Lovelace" }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner");
  const more = await within(panel()).findByRole("button", { name: "More formats" });
  fireEvent.click(more);
  // The editor takes focus on mouseup; the menu must survive it.
  fireEvent.mouseUp(within(panel()).getByRole("menuitem", { name: "Copy as JSON" }));
  fireEvent.click(within(panel()).getByRole("menuitem", { name: "Copy as JSON" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(JSON.stringify([{ id: 7, name: "Ada | Lovelace" }], null, 2)));
  fireEvent.click(more);
  fireEvent.click(within(panel()).getByRole("menuitem", { name: "Copy as Markdown" }));
  await waitFor(() => expect(writeText).toHaveBeenLastCalledWith("| id | name |\n| --- | --- |\n| 7 | Ada \\| Lovelace |"));
  fireEvent.click(more);
  fireEvent.mouseDown(document.body);
  expect(within(panel()).queryByRole("menu")).toBeNull();
});

it("navigates tables with the keyboard and keeps their actions in the footer", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  answer((call) => call.method === "read" ? [{ id: 8, name: "Grace" }] : [{ id: 7, name: "Ada" }, { id: 8, name: "Grace" }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner");
  await within(panel()).findByText("Grace");
  const input = screen.getByRole("textbox", { name: "Command" });
  expect(panel().querySelector(".term-foot")?.contains(within(panel()).getByRole("button", { name: "Expand table" }))).toBe(true);
  fireEvent.keyDown(input, { key: "ArrowUp", ctrlKey: true });
  const table = panel().querySelector<HTMLElement>(".term-scroll")!;
  expect(document.activeElement).toBe(table);
  const active = () => table.querySelector("[data-active]")?.textContent;
  expect(active()).toBe("7");
  fireEvent.keyDown(table, { key: "ArrowDown" });
  fireEvent.keyDown(table, { key: "ArrowRight" });
  expect(active()).toBe("Grace");
  fireEvent.keyDown(table, { key: "c" });
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("Grace"));
  fireEvent.keyDown(table, { key: "Home" });
  expect(active()).toBe("8");
  fireEvent.keyDown(table, { key: "Enter" });
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "read", args: [[8]] }));
  const first = panel().querySelector<HTMLElement>(".term-scroll")!;
  first.focus();
  fireEvent.keyDown(first, { key: "Escape" });
  expect(document.activeElement).toBe(input);
});

const savedTab = (id: string, env: string | null, line: string) => ({
  id, name: "", defaultName: true,
  session: { entries: [{ id: Number(id.slice(1)) * 10, kind: "input", prompt: "odoo>", text: line, command: true }, { id: Number(id.slice(1)) * 10 + 1, kind: "text", text: `out of ${line}` }],
    env: env ? { id: env, name: env } : undefined, context: {}, expanded: false, timing: true },
});

it("restores saved tabs with their output and reconnects each one when it is first shown", async () => {
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t2", tabs: [savedTab("t1", "A", "count res.partner"), savedTab("t2", "B", "whoami")] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(2));
  expect(within(panel()).getByText("out of whoami")).toBeTruthy();
  expect(within(panel()).getByText(/Session restored/)).toBeTruthy();
  await waitFor(() => expect(within(panel()).getByText(/^Connected to B/)).toBeTruthy());
  expect(ipc.check).toHaveBeenCalledTimes(1);
  expect(ipc.check).toHaveBeenCalledWith("B");
  fireEvent.click(screen.getAllByRole("tab")[0]);
  expect(within(panel()).getByText("out of count res.partner")).toBeTruthy();
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
  expect(ipc.check).toHaveBeenCalledTimes(2);
  // The restore note and the automatic reconnection are shown but not saved, so restarts do not pile them up.
  vi.mocked(ipc.saveShellSessions).mockClear();
  await waitFor(() => expect(ipc.saveShellSessions).toHaveBeenCalled(), { timeout: 3000 });
  const saved = vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)![0];
  expect(saved).toContain("out of whoami");
  expect(saved).not.toMatch(/Session restored|Connected to|\\\\c [AB]/);
});

it("does not announce a restored session when only untouched tabs were saved", async () => {
  const untouched = { id: "t9", name: "", defaultName: true, session: { entries: [{ id: 90, kind: "info", text: "", translationKey: "shell.initial" }], context: {}, expanded: false, timing: true } };
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t9", tabs: [untouched, { ...untouched, id: "t8" }] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellSessions).toHaveBeenCalled());
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  expect(within(panel()).queryByText(/Session restored/)).toBeNull();
  // A saved session with work still restores, without its untouched tabs.
  cleanup();
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t9", tabs: [untouched, savedTab("t2", null, "whoami")] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(within(panel()).getByText(/Session restored/)).toBeTruthy());
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  expect(within(panel()).getByText("out of whoami")).toBeTruthy();
});

it("reports a restored environment that no longer exists and leaves the tab disconnected", async () => {
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t1", tabs: [savedTab("t1", "Gone", "whoami")] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(within(panel()).getByText(/Gone no longer exists/)).toBeTruthy());
  expect(ipc.check).not.toHaveBeenCalled();
  expect(panel().querySelector(".term-input .term-prompt")?.textContent).toBe("odoo>");
});

it("saves sessions to the vault only after loading them, without pending confirmations", async () => {
  let load!: (data: string | null) => void;
  vi.mocked(ipc.shellSessions).mockReturnValue(new Promise((resolve) => { load = resolve; }));
  requireConfirmation(["search_read"]);
  render(<ShellPage envs={envs} />);
  command("\\c A");
  await waitFor(() => expect(within(panel()).getByText(/^Connected to A/)).toBeTruthy());
  await new Promise((resolve) => setTimeout(resolve, 1000));
  expect(ipc.saveShellSessions).not.toHaveBeenCalled();
  load(null);
  command("unlink res.partner 5");
  await waitFor(() => expect(ipc.saveShellSessions).toHaveBeenCalled(), { timeout: 3000 });
  const saved = JSON.parse(vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)![0]);
  expect(saved.tabs[0].session.env).toEqual({ id: "A", name: "A" });
  expect(JSON.stringify(saved)).toContain("unlink res.partner 5");
  expect(JSON.stringify(saved)).not.toContain("confirming");
});

it("warns when $_ holds ids of another model and does not flag $_ as a syntax error", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.model === "ir.model.data") return { data: [{ id: 2, module: "base", name: "main_company", model: "res.company", res_id: 1 }], write: false, upstream_ms: 1, upstream_bytes: 2 };
    if (call.model === "ir.model") return { data: [], write: false, upstream_ms: 1, upstream_bytes: 2 };
    if (!call.confirmed) throw { code: "confirmation_required", message: "", outcome: "not_sent" };
    return { data: true, write: true, upstream_ms: 1, upstream_bytes: 2 };
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("ref base.main_company");
  await within(panel()).findByText("main_company");
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.change(input, { target: { value: "write res.company $_ {'name': 'X'}" } });
  await new Promise((resolve) => setTimeout(resolve, 900));
  expect(panel().querySelector(".term-hint")).toBeNull();
  fireEvent.keyDown(input, { key: "Enter" });
  await waitFor(() => expect(within(panel()).getByText(/\$_ holds ir\.model\.data ids, not res\.company ids/)).toBeTruthy());
  expect(within(panel()).getByText(/^write res\.company \[\[2\]/)).toBeTruthy();
});
