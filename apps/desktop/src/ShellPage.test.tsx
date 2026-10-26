import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ShellPage, { HINT_DELAY_MS, SAVE_DELAY_MS } from "./ShellPage";
import App from "./App";
import { ipc, type AgentRequest, type Environment } from "./ipc";
import { captureWindowState } from "./windowState";
import { envNamed, shellResult } from "./test/fixtures";
import { command, newSession, stubScroll } from "./test/terminal";

vi.mock("./ipc", async () => (await import("./test/terminal")).terminalIpc());
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
vi.mock("./WindowTitleBar", () => ({ WindowTitleBar: () => null }));
const envs = ["A", "B"].map((name) => envNamed(name, { write_mode: "approval" }));
beforeEach(() => {
  vi.resetAllMocks();
  stubScroll();
  vi.mocked(ipc.shellHistory).mockResolvedValue([]);
  vi.mocked(ipc.shellHistoryAdd).mockResolvedValue(undefined);
  vi.mocked(ipc.check).mockImplementation(async (id) => ({ environment_id: id, status: "ok", latency_ms: 1 }));
  vi.mocked(ipc.shell).mockResolvedValue(shellResult([]));
});
afterEach(() => { cleanup(); vi.useRealTimers(); });
const panel = () => screen.getByRole("tabpanel");
/** Like the core, require confirmation for calls outside the read catalog. */
const requireConfirmation = (reads: string[]) => vi.mocked(ipc.shell).mockImplementation(async (call) => {
  if (!call.confirmed && call.model !== "ir.model" && !reads.includes(call.method)) throw { code: "confirmation_required", message: "write requires confirmation", outcome: "not_sent" };
  return shellResult(true, !reads.includes(call.method));
});

/** The active session is connected: its mode chip shows next to `database` (fixtures name databases like their env). */
const connectedTo = (database: string) => waitFor(() => {
  expect(panel().querySelector(".term-status .term-mode")).toBeTruthy();
  expect(panel().querySelector(".term-status .term-db")?.textContent).toBe(database);
});
const openSession = async (env = "A", database = env) => {
  newSession(env);
  await connectedTo(database);
};
it("keeps each tab's context, output and unfinished input when switching tabs and pages", async () => {
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  command('\\context {"lang":"es_ES"}');
  const firstTab = screen.getByRole("tab");
  fireEvent.change(screen.getByRole("textbox", { name: "Command" }), { target: { value: "read res.partner 1" } });
  await openSession("B");
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
    ? Promise.resolve(shellResult([]))
    : new Promise((resolve) => { finish = resolve; }));
  render(<ShellPage envs={envs} />);
  await openSession("A");
  const firstTab = screen.getByRole("tab");
  command("read res.partner 1");
  const close = screen.getByRole("button", { name: /Close session/ }) as HTMLButtonElement;
  expect(close.disabled).toBe(true);
  expect(firstTab.querySelector(".spin")).not.toBeNull();
  fireEvent(firstTab, new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.getByRole("tab")).toBe(firstTab);
  await openSession("B");
  finish(shellResult("result from A"));
  await waitFor(() => expect(close.disabled).toBe(false));
  expect(within(panel()).queryByText("result from A")).toBeNull();
  // Finished behind another tab: the tab is marked until shown.
  expect(firstTab.querySelector(".spin")).toBeNull();
  expect(within(firstTab).getByRole("status", { name: "Finished while you were away" })).toBeTruthy();
  fireEvent.click(firstTab);
  expect(within(firstTab).queryByRole("status")).toBeNull();
  expect(within(panel()).getByText("result from A")).toBeTruthy();
  expect(panel().querySelector(".term-db")?.textContent).toBe("A");
});

it("opens another environment in a new tab, leaving a pending confirmation where it was", async () => {
  requireConfirmation([]);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  command("write res.partner [1] {'name': 'Ada'}");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  rerender(<ShellPage envs={envs} connectTo="B" />);
  await connectedTo("B");
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  rerender(<ShellPage envs={envs} connectTo={null} />);
  fireEvent.click(screen.getAllByRole("tab")[0]);
  expect(within(panel()).getByText("Send to A? [y/N]")).toBeTruthy();
});

it("reuses a tab already connected to the requested environment", async () => {
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  rerender(<ShellPage envs={envs} connectTo="B" />);
  await connectedTo("B");
  rerender(<ShellPage envs={envs} connectTo={null} />);
  vi.mocked(ipc.check).mockClear();
  rerender(<ShellPage envs={envs} connectTo="A" command="read res.partner 7" />);
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "read" && call.environment_id === "A")).toBe(true));
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(screen.getAllByRole("tab")[0].getAttribute("aria-selected")).toBe("true");
  expect(ipc.check).not.toHaveBeenCalled();
});

it("opens one session for a requested environment when none is open", async () => {
  render(<ShellPage envs={envs} connectTo="B" />);
  await connectedTo("B");
  expect(screen.getAllByRole("tab")).toHaveLength(1);
});

it("runs a requested command once connected to the requested environment", async () => {
  render(<ShellPage envs={envs} connectTo="B" command="read res.partner 7,8" />);
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "read" && call.model === "res.partner")).toBe(true));
  const read = vi.mocked(ipc.shell).mock.calls.find(([call]) => call.method === "read")![0];
  expect(read).toMatchObject({ environment_id: envs[1].id, args: [[7, 8]] });
});

it("asks for confirmation only when the core answers confirmation_required", async () => {
  requireConfirmation(["check_access_rights"]);
  render(<ShellPage envs={envs} />);
  await openSession("A");
  // Model and field lookups for completion and headers are not part of the conversation.
  const sent = () => vi.mocked(ipc.shell).mock.calls.map(([call]) => call).filter((call) => call.model !== "ir.model" && call.method !== "fields_get");
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

it("says the environment is read-only instead of asking to confirm a write it would refuse", async () => {
  requireConfirmation([]);
  const blocked = [{ ...envs[0], write_mode: "read_only" as const }, { ...envs[1], write_mode: "temporary" as const, write_until: "2000-01-01T00:00:00Z" }];
  const { rerender } = render(<ShellPage envs={blocked} />);
  await openSession("A");
  const sent = () => vi.mocked(ipc.shell).mock.calls.map(([call]) => call).filter((call) => call.model !== "ir.model");
  command("call res.partner action_custom [[1]]");
  expect(await within(panel()).findByText(/A is read-only: this write was not sent/)).toBeTruthy();
  expect(screen.queryByText("Send to A? [y/N]")).toBeNull();
  expect(sent()).toEqual([expect.objectContaining({ confirmed: false })]);
  await openSession("B");
  command("call res.partner action_custom [[1]]");
  expect(await within(panel()).findByText(/temporary write window for B has expired/)).toBeTruthy();
  // A window that closes while the prompt is open blocks the answer too; the core stays the final check.
  rerender(<ShellPage envs={[blocked[0], { ...blocked[1], write_until: "2999-01-01T00:00:00Z" }]} />);
  command("call res.partner action_custom [[1]]");
  expect(await screen.findByText("Send to B? [y/N]")).toBeTruthy();
  rerender(<ShellPage envs={blocked} />);
  command("y");
  expect(await within(panel()).findAllByText(/temporary write window for B has expired/)).toHaveLength(2);
  expect(sent().filter((call) => call.confirmed)).toHaveLength(0);
});

it("supports keyboard navigation and offers a new session after closing the last one", async () => {
  render(<ShellPage envs={envs} />);
  await openSession("A");
  await openSession("B");
  const tabs = screen.getAllByRole("tab");
  fireEvent.keyDown(tabs[1], { key: "ArrowLeft" });
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  fireEvent.click(screen.getAllByRole("button", { name: /Close session/ })[0]);
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: /Close session/ }));
  expect(screen.queryByRole("tab")).toBeNull();
  expect(screen.getByRole("heading", { name: "No sessions" })).toBeTruthy();
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
    await screen.findByRole("heading", { name: "No sessions" });
    await openSession("A");
    command("\\context {\"lang\":\"es_ES\"}");
    newSession("B");
    expect(screen.getAllByRole("tab")).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "Lock vault" }));
    const password = await screen.findByLabelText(/^Master password/);
    expect(screen.queryByRole("tab")).toBeNull();
    fireEvent.change(password, { target: { value: "fixture-password" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    await screen.findByRole("heading", { name: "No sessions" });
    expect(screen.queryByRole("tab")).toBeNull();
    expect(screen.queryByText(/Session context set/)).toBeNull();
    await openSession("A");
    command("\\context");
    expect(within(panel()).getByText("{}", { exact: true })).toBeTruthy();
  } finally { localStorage.removeItem("odx.page"); }
});

