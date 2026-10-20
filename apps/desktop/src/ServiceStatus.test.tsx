import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ServiceStatus from "./ServiceStatus";
import App from "./App";
import { ipc, type Service, type ServiceStatus as Services } from "./ipc";

vi.mock("./ipc", () => ({ ipc: {
  status: vi.fn(), environments: vi.fn(), approvals: vi.fn(), clients: vi.fn(), serviceStatus: vi.fn(), lock: vi.fn(), decide: vi.fn(),
  activity: vi.fn(), settings: vi.fn(), proxy: vi.fn(), headers: vi.fn(), context: vi.fn(), shellHistory: vi.fn(), shellSessions: vi.fn(async () => null), saveShellSessions: vi.fn(async () => undefined), check: vi.fn(), faviconPage: vi.fn(), faviconImage: vi.fn(),
} }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
vi.mock("./WindowTitleBar", () => ({ WindowTitleBar: () => null }));

const listening: Service = { state: "listening", address: "127.0.0.1:8770" };
const services = (api: Service, mcp: Service = listening, connections: Services["connections"] = []): Services => ({
  api, mcp, connections, connector_command: "/tmp/fixture-Odx", connector_args: ["mcp-stdio"], api_url: "http://127.0.0.1:8770/api/v1",
});
const bar = () => screen.getByRole("contentinfo", { name: "Service status" });
afterEach(() => { cleanup(); vi.useRealTimers(); });

it("shows listening services as active only while the vault is unlocked", () => {
  const openMcp = vi.fn();
  render(<ServiceStatus vault="unlocked" services={services(listening, listening, [{ client_id: "c", client_label: "Agent", since: "2026-10-16T10:00:00Z" }])} pending={2} openMcp={openMcp} />);
  fireEvent.click(within(bar()).getByRole("button", { name: /MCP active · 1 connection/ }));
  expect(openMcp).toHaveBeenCalled();
  expect(bar().textContent).not.toContain("API");
  expect(within(bar()).queryByRole("button", { name: /API/ })).toBeNull();
  expect(within(bar()).getByRole("button", { name: "2 pending approvals" })).toBeTruthy();
  expect(bar().textContent).not.toContain("locked");
});

it("distinguishes a stopped service from one that failed, with its error", () => {
  render(<ServiceStatus vault="unlocked" services={services({ state: "stopped" }, { state: "error", error: "service.mcp_socket_failed" })} openMcp={vi.fn()} />);
  expect(bar().textContent).not.toContain("API");
  const mcp = within(bar()).getByRole("button", { name: /MCP error/ });
  expect(mcp.className).toContain("text-danger");
  expect(mcp.getAttribute("data-tip")).toContain("The local MCP connector socket could not be opened.");
  expect(bar().textContent).not.toContain("active");
});

it("hides service states until the vault is unlocked", () => {
  const { rerender } = render(<ServiceStatus vault="locked" services={services({ state: "stopped" })} openMcp={vi.fn()} />);
  expect(bar().textContent).not.toContain("Vault locked");
  expect(bar().textContent).not.toContain("MCP");
  expect(bar().textContent).not.toContain("API");
  expect(bar().textContent).toContain("Unlock Odx to access saved environments");
  expect(bar().textContent).not.toContain("active");
  expect(within(bar()).queryAllByRole("button")).toHaveLength(0);
  expect(bar().querySelector(".tone-ok")).toBeNull();
  for (const vault of ["loading", "absent"]) {
    rerender(<ServiceStatus vault={vault} services={null} />);
    expect(bar().textContent).toContain("Unlock Odx to access saved environments");
    expect(bar().textContent).not.toContain("active");
  }
});

beforeEach(() => {
  vi.resetAllMocks();
  Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: vi.fn() });
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
  localStorage.removeItem("odx.page");
  vi.mocked(ipc.status).mockResolvedValue("unlocked");
  vi.mocked(ipc.environments).mockResolvedValue([]);
  vi.mocked(ipc.approvals).mockResolvedValue([]);
  vi.mocked(ipc.clients).mockResolvedValue([]);
  vi.mocked(ipc.serviceStatus).mockResolvedValue(services({ state: "stopped" }));
  vi.mocked(ipc.activity).mockResolvedValue({ events: [], next_cursor: null, complete: true });
  vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
  vi.mocked(ipc.context).mockResolvedValue({ entries: [], inherited: {} });
  vi.mocked(ipc.shellHistory).mockResolvedValue([]);
  vi.mocked(ipc.settings).mockRejectedValue("no settings in this test");
  vi.mocked(ipc.proxy).mockResolvedValue(null);
});

