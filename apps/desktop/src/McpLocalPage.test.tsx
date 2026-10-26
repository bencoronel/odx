import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import McpLocalPage from "./McpLocalPage";
import { ipc, type Client, type ServiceStatus } from "./ipc";
import { envNamed } from "./test/fixtures";
vi.mock("./ipc", () => ({ ipc: { agentSetup: vi.fn(), agentInstall: vi.fn(), mcpDisconnect: vi.fn() } }));
afterEach(cleanup);
beforeEach(() => {
  localStorage.clear();
  vi.mocked(ipc.agentSetup).mockResolvedValue("no_cli");
});
const services: ServiceStatus = {
  api: { state: "listening" }, mcp: { state: "listening" },
  mcp_name: "odx", connector_command: "/tmp/fixture-Odx", connector_args: ["mcp-stdio"],
  api_url: "http://127.0.0.1:8770/api/v1", connections: [],
};
const profile: Client = {
  id: "local-mcp-stdio", local_stdio: true, label: "MCP local",
  environment_ids: [], all_environments: true, capabilities: ["odoo:read"], revoked: false,
  created_at: "2026-10-16T00:00:00Z", token_issued_at: "2026-10-16T00:00:00Z",
};
const envs = ["A", "B"].map((name) => envNamed(name, { id: `env-${name}` }));
function Page(props: Partial<Parameters<typeof McpLocalPage>[0]>) {
  return <McpLocalPage clients={[]} services={services} envs={envs} busy={false} update={vi.fn()} {...props} />;
}
const snippet = () => document.querySelector("pre")!.textContent;

it("summarises connected agents and last use without token actions", () => {
  render(<Page clients={[{ ...profile, last_used_at: new Date(Date.now() - 5 * 60_000).toISOString() }]} services={{ ...services, connections: [
    { id: 1, client_id: profile.id, client_label: profile.label, since: "2026-10-17T10:00:00Z", calls: 0 },
    { id: 2, client_id: profile.id, client_label: profile.label, since: "2026-10-17T10:01:00Z", calls: 3, pid: 4242, command: "claude --resume", cwd: "~/fixture/project" },
  ] }} />);
  expect(screen.getByRole("status", { name: "Local MCP status" }).textContent).toBe("2 agents connected · 5 minutes ago");
  const rows = within(screen.getByRole("region", { name: "Open connections" })).getAllByRole("button", { name: "Disconnect" });
  expect(rows).toHaveLength(2);
  expect(screen.getByText("~/fixture/project").nextElementSibling!.textContent).toMatch(/^claude --resume · PID 4242 · 3 calls · since /);
  expect(screen.getByText("Unknown agent").nextElementSibling!.textContent).toMatch(/^0 calls · since /);
  expect(document.body.textContent).not.toContain("ODX_TOKEN");
  for (const name of ["Replace token", "Revoke", "New HTTP client"]) expect(screen.queryByRole("button", { name })).toBeNull();
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("disconnects one connector and keeps its button busy until the list drops it", async () => {
  vi.mocked(ipc.mcpDisconnect).mockResolvedValue(true);
  render(<Page clients={[profile]} services={{ ...services, connections: [{ id: 7, client_id: profile.id, since: "2026-10-17T10:00:00Z", calls: 0 }] }} />);
  fireEvent.click(screen.getByRole("button", { name: "Disconnect" }));
  expect(ipc.mcpDisconnect).toHaveBeenCalledWith(7);
  expect((await screen.findByRole("button", { name: "Disconnecting…" }) as HTMLButtonElement).disabled).toBe(true);
});

it("shows one setup snippet at a time for the chosen agent", () => {
  render(<Page clients={[profile]} />);
  expect(snippet()).toBe("claude mcp add --transport stdio --scope user odx -- /tmp/fixture-Odx mcp-stdio");
  fireEvent.click(screen.getByRole("radio", { name: "Codex CLI" }));
  expect(snippet()).toBe('codex mcp add odx --env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -- /tmp/fixture-Odx mcp-stdio');
  fireEvent.click(screen.getByRole("radio", { name: "JSON" }));
  expect(JSON.parse(snippet()!).mcpServers.odx).toEqual({ command: services.connector_command, args: ["mcp-stdio"] });
  expect(document.querySelectorAll("pre")).toHaveLength(1);
});

it("quotes both CLI executable paths with spaces, apostrophes and shell syntax", () => {
  render(<Page clients={[profile]} services={{ ...services, connector_command: "/tmp/ODX user's $(example).AppImage" }} />);
  expect(snippet()).toBe(String.raw`claude mcp add --transport stdio --scope user odx -- '/tmp/ODX user'\''s $(example).AppImage' mcp-stdio`);
  fireEvent.click(screen.getByRole("radio", { name: "Codex CLI" }));
  expect(snippet()).toBe(String.raw`codex mcp add odx --env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -- '/tmp/ODX user'\''s $(example).AppImage' mcp-stdio`);
});

it("edits shared permissions inline, discards drafts and warns that saving revokes pending requests", () => {
  const update = vi.fn();
  render(<Page clients={[profile]} update={update} />);
  const form = within(screen.getByRole("form", { name: "Permissions" }));
  const actions = () => ["Save", "Discard"].map((name) => form.queryByRole("button", { name }));
  expect(actions()).toEqual([null, null]);
  expect(form.queryByText(/revokes the pending requests/)).toBeNull();
  fireEvent.click(form.getByRole("radio", { name: "Read and request writes" }));
  expect(actions().every(Boolean)).toBe(true);
  expect(form.getByText(/revokes the pending requests/)).toBeTruthy();
  fireEvent.click(form.getByRole("button", { name: "Discard" }));
  expect(form.getByRole("radio", { name: "Read and request writes" }).getAttribute("aria-checked")).toBe("false");
  expect(actions()).toEqual([null, null]);
  expect(update).not.toHaveBeenCalled();
});

it("ignores dormant environment selections when restoring unrestricted scope", () => {
  render(<Page clients={[profile]} />);
  const save = () => screen.queryByRole("button", { name: "Save" }) as HTMLButtonElement | null;
  fireEvent.click(screen.getByRole("radio", { name: "Selected only" }));
  expect(save()?.disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: /^A/ }));
  expect(save()?.disabled).toBe(false);
  fireEvent.click(screen.getByRole("radio", { name: "All, including new ones" }));
  expect(save()).toBeNull();
});