it("saves pending session changes before locking the vault", async () => {
  let status = "unlocked";
  vi.mocked(ipc.status).mockImplementation(async () => status);
  vi.mocked(ipc.environments).mockResolvedValue(envs);
  vi.mocked(ipc.approvals).mockResolvedValue([]);
  vi.mocked(ipc.clients).mockResolvedValue([]);
  vi.mocked(ipc.serviceStatus).mockResolvedValue(null as never);
  vi.mocked(ipc.lock).mockImplementation(async () => { status = "locked"; });
  vi.mocked(ipc.saveShellSessions).mockImplementation(async () => { if (status === "locked") throw { code: "locked" }; });
  localStorage.setItem("odx.page", "shell");
  try {
    render(<App />);
    await screen.findByRole("heading", { name: "No sessions" });
    await openSession("A");
    command("\\context {\"lang\":\"es_ES\"}");
    // Lock within the save delay: the change is saved first, while the vault still accepts it.
    fireEvent.click(screen.getByRole("button", { name: "Lock vault" }));
    await screen.findByLabelText(/^Master password/);
    const saves = vi.mocked(ipc.saveShellSessions).mock;
    expect(saves.calls.at(-1)?.[0]).toContain("es_ES");
    expect(saves.invocationCallOrder.at(-1)).toBeLessThan(vi.mocked(ipc.lock).mock.invocationCallOrder[0]);
  } finally { localStorage.removeItem("odx.page"); }
});

it("does not save every tab just because the active tab changed", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellSessions).toHaveBeenCalled());
  // Both connected first: their status lines are changes that get saved too.
  await openSession("A");
  await openSession("B");
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  await waitFor(() => expect(ipc.saveShellSessions).toHaveBeenCalled());
  vi.mocked(ipc.saveShellSessions).mockClear();
  fireEvent.click(screen.getAllByRole("tab")[0]);
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  expect(ipc.saveShellSessions).not.toHaveBeenCalled();
  // The next save records it.
  command("\\context");
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  await waitFor(() => expect(ipc.saveShellSessions).toHaveBeenCalled());
  const saved = JSON.parse(vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)![0]);
  expect(saved.activeId).toBe(saved.tabs[0].id);
});

it("uses distinct tab targets and switches to exactly one panel on click", () => {
  render(<ShellPage envs={envs} />);
  newSession("A");
  newSession("B");
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
  newSession("A");
  fireEvent.doubleClick(screen.getByRole("tab", { name: "A" }));
  let name = screen.getByRole("textbox", { name: "Session name" });
  expect((name as HTMLInputElement).selectionStart).toBe(0);
  expect((name as HTMLInputElement).selectionEnd).toBe((name as HTMLInputElement).value.length);
  fireEvent.change(name, { target: { value: "  Ventas  " } });
  fireEvent.keyDown(name, { key: "Enter" });
  expect(screen.getByRole("tab", { name: "Ventas A" })).toBeTruthy();
  newSession("B");
  fireEvent.click(screen.getByRole("tab", { name: "Ventas A" }));
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Ventas A" }));
  name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "Discard" } });
  fireEvent.keyDown(name, { key: "Escape" });
  expect(screen.getByRole("tab", { name: "Ventas A" })).toBeTruthy();
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Ventas A" }));
  name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "Contabilidad" } });
  fireEvent.blur(name);
  expect(screen.getByRole("tab", { name: "Contabilidad A" })).toBeTruthy();
  // An empty name follows the environment again.
  fireEvent.doubleClick(screen.getByRole("tab", { name: "Contabilidad A" }));
  name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "" } });
  fireEvent.keyDown(name, { key: "Enter" });
  expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
});

it("closes tabs with the middle mouse button and ignores right clicks", async () => {
  render(<ShellPage envs={envs} />);
  await openSession("A");
  await openSession("A");
  const first = screen.getAllByRole("tab", { name: "A" })[0];
  fireEvent(first, new MouseEvent("auxclick", { button: 2, bubbles: true, cancelable: true }));
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  fireEvent(first, new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.getAllByRole("tab", { name: "A" })).toHaveLength(1);
  expect(screen.getByRole("tab", { name: "A" })).not.toBe(first);
  fireEvent(screen.getByRole("tab"), new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
  expect(screen.queryByRole("tab")).toBeNull();
});

it("offers rename, pin and close actions on a tab's right-click menu", async () => {
  render(<ShellPage envs={envs} />);
  for (const env of ["A", "B", "A", "B"]) await openSession(env);
  const names = () => screen.getAllByRole("tab").map((tab) => tab.textContent);
  const menu = (index: number) => { fireEvent.contextMenu(screen.getAllByRole("tab")[index]); return screen.getByRole("menu"); };
  const item = (index: number, name: string) => within(menu(index)).getByRole("menuitem", { name: new RegExp(`^${name}($|Ctrl|F2)`) }) as HTMLButtonElement;
  expect(item(0, "Close to the left").disabled).toBe(true);
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();

  // Pinning moves the tab first; pinned tabs have no close button and survive the bulk closes.
  fireEvent.click(item(3, "Pin"));
  expect(names()).toEqual(["B", "A", "B", "A"]);
  expect(screen.getAllByRole("button", { name: /Close session/ })).toHaveLength(3);
  expect(screen.getAllByRole("button", { name: "Unpin" })).toHaveLength(1);
  // Closing the active tab from another tab's menu leaves that tab in front.
  fireEvent.click(screen.getAllByRole("tab")[3]);
  fireEvent.click(item(2, "Close to the right"));
  expect(names()).toEqual(["B", "A", "B"]);
  expect(screen.getAllByRole("tab")[2].getAttribute("aria-selected")).toBe("true");
  fireEvent.click(item(2, "Close to the left"));
  expect(names()).toEqual(["B", "B"]);
  expect(item(1, "Close others").disabled).toBe(true);
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });

  fireEvent.click(item(0, "Unpin"));
  expect(screen.getAllByRole("button", { name: /Close session/ })).toHaveLength(2);
  fireEvent.click(item(0, "Rename"));
  const name = screen.getByRole("textbox", { name: "Session name" });
  fireEvent.change(name, { target: { value: "Ventas" } });
  fireEvent.keyDown(name, { key: "Enter" });
  expect(names()).toEqual(["Ventas B", "B"]);
  fireEvent.click(item(0, "Close"));
  expect(names()).toEqual(["B"]);
});

