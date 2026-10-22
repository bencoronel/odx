import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import EnvironmentForm from "./EnvironmentForm";
import EnvironmentsPage from "./EnvironmentsPage";
import { ipc, type Environment } from "./ipc";
import { i18n } from "./i18n";
vi.mock("./ipc", () => ({ ipc: { faviconPage: vi.fn().mockResolvedValue({ url: "https://example.test/", html: "" }), faviconImage: vi.fn().mockRejectedValue(new Error("no icon")), headers: vi.fn().mockResolvedValue({ headers: [], inherited: [] }), context: vi.fn().mockResolvedValue({ entries: [], inherited: {} }), environmentProxy: vi.fn().mockResolvedValue({ mode: "inherit", id: null }), proxies: vi.fn().mockResolvedValue({ proxies: [], default: null }), tagColors: vi.fn().mockResolvedValue({}), setTagColor: vi.fn().mockResolvedValue(undefined), databases: vi.fn().mockResolvedValue(["prod", "staging"]), checkDraft: vi.fn() } }));
vi.mock("./HealthChip", () => ({ HealthChip: () => null, HealthDot: () => null, checkHealth: vi.fn(), forgetHealth: vi.fn() }));
afterEach(async () => { cleanup(); await i18n.changeLanguage("en"); localStorage.removeItem("odx.collapsed.connection"); });

it("creates with automatic detection without opening advanced options", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={save} cancel={vi.fn()} />);
  expect(screen.getByText("Advanced options").closest("details")!.open).toBe(false);
  for (const [label, value] of [[/^Name/, "Dev"], [/^Odoo URL/, "https://odoo.example.test"], [/^Database/, "db"], [/^User/, "user"], [/^API key or password/, "fixture-secret"]] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Create environment" }));
  await vi.waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0]).toMatchObject({ protocol: "auto", context: {} });
});

it("creates the environment with the selected protocol", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={save} cancel={vi.fn()} />);
  expect(screen.queryByRole("heading", { name: "Context" })).toBeNull();
  expect(screen.queryByLabelText("Environment context")).toBeNull();
  fireEvent.click(screen.getByText("Advanced options"));
  const protocol = screen.getByRole("combobox", { name: "Protocol" });
  expect(protocol.textContent).toBe("Automatic");
  fireEvent.click(protocol);
  expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Automatic", "JSON-RPC", "XML-RPC", "JSON-2"]);
  fireEvent.keyDown(protocol, { key: "Escape" });
  for (const [label, value] of [[/^Name/, "Dev"], [/^Odoo URL/, "https://odoo.example.test"], [/^Database/, "db"], [/^User/, "user"], [/^API key or password/, "fixture-secret"]] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  fireEvent.click(protocol);
  fireEvent.mouseDown(screen.getByRole("option", { name: "JSON-2" }));
  fireEvent.click(screen.getByRole("button", { name: "Create environment" }));
  await vi.waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][0]).toMatchObject({ name: "Dev", protocol: "json2", context: {} });
});

it("changes the protocol when editing the connection", async () => {
  localStorage.setItem("odx.collapsed.connection", "0");
  const env: Environment = { id: "fixture", name: "Fixture", url: "https://example.test", database: "fixture-db", login: "fixture-user", protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true, tags: [] };
  const update = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={update} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} changeMode={vi.fn()} />);
  const form = screen.getByRole("form", { name: "Connection" });
  fireEvent.click(within(form).getByRole("button", { name: "Edit" }));
  expect(within(form).queryByText("Advanced options")).toBeNull();
  fireEvent.click(within(form).getByRole("combobox", { name: "Protocol" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "XML-RPC" }));
  fireEvent.click(within(form).getByRole("button", { name: "Save" }));
  await vi.waitFor(() => expect(update).toHaveBeenCalledOnce());
  expect(update.mock.calls[0]).toEqual(["fixture", expect.objectContaining({ protocol: "xmlrpc", secret: "" })]);
});