it("keeps the sidebar and service status across pages without refetching on navigation", async () => {
  render(<App />);
  await within(await screen.findByRole("contentinfo", { name: "Service status" })).findByText(/MCP active/);
  const nav = within(screen.getByRole("navigation", { name: "Main navigation" }));
  const mcp = within(nav.getByRole("group", { name: "MCP" }));
  expect(mcp.getByRole("button", { name: "Local" })).toBeTruthy();
  for (const api of nav.getAllByRole("button", { name: "API" })) {
    expect((api as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(api);
  }
  expect(nav.getByRole("button", { name: "Environments" }).getAttribute("aria-current")).toBe("page");
  const calls = vi.mocked(ipc.serviceStatus).mock.calls.length;
  const before = bar().textContent;
  for (const name of ["Local", "Activity", "Settings", "Terminal", "Approvals", "Environments"]) {
    fireEvent.click(within(screen.getByRole("navigation", { name: "Main navigation" })).getByRole("button", { name }));
    expect(screen.getByRole("button", { name }).getAttribute("aria-current")).toBe("page");
    expect(bar().textContent).toBe(before);
  }
  fireEvent.click(screen.getByRole("button", { name: "Activity" }));
  expect(localStorage.getItem("odx.page")).toBe("activity");
  expect(vi.mocked(ipc.serviceStatus).mock.calls.length).toBe(calls);
});

it("starts locked without announcing services as ready", async () => {
  vi.mocked(ipc.status).mockResolvedValue("locked");
  vi.mocked(ipc.serviceStatus).mockResolvedValue(services({ state: "stopped" }));
  render(<App />);
  await within(await screen.findByRole("contentinfo", { name: "Service status" })).findByText("Unlock Odx to access saved environments");
  expect(bar().textContent).not.toContain("Vault locked");
  expect(bar().textContent).not.toContain("MCP");
  expect(bar().textContent).not.toContain("API");
  expect(bar().textContent).not.toContain("active");
  expect(screen.queryByRole("navigation", { name: "Main navigation" })).toBeNull();
  expect(ipc.environments).not.toHaveBeenCalled();
});

it("drops the active status when the vault is locked outside the window", async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  render(<App />);
  await within(await screen.findByRole("contentinfo", { name: "Service status" })).findByText(/MCP active/);
  vi.mocked(ipc.status).mockResolvedValue("locked");
  vi.mocked(ipc.serviceStatus).mockResolvedValue(services({ state: "stopped" }));
  await act(async () => { await vi.advanceTimersByTimeAsync(2100); });
  await waitFor(() => expect(bar().textContent).not.toContain("Vault locked"));
  expect(bar().textContent).not.toContain("active");
  expect(screen.getByRole("heading", { name: "Vault locked" })).toBeTruthy();
});

it("locking from the sidebar drops pending work and navigation and shows the unlock screen", async () => {
  vi.mocked(ipc.approvals).mockResolvedValue([{ id: "p1", environment_id: "e", environment_name: "A", model: "res.partner", method: "write", args: [], kwargs: {}, context: {}, expires_at: "2026-10-16T10:05:00Z" }]);
  render(<App />);
  await within(await screen.findByRole("contentinfo", { name: "Service status" })).findByRole("button", { name: "1 pending approval" });
  vi.mocked(ipc.lock).mockImplementation(async () => {
    vi.mocked(ipc.status).mockResolvedValue("locked");
    vi.mocked(ipc.serviceStatus).mockResolvedValue(services({ state: "stopped" }));
  });
  fireEvent.click(screen.getByRole("button", { name: "Lock vault" }));
  await screen.findByRole("heading", { name: "Vault locked" });
  expect(ipc.lock).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("navigation", { name: "Main navigation" })).toBeNull();
  expect(bar().textContent).not.toContain("pending");
  expect(bar().textContent).not.toContain("active");
});

it("moves between sections with Ctrl+1…6 keeping the service status", async () => {
  render(<App />);
  await within(await screen.findByRole("contentinfo", { name: "Service status" })).findByText(/MCP active/);
  const before = bar().textContent;
  for (const [key, name] of [["4", "Local"], ["2", "Approvals"], ["6", "Settings"], ["5", "Activity"], ["1", "Environments"]]) {
    fireEvent.keyDown(window, { key, ctrlKey: true });
    await waitFor(() => expect(screen.getByRole("button", { name }).getAttribute("aria-current")).toBe("page"));
    expect(bar().textContent).toBe(before);
  }
  // Other modifiers preserve system and Shell shortcuts.
  fireEvent.keyDown(window, { key: "2", ctrlKey: true, shiftKey: true });
  expect(screen.getByRole("button", { name: "Environments" }).getAttribute("aria-current")).toBe("page");
});

it("closes the approval dialog when the decision succeeds even if the refresh after it fails", async () => {
  vi.mocked(ipc.approvals).mockResolvedValue([{ id: "p1", environment_id: "e", environment_name: "A", model: "res.partner", method: "write", args: [], kwargs: {}, context: {}, expires_at: new Date(Date.now() + 300_000).toISOString() }]);
  localStorage.setItem("odx.page", "approvals");
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
  vi.mocked(ipc.decide).mockResolvedValue(undefined);
  vi.mocked(ipc.clients).mockRejectedValue({ code: "app.persistence" });
  fireEvent.click(await screen.findByRole("button", { name: "Confirm send" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(ipc.decide).toHaveBeenCalledWith("p1", true);
});

it("keeps the environment draft across pages and drops it on cancel", async () => {
  localStorage.setItem("odx.page", "environments");
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Create environment" }));
  fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "Dev" } });
  fireEvent.click(screen.getByRole("button", { name: "Activity" }));
  fireEvent.click(screen.getByRole("button", { name: "Environments" }));
  const name = () => screen.getByLabelText(/^Name/) as HTMLInputElement;
  expect((await screen.findByLabelText(/^Name/) as HTMLInputElement).value).toBe("Dev");
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.click(await screen.findByRole("button", { name: "Create environment" }));
  expect(name().value).toBe("");
});