it("reorders tabs by dragging them, within the pinned or unpinned group", async () => {
  render(<ShellPage envs={envs} />);
  for (const env of ["A", "B"]) await openSession(env);
  const names = () => screen.getAllByRole("tab").map((tab) => tab.textContent);
  // Each tab 100px wide, side by side.
  screen.getAllByRole("tab").forEach((tab, i) => vi.spyOn(tab.parentElement!, "getBoundingClientRect").mockReturnValue(
    { width: 100, height: 34, x: i * 100, y: 0, top: 0, left: i * 100, right: (i + 1) * 100, bottom: 34, toJSON: () => ({}) }));
  const [a] = screen.getAllByRole("tab");
  fireEvent.mouseDown(a, { button: 0, clientX: 10 });
  // Short of the next tab's middle nothing moves.
  // The tab follows the pointer; short of the next tab's middle there is no landing line.
  fireEvent.mouseMove(window, { clientX: 140 });
  expect(a.parentElement!.style.transform).toBe("translateX(130px)");
  expect(document.querySelector("[data-drop]")).toBeNull();
  // Past it, a line marks the gap after B; the order changes only on release.
  fireEvent.mouseMove(window, { clientX: 160 });
  expect(screen.getAllByRole("tab")[1].parentElement!.getAttribute("data-drop")).toBe("after");
  expect(names()).toEqual(["A", "B"]);
  fireEvent.mouseUp(window);
  expect(names()).toEqual(["B", "A"]);
  expect(document.querySelector("[data-drop], [data-dragging]")).toBeNull();
  // After the release the pointer moves nothing.
  fireEvent.mouseMove(window, { clientX: 40 });
  expect(names()).toEqual(["B", "A"]);

  // A pinned tab never crosses into the unpinned ones.
  fireEvent.contextMenu(screen.getAllByRole("tab")[0]);
  fireEvent.click(screen.getByRole("menuitem", { name: "Pin" }));
  fireEvent.mouseDown(screen.getAllByRole("tab")[0], { button: 0, clientX: 10 });
  fireEvent.mouseMove(window, { clientX: 160 });
  expect(document.querySelector("[data-drop]")).toBeNull();
  fireEvent.mouseUp(window);
  expect(names()).toEqual(["B", "A"]);
});

it("searches history backwards with Ctrl+R like bash", async () => {
  vi.mocked(ipc.shellHistory).mockResolvedValue([
    { at: "2026-01-01T00:00:00Z", command: "count res.partner" },
    { at: "2026-01-02T00:00:00Z", command: "search res.partner -l 5" },
    { at: "2026-01-03T00:00:00Z", command: "whoami" },
  ]);
  render(<ShellPage envs={envs} />);
  await openSession("A");
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  // What reads on the line: the query once (its painted copy is hidden from readers).
  const line = () => { const label = input.closest("label")!.cloneNode(true) as HTMLElement; label.querySelectorAll("[aria-hidden]").forEach((n) => n.remove()); return label.textContent; };
  fireEvent.change(input, { target: { value: "typed" } });
  fireEvent.keyDown(input, { key: "r", ctrlKey: true });
  // The query replaces the line; the newest match follows it, the query marked.
  fireEvent.change(input, { target: { value: "partner" } });
  expect(line()).toBe("(reverse-i-search) 'partner': search res.partner -l 5");
  expect(input.closest("label")!.querySelector("mark")?.textContent).toBe("partner");
  fireEvent.keyDown(input, { key: "r", ctrlKey: true });
  expect(line()).toContain("': count res.partner");
  // A query that stops matching keeps the last match and says so.
  fireEvent.change(input, { target: { value: "partnerx" } });
  expect(line()).toBe("(failed reverse-i-search) 'partnerx': search res.partner -l 5");
  // Ctrl+G gives the line back; an arrow leaves the match to edit; Enter runs it.
  fireEvent.keyDown(input, { key: "g", ctrlKey: true });
  expect(input.value).toBe("typed");
  fireEvent.keyDown(input, { key: "r", ctrlKey: true });
  fireEvent.change(input, { target: { value: "count" } });
  fireEvent.keyDown(input, { key: "ArrowRight" });
  expect(input.value).toBe("count res.partner");
  fireEvent.keyDown(input, { key: "r", ctrlKey: true });
  fireEvent.change(input, { target: { value: "who" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(input.value).toBe("");
  await waitFor(() => expect(within(panel()).getAllByText("whoami").length).toBeGreaterThan(0));
});

it("preserves the tab label width and icon while editing its name", () => {
  render(<ShellPage envs={envs} />);
  newSession("A");
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
  await openSession("A");
  for (const line of ["\\context", "\\timing", "\\context", "\\context"]) command(line);
  await waitFor(() => expect(stored.map((h) => h.command)).toEqual(["\\context", "\\timing", "\\context"]));
  const input = () => (screen.getByRole("textbox", { name: "Command" }) as HTMLInputElement).value;
  command("!2");
  expect(input()).toBe("\\timing");
  command("!3");
  expect(input()).toBe("\\context");
  newSession("A");
  await waitFor(() => expect(ipc.shellHistory).toHaveBeenCalledTimes(2));
  await waitFor(() => { command("!2"); expect(input()).toBe("\\timing"); });
  command("!3");
  expect(input()).toBe("\\context");
});

const answer = (data: (call: Parameters<typeof ipc.shell>[0]) => unknown) => vi.mocked(ipc.shell).mockImplementation(async (call) => ({ data: call.model === "ir.model" ? [] : data(call), write: false, upstream_ms: 3, upstream_bytes: 4 }));
const connectA = () => openSession("A");

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
  expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["", "country_id", "count"]);
  const firstColumn = () => [...table.querySelectorAll("tbody tr")].map((r) => r.querySelector("td:nth-child(2)")?.textContent);
  // The count column adds up in the footer; the group column has nothing to add.
  expect([...table.querySelectorAll("tfoot td")].map((td) => td.textContent)).toEqual(["Σ", "", "16"]);
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
    ? Promise.resolve(shellResult([]))
    : new Promise((resolve) => { finish = resolve; }));
  render(<ShellPage envs={envs} />);
  await connectA();
  command("count res.partner");
  expect(within(panel()).getByText(/Ctrl\+C discards/)).toBeTruthy();
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.keyDown(input, { key: "c", ctrlKey: true });
  expect(within(panel()).getByText(/^Discarded\./)).toBeTruthy();
  expect(within(panel()).queryByText(/Ctrl\+C discards/)).toBeNull();
  finish(shellResult(4242));
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
  fireEvent.doubleClick(await within(panel()).findByRole("button", { name: "Spain (68)" }));
  const typed = (line: string) => [...panel().querySelectorAll(".term-cmd")].some((l) => l.textContent?.includes(line));
  await waitFor(() => expect(typed("read res.country 68 -f *")).toBe(true));
  expect(vi.mocked(ipc.shell).mock.calls.map(([call]) => call).filter((call) => call.method !== "fields_get").at(-1)).toMatchObject({ model: "res.country", method: "read", args: [[68]], kwargs: {} });
  fireEvent.doubleClick(within(panel()).getAllByRole("button", { name: "7" })[0]);
  await waitFor(() => expect(typed("read res.partner 7 -f *")).toBe(true));
  fireEvent.doubleClick(within(panel()).getAllByRole("button", { name: /^x{80}…$/ })[0]);
  const dialog = within(panel()).getByRole("dialog", { name: "res.partner 7 · comment" });
  expect(dialog.textContent).toContain("x".repeat(120));
  fireEvent.keyDown(screen.getByRole("textbox", { name: "Command" }), { key: "Escape" });
  expect(within(panel()).queryByRole("dialog")).toBeNull();
});