it("locks the form while saving and keeps the draft after an error", async () => {
  let fail!: (reason: unknown) => void;
  const update = vi.fn(() => new Promise<void>((_, reject) => { fail = reject; }));
  render(<Page clients={[profile]} update={update} />);
  fireEvent.click(screen.getByRole("radio", { name: "Read and request writes" }));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(update).toHaveBeenCalledWith(profile.id, profile.label, null, ["odoo:read", "odoo:write-request"]);
  expect(screen.getByRole("radio", { name: "Read and request writes" }).matches(":disabled")).toBe(true);
  fail({ code: "app.persistence" });
  expect((await screen.findByRole("alert")).textContent).toContain("Odx could not save or audit this operation.");
  expect(screen.getByRole("radio", { name: "Read and request writes" }).getAttribute("aria-checked")).toBe("true");
  await waitFor(() => expect((screen.getByRole("button", { name: "Save" }) as HTMLButtonElement).disabled).toBe(false));
});

it("warns about missing environments and shows only connector errors", () => {
  render(<Page clients={[profile]} envs={[]} services={{ ...services, mcp: { state: "error", error: "unknown" } }} />);
  expect(screen.getByText(/No environments yet/)).toBeTruthy();
  expect(screen.getByRole("alert")).toBeTruthy();
  cleanup();
  render(<Page clients={[profile]} />);
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.queryByText("active")).toBeNull();
});

it("keeps setup open and moves it below the permissions once an agent has used Odx", () => {
  render(<Page clients={[{ ...profile, last_used_at: "2026-10-17T10:00:00Z" }]} />);
  expect(document.querySelectorAll("pre")).toHaveLength(1);
  expect(screen.queryByRole("button", { name: /Connect an agent/ })).toBeNull();
  const order = [...document.querySelectorAll("[aria-labelledby='mcp-connect-title'], form[aria-label='Permissions']")].map((el) => el.tagName);
  expect(order).toEqual(["FORM", "SECTION"]);
});

it("keeps the last use time current while the page stays open", () => {
  vi.useFakeTimers({ now: Date.parse("2026-10-18T10:00:00Z") });
  try {
    render(<Page clients={[{ ...profile, last_used_at: "2026-10-18T09:55:00Z" }]} />);
    const status = () => screen.getByRole("status", { name: "Local MCP status" }).textContent;
    expect(status()).toContain("5 minutes ago");
    act(() => { vi.advanceTimersByTime(2 * 60_000); });
    expect(status()).toContain("7 minutes ago");
  } finally { vi.useRealTimers(); }
});

it("puts connecting first until an agent has used Odx", () => {
  render(<Page clients={[profile]} />);
  const order = [...document.querySelectorAll("[aria-labelledby='mcp-connect-title'], form[aria-label='Permissions']")].map((el) => el.tagName);
  expect(order).toEqual(["SECTION", "FORM"]);
  expect(screen.queryByRole("status", { name: "Local MCP status" })).toBeNull();
});

it("warns where write requests run without approval, for the whole scope or per environment", () => {
  const live = envs.map((env, i) => ({ ...env, write_mode: i ? "approval" as const : "permanent" as const }));
  render(<Page clients={[{ ...profile, capabilities: ["odoo:read", "odoo:write-request"] }]} envs={live} />);
  expect(screen.getByText("Agents can create and update without approval in A.")).toBeTruthy();
  fireEvent.click(screen.getByRole("radio", { name: "Read only" }));
  expect(screen.queryByText(/without approval/)).toBeNull();
  fireEvent.click(screen.getByRole("radio", { name: "Read and request writes" }));
  fireEvent.click(screen.getByRole("radio", { name: "Selected only" }));
  expect(within(screen.getByRole("checkbox", { name: /^A/ }).closest("label")!).getByText("No approval")).toBeTruthy();
  expect(screen.queryByText(/in A\./)).toBeNull();
});

it("adds Odx to an installed agent in one click and opens setup when a registration points elsewhere", async () => {
  vi.mocked(ipc.agentSetup).mockImplementation(async (agent) => agent === "claude" ? "missing" : "no_cli");
  vi.mocked(ipc.agentInstall).mockResolvedValue("added");
  render(<Page clients={[profile]} />);
  expect(await screen.findByText("Claude Code is installed on this computer.")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  expect(await screen.findByText("Added to Claude Code")).toBeTruthy();
  expect(ipc.agentInstall).toHaveBeenCalledWith("claude");
  fireEvent.click(screen.getByRole("radio", { name: "Codex CLI" }));
  expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
  cleanup();
  vi.mocked(ipc.agentSetup).mockImplementation(async (agent) => agent === "claude" ? "outdated" : "no_cli");
  render(<Page clients={[{ ...profile, last_used_at: "2026-10-17T10:00:00Z" }]} />);
  expect(await screen.findByRole("button", { name: "Update" })).toBeTruthy();
  expect(screen.getByText("Claude Code runs Odx from another path.")).toBeTruthy();
});
