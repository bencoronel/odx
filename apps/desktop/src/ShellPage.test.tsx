import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ShellPage from "./ShellPage";
import App from "./App";
import { ipc, type Environment } from "./ipc";

vi.mock("./ipc", () => ({ ipc: {
  shellHistory: vi.fn(), shellHistoryAdd: vi.fn(), check: vi.fn(), shell: vi.fn(),
  status: vi.fn(), environments: vi.fn(), approvals: vi.fn(), clients: vi.fn(), serviceStatus: vi.fn(), lock: vi.fn(), unlock: vi.fn(),
} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
vi.mock("./WindowTitleBar", () => ({ WindowTitleBar: () => null }));
const envs: Environment[] = ["A", "B"].map((name) => ({
  id: name, name, url: `https://${name}.test`, database: name, login: "fixture",
  protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true,
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