it("inserts newlines with Shift+Enter, submits multi-line commands and flags syntax errors after a pause", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
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
  await vi.advanceTimersByTimeAsync(HINT_DELAY_MS + 200);
  expect(panel().querySelector(".term-hint")).toBeNull();
  fireEvent.change(input, { target: { value: "search res.partner -d {'name': 1}" } });
  await vi.advanceTimersByTimeAsync(HINT_DELAY_MS + 200);
  await waitFor(() => expect(panel().querySelector(".term-hint")?.textContent).toMatch(/list/i));
  // A lone first word is left to the highlighter.
  fireEvent.change(input, { target: { value: "\\" } });
  await vi.advanceTimersByTimeAsync(HINT_DELAY_MS + 200);
  expect(panel().querySelector(".term-hint")).toBeNull();
});

it("suggests the newest matching history command and accepts it with the right arrow", async () => {
  vi.mocked(ipc.shellHistory).mockResolvedValue([
    { at: "2026-10-19T10:00:00Z", command: "search res.partner -l 5" },
    { at: "2026-10-19T10:01:00Z", command: "search res.users -l 1" },
  ]);
  render(<ShellPage envs={envs} />);
  await openSession("A");
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
    ? shellResult([{ id: 3, name: "Ada", email: "ada@x" }, { id: 5, name: "Bob", email: "bob@x" }])
    : shellResult(true));
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
  expect(within(tables.at(-1)!).getAllByRole("columnheader").map((th) => th.textContent)).toEqual(["", "email", "id"]);
  command("\\cols nope");
  expect(within(panel()).getByText(/Unknown columns: nope/)).toBeTruthy();
  command("\\prev");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0].kwargs).toMatchObject({ offset: 0 }));
  command("call res.partner check $ada");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "check", args: [3] }));
});

it("restores aliases from the vault even when no saved tab was used", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "x", aliases: { pp: "search res.partner" }, tabs: [] }));
  render(<ShellPage envs={envs} />);
  await connectA();
  command("pp -l 1");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ model: "res.partner", method: "search_read" }));
  command("\\alias pq count res.partner");
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  expect(JSON.parse(vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)?.[0] ?? "{}").aliases).toEqual({ pp: "search res.partner", pq: "count res.partner" });
});

it("finds text in the output with Ctrl+F and zooms the font with Ctrl +/-", async () => {
  render(<ShellPage envs={envs} />);
  await openSession("A");
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
    if (call.method === "search_read") return shellResult([{ id: 3 }, { id: 5 }]);
    if (!call.confirmed) throw { code: "confirmation_required", message: "", outcome: "not_sent" };
    return shellResult(true, true);
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

it("never lets $_ keep ids no longer shown: a table without ids clears it, a create replaces it", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.method === "search_read") return shellResult([{ id: 3, name: "A" }, { id: 5, name: "B" }]);
    if (!call.confirmed) throw { code: "confirmation_required", message: "", outcome: "not_sent" };
    return shellResult(call.method === "create" ? 9 : true, true);
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f name");
  await waitFor(() => expect(within(panel()).getByText("2 rows", { exact: false })).toBeTruthy());
  command("\\cols name");
  command("write res.partner $_ {'active': False}");
  expect(within(panel()).getByText("$_ needs a previous result with records")).toBeTruthy();
  command("search res.partner -f name");
  await waitFor(() => expect(within(panel()).getAllByText("2 rows", { exact: false })).toHaveLength(2));
  command("create res.partner {'name': 'C'}");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  command("y");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.at(-1)?.[0]).toMatchObject({ method: "create", confirmed: true }));
  command("write res.partner $_ {'active': False}");
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "write" && JSON.stringify(call.args) === JSON.stringify([[9], { active: false }]))).toBe(true));
});

it("opens records in Odoo with Ctrl+click and saves tables to Downloads", async () => {
  vi.mocked(ipc.openUrl).mockResolvedValue(undefined);
  vi.mocked(ipc.shellExport).mockResolvedValue("/home/u/Downloads/odx-res.partner.csv");
  answer((call) => call.method === "fields_get" ? { country_id: { relation: "res.country" } } : [{ id: 7, name: "Ada, \"the\" first", country_id: [68, "Spain"] }, { id: 8, name: "=HYPERLINK(\"x\")", country_id: false }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f name,country_id");
  fireEvent.doubleClick(await within(panel()).findByRole("button", { name: "Spain (68)" }), { ctrlKey: true });
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
    if (call.model === "ir.model") return shellResult([]);
    throw { code: "upstream.remote", detail: "ValidationError: Email is required\nline 2\nline 3", outcome: "failed" };
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("read res.partner 1");
  const summary = await within(panel()).findByText("Show 2 more lines");
  expect(summary.closest(".term-line")?.textContent).toContain("ValidationError: Email is required");
  expect(summary.closest("details")?.open).toBe(false);
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
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t2", tabs: [savedTab("t1", "A", "count res.partner"), savedTab("t2", "B", "whoami")] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(screen.getAllByRole("tab")).toHaveLength(2));
  expect(within(panel()).getByText("out of whoami")).toBeTruthy();
  expect(within(panel()).getByText(/Session restored/)).toBeTruthy();
  await connectedTo("B");
  expect(ipc.check).toHaveBeenCalledTimes(1);
  expect(ipc.check).toHaveBeenCalledWith("B");
  fireEvent.click(screen.getAllByRole("tab")[0]);
  expect(within(panel()).getByText("out of count res.partner")).toBeTruthy();
  await connectedTo("A");
  expect(ipc.check).toHaveBeenCalledTimes(2);
  // The restore note and the automatic reconnection are shown but not saved, so restarts do not pile them up.
  vi.mocked(ipc.saveShellSessions).mockClear();
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  expect(ipc.saveShellSessions).toHaveBeenCalled();
  const saved = vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)![0];
  expect(saved).toContain("out of whoami");
  expect(saved).not.toMatch(/Session restored|Connected to|\\\\c [AB]/);
});

it("does not switch a restored tab to the requested environment", async () => {
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t2", tabs: [savedTab("t2", "B", "whoami")] }));
  const handled = vi.fn();
  render(<ShellPage envs={envs} connectTo="A" command="read res.partner 7" onConnectHandled={handled} />);
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "read" && call.environment_id === "A")).toBe(true));
  // The restored session stays as it was; the request gets its own tab, shown.
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  await connectedTo("A");
  expect(within(panel()).queryByText("out of whoami")).toBeNull();
  fireEvent.click(screen.getAllByRole("tab")[0]);
  expect(within(panel()).getByText("out of whoami")).toBeTruthy();
});