it("loads databases on request, tests without a name, suggests the name and saves tags", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentForm busy={false} tagSuggestions={["Cliente A"]} save={save} cancel={vi.fn()} />);
  const url = screen.getByLabelText(/^Odoo URL/);
  expect(document.activeElement).toBe(url);
  fireEvent.change(url, { target: { value: "https://odoo.example.test" } });
  fireEvent.blur(url);
  // Leaving the URL suggests a name but sends nothing.
  expect((screen.getByLabelText(/^Name/) as HTMLInputElement).value).toBe("odoo.example.test");
  expect(ipc.databases).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Load databases from the server" }));
  await vi.waitFor(() => expect(ipc.databases).toHaveBeenCalledWith("https://odoo.example.test"));
  const database = screen.getByRole("combobox", { name: "Database" });
  fireEvent.focus(database);
  fireEvent.mouseDown(await screen.findByRole("option", { name: "staging" }));
  expect((database as HTMLInputElement).value).toBe("staging");
  // Free text is still accepted.
  fireEvent.change(database, { target: { value: "other-db" } });
  fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "" } });
  for (const [label, value] of [[/^User/, "user"], [/^API key or password/, "fixture-secret"]] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  vi.mocked(ipc.checkDraft).mockResolvedValueOnce({ environment_id: "draft", status: "ok", server_version: "17.0", latency_ms: 42 });
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
  expect((await screen.findByRole("status")).textContent).toContain("Odoo 17.0 · 42 ms");
  expect(ipc.checkDraft).toHaveBeenCalledWith(expect.objectContaining({ name: "draft", database: "other-db", secret: "fixture-secret" }), { mode: "inherit" });
  // Without a name, Create lets the browser flag the missing field instead of staying disabled.
  const create = screen.getByRole("button", { name: "Create environment" }) as HTMLButtonElement;
  expect(create.disabled).toBe(false);
  fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "Dev" } });
  const tag = screen.getByRole("combobox", { name: "Add tag" });
  fireEvent.change(tag, { target: { value: "prod" } });
  fireEvent.keyDown(tag, { key: "Enter" });
  fireEvent.focus(tag);
  fireEvent.mouseDown(screen.getByRole("option", { name: "Cliente A" }));
  fireEvent.click(create);
  await vi.waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save.mock.calls[0][1]).toEqual(["prod", "Cliente A"]);
  expect(save.mock.calls[0][0]).toMatchObject({ name: "Dev", database: "other-db" });
});

it("ignores a database list that arrives for an older URL", async () => {
  let answer!: (names: string[]) => void;
  vi.mocked(ipc.databases).mockReturnValueOnce(new Promise((resolve) => { answer = resolve; }));
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={vi.fn()} cancel={vi.fn()} />);
  const url = screen.getByLabelText(/^Odoo URL/);
  fireEvent.change(url, { target: { value: "https://old.example.test" } });
  fireEvent.click(screen.getByRole("button", { name: "Load databases from the server" }));
  fireEvent.change(url, { target: { value: "https://new.example.test" } });
  answer(["old-db"]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  fireEvent.focus(screen.getByRole("combobox", { name: "Database" }));
  expect(screen.queryByRole("option", { name: "old-db" })).toBeNull();
});

it("restores what was typed after being unmounted", () => {
  const draftRef = { current: null };
  const { unmount } = render(<EnvironmentForm busy={false} tagSuggestions={[]} save={vi.fn()} cancel={vi.fn()} draftRef={draftRef} />);
  fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "Dev" } });
  unmount();
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={vi.fn()} cancel={vi.fn()} draftRef={draftRef} />);
  expect((screen.getByLabelText(/^Name/) as HTMLInputElement).value).toBe("Dev");
});

it("prefills the user and database defaults from settings", () => {
  localStorage.setItem("odx.environmentDefaults", JSON.stringify({ login: "admin", database: "" }));
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={vi.fn()} cancel={vi.fn()} />);
  localStorage.removeItem("odx.environmentDefaults");
  expect(screen.getByLabelText<HTMLInputElement>(/^User/).value).toBe("admin");
  expect(screen.getByLabelText<HTMLInputElement>(/^Database/).value).toBe("");
});

it("creates and tests the environment through the proxy chosen in the form", async () => {
  vi.mocked(ipc.proxies).mockResolvedValueOnce({ proxies: [{ id: "vpn", name: "VPN", url: "socks5h://vpn.test:1080", username: "", password: "", environments: 0 }], default: null });
  vi.mocked(ipc.checkDraft).mockResolvedValueOnce({ environment_id: "draft", status: "ok", server_version: "17.0", latency_ms: 42 });
  const save = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentForm busy={false} tagSuggestions={[]} save={save} cancel={vi.fn()} />);
  const proxy = await screen.findByRole("combobox", { name: "Environment proxy" });
  expect(proxy.textContent).toBe("Default (direct)");
  fireEvent.click(proxy);
  fireEvent.mouseDown(screen.getByRole("option", { name: "VPN" }));
  for (const [label, value] of [[/^Name/, "Dev"], [/^Odoo URL/, "https://odoo.example.test"], [/^Database/, "db"], [/^User/, "user"], [/^API key or password/, "fixture-secret"]] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  fireEvent.click(screen.getByRole("button", { name: "Test connection" }));
  await vi.waitFor(() => expect(ipc.checkDraft).toHaveBeenLastCalledWith(expect.objectContaining({ url: "https://odoo.example.test" }), { mode: "profile", id: "vpn" }));
  fireEvent.click(screen.getByRole("button", { name: "Create environment" }));
  await vi.waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ name: "Dev" }), [], { mode: "profile", id: "vpn" }));
});