it("does not announce a restored session when only untouched tabs were saved", async () => {
  const untouched = { id: "t9", name: "", defaultName: true, session: { entries: [{ id: 90, kind: "info", text: "", translationKey: "shell.initial" }], context: {}, expanded: false, timing: true } };
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t9", tabs: [untouched, { ...untouched, id: "t8" }] }));
  render(<ShellPage envs={envs} />);
  await waitFor(() => expect(ipc.shellSessions).toHaveBeenCalled());
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(screen.queryByRole("tab")).toBeNull();
  expect(screen.queryByText(/Session restored/)).toBeNull();
  // A saved session with work still restores, without its untouched tabs.
  cleanup();
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t9", tabs: [untouched, savedTab("t3", null, "\\l"), savedTab("t2", "A", "whoami")] }));
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
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement).disabled).toBe(true);
  expect(within(panel()).getByText("Deleted")).toBeTruthy();
});

it("saves sessions to the vault only after loading them, without pending confirmations", async () => {
  let load!: (data: string | null) => void;
  vi.mocked(ipc.shellSessions).mockReturnValue(new Promise((resolve) => { load = resolve; }));
  requireConfirmation(["search_read"]);
  vi.useFakeTimers({ shouldAdvanceTime: true });
  render(<ShellPage envs={envs} />);
  await openSession("A");
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  expect(ipc.saveShellSessions).not.toHaveBeenCalled();
  load(null);
  command("unlink res.partner 5");
  await vi.advanceTimersByTimeAsync(SAVE_DELAY_MS + 200);
  await waitFor(() => expect(ipc.saveShellSessions).toHaveBeenCalled());
  const saved = JSON.parse(vi.mocked(ipc.saveShellSessions).mock.calls.at(-1)![0]);
  expect(saved.tabs[0]).toMatchObject({ envId: "A", envName: "A", session: { env: { id: "A", name: "A" } } });
  expect(JSON.stringify(saved)).toContain("unlink res.partner 5");
  expect(JSON.stringify(saved)).not.toContain("confirming");
});

it("warns when $_ holds ids of another model and does not flag $_ as a syntax error", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.model === "ir.model.data") return shellResult([{ id: 2, module: "base", name: "main_company", model: "res.company", res_id: 1 }]);
    if (call.model === "ir.model") return shellResult([]);
    if (!call.confirmed) throw { code: "confirmation_required", message: "", outcome: "not_sent" };
    return shellResult(true, true);
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("ref base.main_company");
  await within(panel()).findByText("main_company");
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.change(input, { target: { value: "write res.company $_ {'name': 'X'}" } });
  await vi.advanceTimersByTimeAsync(HINT_DELAY_MS + 200);
  expect(panel().querySelector(".term-hint")).toBeNull();
  fireEvent.keyDown(input, { key: "Enter" });
  await waitFor(() => expect(within(panel()).getByText(/\$_ holds ir\.model\.data ids, not res\.company ids/)).toBeTruthy());
  expect(await within(panel()).findByText(/^write res\.company \[\[2\]/)).toBeTruthy();
});

it("asks for the environment when opening a session and keeps it and its write mode in view", async () => {
  render(<ShellPage envs={envs} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  const menu = screen.getByRole("menu", { name: "New Terminal session" });
  expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["A", "B"]);
  fireEvent.click(within(menu).getByRole("menuitem", { name: "B" }));
  expect(screen.queryByRole("menu")).toBeNull();
  expect(screen.getAllByRole("tab")).toHaveLength(1);
  expect(screen.getByRole("tab").textContent).toBe("B");
  await connectedTo("B");
  expect(ipc.check).toHaveBeenCalledWith("B");
  expect(within(panel()).getByText("Approval")).toBeTruthy();
  expect(within(panel()).queryByRole("combobox")).toBeNull();
  // Switching environments inside a session is gone: ids from one database must not reach another.
  command("\\c A");
  expect(within(panel()).getByText(/Unknown command: \\c/)).toBeTruthy();
  expect(ipc.check).toHaveBeenCalledTimes(1);
});

it("keeps a failed session on its environment; its Error chip says why and retries on click", async () => {
  vi.mocked(ipc.check).mockResolvedValueOnce({ environment_id: "A", status: "unreachable", latency_ms: 1, error: "offline" });
  render(<ShellPage envs={envs} />);
  newSession("A");
  const chip = await within(panel()).findByRole("button", { name: "Retry connection" });
  expect(chip.textContent).toBe("Error");
  expect(chip.getAttribute("data-tip")).toBe("Could not connect to A: offline");
  expect(panel().querySelector(".term-input .term-db")?.textContent).toBe("A");
  expect(within(panel()).queryByText(/Could not connect/)).toBeNull();
  let answer!: (health: Awaited<ReturnType<typeof ipc.check>>) => void;
  vi.mocked(ipc.check).mockReturnValueOnce(new Promise((resolve) => { answer = resolve; }));
  fireEvent.click(chip);
  const retrying = within(panel()).getByRole("button", { name: "Retrying…" }) as HTMLButtonElement;
  expect(retrying.disabled).toBe(true);
  answer({ environment_id: "A", status: "ok", latency_ms: 1 });
  await connectedTo("A");
  expect(ipc.check).toHaveBeenCalledTimes(2);
  expect(within(panel()).queryByRole("button", { name: /Retry/ })).toBeNull();
});

it("opens the environment menu with Ctrl+T, closes with Ctrl+W and cycles with Ctrl+Tab", async () => {
  render(<ShellPage envs={envs} />);
  fireEvent.keyDown(window, { key: "t", ctrlKey: true });
  fireEvent.click(screen.getByRole("menuitem", { name: "A" }));
  await openSession("B");
  fireEvent.keyDown(window, { key: "t", ctrlKey: true });
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  expect(screen.queryByRole("menu")).toBeNull();
  await openSession("A");
  expect(screen.getAllByRole("tab")).toHaveLength(3);
  expect(screen.getAllByRole("tab")[2].getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(window, { key: "Tab", ctrlKey: true });
  expect(screen.getAllByRole("tab")[0].getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(window, { key: "Tab", ctrlKey: true, shiftKey: true });
  expect(screen.getAllByRole("tab")[2].getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(window, { key: "w", ctrlKey: true });
  expect(screen.getAllByRole("tab")).toHaveLength(2);
});

it("lists commands in a panel with \\? and types the chosen one", async () => {
  render(<ShellPage envs={envs} />);
  await openSession("A");
  command("\\?");
  const help = screen.getByRole("complementary", { name: "Commands" });
  expect(within(help).getByRole("heading", { name: "Queries" })).toBeTruthy();
  fireEvent.click(within(help).getByRole("button", { name: /^search <model>/ }));
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement).value).toBe("search ");
  expect(screen.queryByRole("complementary", { name: "Commands" })).toBeNull();
});

it("shows the records and values of a write before it is sent, and sends it from the preview", async () => {
  requireConfirmation([]);
  render(<ShellPage envs={envs} />);
  await openSession("A");
  command("write res.partner [7] {'name': 'Ada'}");
  const preview = await screen.findByRole("region", { name: "Send to A? [y/N]" });
  expect(within(preview).getByText("res.partner")).toBeTruthy();
  expect(within(preview).getByText(/A · 7/)).toBeTruthy();
  expect(within(preview).getByText("Ada", { exact: false })).toBeTruthy();
  fireEvent.click(within(preview).getByRole("button", { name: "Send" }));
  await waitFor(() => expect(ipc.shell).toHaveBeenCalledWith(expect.objectContaining({ method: "write", confirmed: true })));
  expect(screen.queryByRole("region", { name: "Send to A? [y/N]" })).toBeNull();
});

it("adds a search box to the session menu when there are many environments, and Enter opens the first match", async () => {
  const many: Environment[] = Array.from({ length: 10 }, (_, i) => ({ ...envs[0], id: `e${i}`, name: `Client ${i}`, url: `https://client${i}.test`, tags: i === 7 ? ["vip"] : [] }));
  const { rerender } = render(<ShellPage envs={envs} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  expect(screen.queryByRole("searchbox", { name: "Find environment" })).toBeNull();
  fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
  rerender(<ShellPage envs={many} />);
  fireEvent.click(screen.getByRole("button", { name: "New Terminal session" }));
  const search = screen.getByRole("searchbox", { name: "Find environment" });
  expect(document.activeElement).toBe(search);
  fireEvent.change(search, { target: { value: "VIP" } });
  expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Client 7"]);
  fireEvent.change(search, { target: { value: "nothing" } });
  expect(screen.getByText("No matches")).toBeTruthy();
  fireEvent.change(search, { target: { value: "client3.test" } });
  fireEvent.keyDown(search, { key: "Enter" });
  expect(screen.queryByRole("menu")).toBeNull();
  await waitFor(() => expect(ipc.check).toHaveBeenCalledWith("e3"));
  expect(screen.getByRole("tab").textContent).toBe("Client 3");
});

it("prompts with the login, shortening a long one, and shows the database at the end of the input", async () => {
  const long = [{ ...envs[0], login: "benjamin.coronel@factorlibre.com", database: "erp_prod" }, envs[1]];
  render(<ShellPage envs={long} />);
  await openSession("B");
  const prompt = () => panel().querySelector(".term-input .term-prompt")?.textContent;
  const database = () => panel().querySelector(".term-input .term-db")?.textContent;
  expect([prompt(), database()]).toEqual(["fixture>", "B"]);
  await openSession("A", "erp_prod");
  expect([prompt(), database()]).toEqual(["benjamin.coronel@fa…>", "erp_prod"]);
  command("\\timing");
  expect(within(panel()).getAllByText("benjamin.coronel@fa…>").length).toBeGreaterThan(1);
});

it("shows the connection only on the status chip, never as output", async () => {
  let answer!: (health: Awaited<ReturnType<typeof ipc.check>>) => void;
  vi.mocked(ipc.check).mockReturnValueOnce(new Promise((resolve) => { answer = resolve; }));
  render(<ShellPage envs={envs} />);
  newSession("A");
  expect(within(panel()).getByRole("button", { name: "Connecting…" })).toBeTruthy();
  expect(within(panel()).queryByText(/Ctrl\+C discards/)).toBeNull();
  answer({ environment_id: "A", status: "ok", server_version: "17.0", latency_ms: 42 });
  await connectedTo("A");
  expect(panel().querySelector(".term-mode")?.getAttribute("data-tip")).toBe("Approval required\nOdoo 17.0 · 42 ms");
  expect(panel().querySelector(".term-out")?.textContent).not.toMatch(/Connect|17\.0/);
});

it("keeps Odoo commands typed instead of sending them while disconnected, and still runs local ones", async () => {
  vi.mocked(ipc.check).mockResolvedValue({ environment_id: "A", status: "unreachable", latency_ms: 1, error: "offline" });
  render(<ShellPage envs={envs} />);
  newSession("A");
  await within(panel()).findByRole("button", { name: "Retry connection" });
  vi.mocked(ipc.shellHistoryAdd).mockClear();
  command("search res.partner -l 5");
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  expect(input.value).toBe("search res.partner -l 5");
  expect(panel().querySelector(".term-hint")?.textContent).toBe("Not connected: click Error to retry, then run it.");
  expect(vi.mocked(ipc.shell).mock.calls.filter(([call]) => call.model !== "ir.model")).toHaveLength(0);
  expect(ipc.shellHistoryAdd).not.toHaveBeenCalled();
  expect(panel().querySelector(".term-out")?.textContent).not.toContain("search res.partner");
  // An incomplete command is held too, rather than answered with its usage.
  command("search");
  expect(panel().querySelector(".term-out")?.textContent).not.toContain("Missing model");
  command("\\context");
  expect(within(panel()).getByText("{}", { exact: true })).toBeTruthy();
});

it("describes table columns with the field label, type and relation after a query", async () => {
  answer((call) => call.method === "fields_get"
    ? { name: { string: "Name", type: "char" }, country_id: { string: "Country", type: "many2one", relation: "res.country" } }
    : [{ id: 7, name: "Ada", country_id: [68, "Spain"] }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f name,country_id");
  const header = async (column: string) => (await within(panel()).findByRole("button", { name: `Sort by ${column}` })).closest("th")?.getAttribute("data-tip");
  await waitFor(async () => expect(await header("country_id")).toBe("Country\nmany2one → res.country"));
  expect(await header("name")).toBe("Name\nchar");
  const loads = () => vi.mocked(ipc.shell).mock.calls.filter(([call]) => call.method === "fields_get").length;
  expect(loads()).toBe(1);
  command("search res.partner -f name");
  await waitFor(() => expect(within(panel()).getAllByRole("table")).toHaveLength(2));
  expect(loads()).toBe(1);
});

it("runs pasted commands in order on Enter, stops at the first error, and pastes them as text on Escape", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.model === "res.users") throw { code: "upstream.remote", detail: "Access denied", outcome: "failed" };
    return shellResult(call.model === "ir.model" ? [] : 3);
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  const paste = (text: string) => fireEvent.paste(input, { clipboardData: { getData: () => text } });
  const counted = () => vi.mocked(ipc.shell).mock.calls.map(([call]) => call).filter((call) => call.method === "search_count").map((call) => call.model);
  paste("count res.partner\ncount res.users\ncount res.country\n");
  expect(within(panel()).getByText("3 commands pasted")).toBeTruthy();
  expect(input.value).toBe("");
  fireEvent.keyDown(input, { key: "Enter" });
  await waitFor(() => expect(within(panel()).getByText("Stopped after command 2 of 3.")).toBeTruthy());
  expect(counted()).toEqual(["res.partner", "res.users"]);
  // A multi-line command (continuation lines) pastes as usual; Escape turns a pasted list into text.
  paste("search res.partner\n  -l 5");
  expect(within(panel()).queryByText(/commands pasted/)).toBeNull();
  paste("count res.partner\n\\timing");
  fireEvent.keyDown(input, { key: "Escape" });
  expect(input.value).toBe("count res.partner\n\\timing");
  expect(counted()).toHaveLength(2);
});

it("waits for a write's confirmation inside pasted commands, then goes on", async () => {
  requireConfirmation(["search_count"]);
  render(<ShellPage envs={envs} />);
  await connectA();
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  fireEvent.paste(input, { clipboardData: { getData: () => "write res.partner [7] {'name': 'Ada'}\ncount res.partner" } });
  fireEvent.keyDown(input, { key: "Enter" });
  const preview = await screen.findByRole("region", { name: "Send to A? [y/N]" });
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "search_count")).toBe(false);
  fireEvent.click(within(preview).getByRole("button", { name: "Send" }));
  await waitFor(() => expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "search_count")).toBe(true));
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "write" && call.confirmed)).toBe(true);
});

it("ignores Enter on an empty line", async () => {
  render(<ShellPage envs={envs} />);
  await connectA();
  const lines = () => panel().querySelectorAll(".term-cmd").length;
  const before = lines();
  command("   ");
  command("");
  expect(lines()).toBe(before);
  expect(ipc.shellHistoryAdd).not.toHaveBeenCalled();
});

it("copies a result's call as a Python script without the API key", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  answer(() => [{ id: 7, name: "Ada", active: true }, { id: 8, name: "Grace", active: false }]);
  render(<ShellPage envs={envs} />);
  await connectA();
  command('\\context {"lang": "es_ES"}');
  command("search res.partner -d [('active','=',True)] -f name -l 2");
  fireEvent.click(await within(panel()).findByRole("button", { name: "More formats" }));
  fireEvent.click(within(panel()).getByRole("menuitem", { name: "Copy as Python" }));
  await waitFor(() => expect(writeText).toHaveBeenCalled());
  const script = writeText.mock.calls[0][0] as string;
  expect(script).toContain('url, db, login = "https://A.test", "A", "fixture"');
  expect(script).toContain('key = "<API key>"');
  expect(script).toMatch(/execute_kw\(db, uid, key, "res\.partner", "search_read", \[\], \{.*"domain": \[\["active", "=", True\]\].*"context": \{"lang": "es_ES"\}\}\)/);
});

it("reads values by their field type and marks each command block by outcome", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => {
    if (call.model === "ir.model") return shellResult([]);
    if (call.method === "fields_get") return shellResult({
      state: { string: "Status", type: "selection", selection: [["draft", "Quotation"], ["sale", "Sales Order"]] },
      locked: { string: "Locked", type: "boolean" }, note: { string: "Note", type: "char" },
      date_order: { string: "Order Date", type: "datetime" },
    });
    if (call.model === "nope") throw { code: "upstream.remote", detail: "No model", outcome: "failed" };
    return shellResult([{ id: 1, state: "sale", locked: true, note: false, date_order: "2026-10-23 08:41:07" }, { id: 2, state: "draft", locked: false, note: false, date_order: false }]);
  });
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search sale.order -f state,locked,note,date_order");
  const table = await within(panel()).findByRole("table");
  await waitFor(() => expect(within(table).getByText("Sales Order")).toBeTruthy());
  const cell = (text: string) => within(table).getByText(text).closest("td")!;
  expect(cell("Sales Order").getAttribute("data-tip")).toBe("sale");
  expect(cell("✓").getAttribute("data-tip")).toBe("true");
  expect(cell("✗").className).toContain("bool no");
  // A boolean field's false is a value; a char field's false is empty.
  expect([...table.querySelectorAll("tbody tr:nth-child(2) td")].map((td) => td.className.includes("dim"))).toEqual([false, false, false, false, true, true]);
  const when = [...table.querySelectorAll("td")].find((td) => td.getAttribute("data-tip") === "2026-10-23 08:41:07 UTC")!;
  expect(when.textContent).toBe(new Date("2026-10-23T08:41:07Z").toLocaleString("en", { dateStyle: "short", timeStyle: "medium" }));
  command("read nope 1");
  await waitFor(() => expect(panel().querySelector('.term-group[data-outcome="error"]')).toBeTruthy());
  expect(panel().querySelector('.term-group[data-outcome="read"]')?.textContent).toContain("search sale.order");
});

const tabId = (index: number) => screen.getAllByRole("tab")[index].id.replace("shell-tab-", "");
const agentRun = (id: number, fields: Partial<Extract<AgentRequest, { kind: "run" }>>): AgentRequest =>
  ({ id, kind: "run", envId: "A", command: "count res.partner", clientId: "c1", clientRevision: 1, clientLabel: "Claude", allowWrites: false, ...fields });

it("runs an agent's command in its tab without bringing it forward, marked with the agent and audited as it", async () => {
  answer(() => 42);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  await openSession("B");
  const a = tabId(0);
  const handled = vi.fn();
  rerender(<ShellPage envs={envs} agentRequests={[agentRun(1, { terminal: a })]} onAgentHandled={handled} />);
  expect(handled).toHaveBeenCalledWith(1);
  await waitFor(() => expect(ipc.agentReply).toHaveBeenCalled());
  const [id, result] = vi.mocked(ipc.agentReply).mock.calls[0];
  expect(id).toBe(1);
  expect(result).toMatchObject({ terminal: a, env: "A", command: "count res.partner", output: [{ kind: "input", text: "count res.partner" }, { kind: "text", text: "42" }, { kind: "info", text: "3 ms · 4 B" }] });
  expect(vi.mocked(ipc.shell).mock.calls.find(([call]) => call.method === "search_count")?.[0]).toMatchObject({ environment_id: "A", client_id: "c1", client_revision: 1 });
  // B stays in front; A shows that something happened there.
  expect(screen.getAllByRole("tab")[1].getAttribute("aria-selected")).toBe("true");
  expect(within(screen.getAllByRole("tab")[0]).getByRole("status", { name: "An agent ran commands here" })).toBeTruthy();
  fireEvent.click(screen.getAllByRole("tab")[0]);
  expect(within(panel()).getByText("Claude")).toBeTruthy();
  expect(within(screen.getAllByRole("tab")[0]).queryByRole("status")).toBeNull();
});

it("opens a session for an agent's environment, refuses writes it may not request, and shows a tab on request", async () => {
  requireConfirmation(["search_count"]);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  rerender(<ShellPage envs={envs} agentRequests={[agentRun(2, { envId: "B", command: "write res.partner [1] {'name': 'X'}" })]} />);
  await waitFor(() => expect(ipc.agentReply).toHaveBeenCalled());
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(screen.getAllByRole("tab")[0].getAttribute("aria-selected")).toBe("true");
  const [, result] = vi.mocked(ipc.agentReply).mock.calls[0];
  expect(JSON.stringify(result)).toContain("Claude may not request writes: not sent.");
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "write" && call.confirmed)).toBe(false);
  rerender(<ShellPage envs={envs} agentRequests={[{ id: 3, kind: "show", terminal: tabId(1) }]} />);
  await waitFor(() => expect(screen.getAllByRole("tab")[1].getAttribute("aria-selected")).toBe("true"));
  expect(ipc.agentReply).toHaveBeenLastCalledWith(3);
});

it("runs an agent command queued behind the user's open confirmation as its own command, never as the answer", async () => {
  requireConfirmation(["search_count", "check_access_rights"]);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  command("call res.partner action_custom [[1]]");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  rerender(<ShellPage envs={envs} agentRequests={[agentRun(4, { terminal: tabId(0), command: "count res.partner" })]} />);
  command("n");
  await waitFor(() => expect(ipc.agentReply).toHaveBeenCalled());
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "search_count" && call.client_id === "c1")).toBe(true);
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "action_custom" && call.confirmed)).toBe(false);
  const [, result] = vi.mocked(ipc.agentReply).mock.calls[0];
  expect(JSON.stringify(result)).not.toContain("Cancelled");
});

it("never saves an agent's write confirmation with the window, so it cannot be answered as the user after a restore", async () => {
  requireConfirmation(["search_count", "check_access_rights"]);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  const key = `shell.${tabId(0)}.confirming`;
  rerender(<ShellPage envs={envs} agentRequests={[agentRun(5, { terminal: tabId(0), command: "call res.partner action_custom [[1]]", allowWrites: true })]} />);
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  expect(captureWindowState()[key]).toBeNull();
  command("n");
  await waitFor(() => expect(ipc.agentReply).toHaveBeenCalled());
  command("call res.partner action_custom [[1]]");
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  expect(captureWindowState()[key]).toMatchObject({ call: { method: "action_custom" } });
});

it("runs two agent commands for an environment without a tab in one new session, in order", async () => {
  answer(() => 1);
  const { rerender } = render(<ShellPage envs={envs} />);
  await openSession("A");
  rerender(<ShellPage envs={envs} agentRequests={[agentRun(6, { envId: "B" }), agentRun(7, { envId: "B", command: "count res.users" })]} />);
  await waitFor(() => expect(ipc.agentReply).toHaveBeenCalledTimes(2));
  expect(screen.getAllByRole("tab")).toHaveLength(2);
  expect(vi.mocked(ipc.agentReply).mock.calls.map(([id]) => id)).toEqual([6, 7]);
});

it("lists history as number, date and command, and a click puts the whole command in the input", async () => {
  vi.mocked(ipc.shellHistory).mockResolvedValue([
    { at: "2026-10-19T10:00:00Z", environment: "A", command: "search res.partner -d [('name','ilike','ada')] -l 5" },
    { at: "2026-10-19T10:01:00Z", environment: "B", command: "count res.users" },
  ]);
  render(<ShellPage envs={envs} />);
  await openSession("A");
  command("\\history");
  const table = within(panel()).getAllByRole("table").at(-1)!;
  // # is the number !n recalls.
  expect(within(table).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["", "#", "date", "command"]);
  expect(within(table).getAllByRole("row")[2].textContent).toContain("2");
  fireEvent.doubleClick(within(table).getByRole("button", { name: /^search res\.partner/ }));
  expect((screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement).value).toBe("search res.partner -d [('name','ilike','ada')] -l 5");
  // Loaded, not run.
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.model === "res.partner")).toBe(false);
});

it("opens a table in its own window with the fields that describe it", async () => {
  vi.mocked(ipc.shell).mockImplementation(async (call) => (shellResult(call.model === "ir.model" ? [] : call.method === "fields_get"
    ? { name: { string: "Name", type: "char" } } : [{ id: 7, name: "Ada" }, { id: 8, name: "Grace" }])));
  render(<ShellPage envs={envs} />);
  await connectA();
  command("search res.partner -f name");
  await waitFor(() => expect(within(panel()).getByRole("button", { name: "Sort by name" }).closest("th")?.getAttribute("data-tip")).toBe("Name\nchar"));
  fireEvent.click(within(panel()).getByRole("button", { name: "Open in a window" }));
  const [payload, title] = vi.mocked(ipc.openTableWindow).mock.calls[0] as [{ table: { rows: unknown[][]; model: string }; footer: string; fields: { name: string }[] }, string];
  expect(title).toBe("A (A) - search res.partner -f name");
  expect(payload.table).toMatchObject({ model: "res.partner", rows: [[7, "Ada"], [8, "Grace"]] });
  expect(payload.fields.map((f) => f.name)).toEqual(["name"]);
  expect(payload.footer).toContain("2 rows");
});

it("drops the greeting and the restore note once a command runs, not when one is held back", async () => {
  vi.mocked(ipc.check).mockResolvedValueOnce({ environment_id: "A", status: "unreachable", latency_ms: 1, error: "offline" });
  vi.mocked(ipc.shellSessions).mockResolvedValue(JSON.stringify({ version: 1, activeId: "t1", tabs: [savedTab("t1", "A", "whoami")] }));
  render(<ShellPage envs={envs} />);
  await screen.findByRole("tabpanel");
  await within(panel()).findByRole("button", { name: "Retry connection" });
  expect(within(panel()).getByText(/Session restored/)).toBeTruthy();
  command("count res.partner");
  expect(within(panel()).getByText(/Session restored/)).toBeTruthy();
  command("\\context");
  await waitFor(() => expect(within(panel()).queryByText(/Session restored/)).toBeNull());
  expect(within(panel()).getByText("out of whoami")).toBeTruthy();
});

it("keeps help, history and clearing out of the history", async () => {
  render(<ShellPage envs={envs} />);
  await openSession("A");
  vi.mocked(ipc.shellHistoryAdd).mockClear();
  for (const line of ["\\?", "help", "\\history", "\\history res.partner", "\\s", "\\clear", "\\timing"]) command(line);
  expect(vi.mocked(ipc.shellHistoryAdd).mock.calls.map(([line]) => line)).toEqual(["\\timing"]);
});

it("Escape on a write confirmation stops a pasted batch, as answering n does", async () => {
  requireConfirmation(["search_count"]);
  render(<ShellPage envs={envs} />);
  await connectA();
  const input = screen.getByRole("textbox", { name: "Command" }) as HTMLTextAreaElement;
  fireEvent.paste(input, { clipboardData: { getData: () => "call res.partner action_custom [[1]]\ncount res.partner\n" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(await screen.findByText("Send to A? [y/N]")).toBeTruthy();
  fireEvent.keyDown(input, { key: "Escape" });
  await waitFor(() => expect(within(panel()).getByText("Stopped after command 1 of 2.")).toBeTruthy());
  await new Promise((resolve) => setTimeout(resolve, 50));
  expect(vi.mocked(ipc.shell).mock.calls.some(([call]) => call.method === "search_count")).toBe(false);
});
