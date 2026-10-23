import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import EnvironmentsPage from "./EnvironmentsPage";
import { ipc, type Environment } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { faviconPage: vi.fn().mockResolvedValue({ url: "https://example.test/", html: "" }), faviconImage: vi.fn().mockRejectedValue(new Error("no icon")), headers: vi.fn().mockResolvedValue({ headers: [], inherited: [] }), context: vi.fn().mockResolvedValue({ entries: [], inherited: {} }), environmentProxy: vi.fn().mockResolvedValue({ mode: "inherit", id: null }), proxies: vi.fn().mockResolvedValue({ proxies: [], default: null }), tagColors: vi.fn().mockResolvedValue({}), setTagColor: vi.fn().mockResolvedValue(undefined), saveEnvironmentProxy: vi.fn().mockResolvedValue(undefined), setTags: vi.fn().mockResolvedValue(undefined) } }));
vi.mock("./HealthChip", () => ({ HealthChip: () => null, HealthDot: () => null, checkHealth: vi.fn(), forgetHealth: vi.fn() }));
afterEach(() => {
  cleanup();
  for (const key of ["connection", "context", "headers"]) localStorage.removeItem(`odx.collapsed.${key}`);
});
const env: Environment = { id: "fixture", name: "Fixture", url: "https://example.test", database: "fixture-db", login: "fixture-user", protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true, tags: [] };
function page() {
  return <EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={vi.fn()} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} changeMode={vi.fn()} />;
}
it("starts with only write mode expanded and hides Edit in collapsed sections", async () => {
  render(page());
  expect(screen.getByRole("radiogroup", { name: "Environment write mode" })).toBeTruthy();
  for (const title of ["Connection", "Context", "HTTP headers"]) {
    expect(screen.getByRole("button", { name: `Expand ${title}` }).getAttribute("aria-expanded")).toBe("false");
  }
  expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  for (const title of ["Connection"]) {
    fireEvent.click(screen.getByRole("button", { name: `Expand ${title}` }));
    expect(within(screen.getByRole("form", { name: title })).getByRole("button", { name: "Edit" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: `Collapse ${title}` }));
    expect(within(screen.getByRole("form", { name: title })).queryByRole("button", { name: "Edit" })).toBeNull();
  }
  fireEvent.click(screen.getByRole("button", { name: "Expand Context" }));
  await screen.findByLabelText("New context key");
  expect(within(screen.getByRole("form", { name: "Environment context" })).queryByRole("button", { name: "Edit" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Collapse Context" }));
  expect(screen.queryByLabelText("New context key")).toBeNull();
});
it("preserves an explicitly expanded preference", () => {
  localStorage.setItem("odx.collapsed.connection", "0");
  render(page());
  expect(screen.getByRole("button", { name: "Collapse Connection" })).toBeTruthy();
  expect(within(screen.getByRole("form", { name: "Connection" })).getByRole("button", { name: "Edit" })).toBeTruthy();
});

// Clone, delete and rotate credentials.
type Props = Partial<Parameters<typeof EnvironmentsPage>[0]>;
function withProps(props: Props) {
  return <EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={vi.fn()} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} changeMode={vi.fn()} {...props} />;
}
const dialog = () => screen.getByRole("alertdialog");
/** Clone and delete live in the "More actions" menu. */
function menuAction(name: string) {
  fireEvent.click(screen.getByRole("button", { name: "More actions" }));
  fireEvent.click(within(screen.getByRole("menu", { name: "More actions" })).getByRole("menuitem", { name }));
}

it("clones with a new name, starting read-only without the original's permissions", async () => {
  const other: Environment = { ...env, id: "other", name: "Fixture (copy)" };
  const clone = vi.fn(async (_id: string, name: string): Promise<Environment> => ({ ...env, id: "clone-id", name, write_mode: "read_only" }));
  render(withProps({ envs: [{ ...env, write_mode: "permanent" }, other], clone }));
  menuAction("Duplicate environment");
  const text = dialog().textContent!;
  expect(text).toContain("read-only");
  expect(text).toContain("does not inherit the original permissions");
  const name = within(dialog()).getByRole("textbox", { name: "Copy name" }) as HTMLInputElement;
  // Suggest an available name; “(copy)” already exists.
  expect(name.value).toBe("Fixture (copy 2)");
  const confirm = within(dialog()).getByRole("button", { name: "Duplicate environment" }) as HTMLButtonElement;
  fireEvent.change(name, { target: { value: "Fixture (copy)" } });
  expect(confirm.disabled).toBe(true);
  expect(dialog().textContent).toContain("An environment with this name already exists.");
  fireEvent.change(name, { target: { value: "  " } });
  expect(confirm.disabled).toBe(true);
  fireEvent.change(name, { target: { value: "  Pruebas  " } });
  fireEvent.click(confirm);
  await waitFor(() => expect(clone).toHaveBeenCalledWith("fixture", "Pruebas"));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
});

it("deletes only after explicit confirmation that warns about revoked requests", async () => {
  const remove = vi.fn().mockResolvedValue(true);
  render(withProps({ remove }));
  menuAction("Delete environment");
  expect(dialog().textContent).toContain("their pending requests will be revoked");
  expect(dialog().textContent).toContain("This cannot be undone");
  expect(dialog().textContent).toContain("fixture-db");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Back" }));
  expect(remove).not.toHaveBeenCalled();
  menuAction("Delete environment");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Delete environment" }));
  await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
});

it("clones and deletes from the list's context menu", async () => {
  const remove = vi.fn().mockResolvedValue(true);
  render(withProps({ remove }));
  fireEvent.contextMenu(screen.getByRole("option", { name: /Fixture/ }), { clientX: 10, clientY: 10 });
  const menu = screen.getByRole("menu", { name: "Fixture" });
  expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Open in Terminal", "Duplicate environment", "Add another database", "Delete environment"]);
  fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete environment" }));
  expect(screen.queryByRole("menu", { name: "Fixture" })).toBeNull();
  fireEvent.click(within(dialog()).getByRole("button", { name: "Delete environment" }));
  await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
  fireEvent.contextMenu(screen.getByRole("option", { name: /Fixture/ }));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu", { name: "Fixture" })).toBeNull();
});

it("keeps the dialog open with an error when deletion fails", async () => {
  render(withProps({ remove: vi.fn().mockResolvedValue(false) }));
  menuAction("Delete environment");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Delete environment" }));
  expect((await within(dialog()).findByRole("alert")).textContent).toContain("Could not delete the environment.");
});

it("rotates the key through an empty secret field and never shows the previous one", async () => {
  const update = vi.fn().mockResolvedValue({ ...env });
  const { container } = render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  expect(screen.getByLabelText("Saved credential").textContent).toBe("••••••••••••");
  fireEvent.click(within(screen.getByRole("form", { name: "Connection" })).getByRole("button", { name: "Edit" }));
  const secret = screen.getByLabelText("API key or password") as HTMLInputElement;
  expect(secret.type).toBe("password");
  expect(secret.value).toBe("");
  expect(secret.autocomplete).toBe("new-password");
  // Saving revokes pending approvals and invalidates authentication.
  expect(screen.getByLabelText("Save warning").textContent).toContain("Pending requests for this environment are revoked");
  fireEvent.change(secret, { target: { value: "fixture-new-key" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(update.mock.calls[0][1]).toMatchObject({ secret: "fixture-new-key", name: "Fixture", database: "fixture-db" });
  // Saving removes the input and clears the new key from the page.
  await waitFor(() => expect(screen.queryByLabelText("API key or password")).toBeNull());
  expect(container.innerHTML).not.toContain("fixture-new-key");
});

it("saving other connection fields with an empty secret keeps the stored key, but a new target needs the key", async () => {
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  fireEvent.click(within(screen.getByRole("form", { name: "Connection" })).getByRole("button", { name: "Edit" }));
  const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;
  const secret = screen.getByLabelText("API key or password") as HTMLInputElement;
  fireEvent.change(screen.getByRole("textbox", { name: "User" }), { target: { value: "fixture-user-2" } });
  expect(secret.required).toBe(true);
  expect(secret.placeholder).toBe("API key required for the new URL, database or user");
  expect(save.disabled).toBe(true);
  fireEvent.change(screen.getByRole("textbox", { name: "User" }), { target: { value: "fixture-user" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Renamed" } });
  expect(secret.required).toBe(false);
  fireEvent.click(save);
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(update.mock.calls[0][1]).toMatchObject({ secret: "", name: "Renamed", login: "fixture-user" });
});

// Environment proxy.
const proxyForm = () => screen.getByRole("form", { name: "Connection" });
function editConnection() {
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
}
function chooseOption(label: string, option: string) {
  fireEvent.click(within(proxyForm()).getByRole("combobox", { name: label }));
  fireEvent.mouseDown(screen.getByRole("option", { name: option }));
}

it("ignores unchanged submissions and disables Save again when edits are reverted", async () => {
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", id: null });
  vi.mocked(ipc.saveEnvironmentProxy).mockClear();
  const update = vi.fn();
  render(withProps({ update }));
  editConnection();
  await within(proxyForm()).findByRole("combobox", { name: "Environment proxy" });
  const save = screen.getByRole("button", { name: "Save" }) as HTMLButtonElement;
  expect(save.disabled).toBe(true);
  const name = within(proxyForm()).getByRole("textbox", { name: "Name" });
  fireEvent.change(name, { target: { value: "Another name" } });
  expect(save.disabled).toBe(false);
  fireEvent.change(name, { target: { value: "  Fixture  " } });
  expect(save.disabled).toBe(true);
  chooseOption("Protocol", "XML-RPC");
  expect(save.disabled).toBe(false);
  chooseOption("Protocol", "JSON-RPC");
  expect(save.disabled).toBe(true);
  chooseOption("Environment proxy", "Direct connection");
  expect(save.disabled).toBe(false);
  chooseOption("Environment proxy", "Default (direct)");
  expect(save.disabled).toBe(true);
  fireEvent.submit(proxyForm());
  expect(update).not.toHaveBeenCalled();
  expect(ipc.saveEnvironmentProxy).not.toHaveBeenCalled();
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

const office = { id: "office", name: "Office", url: "http://proxy.example.test:3128", username: "", password: "", environments: 0 };
const vpn = { id: "vpn", name: "VPN", url: "socks5h://vpn.example.test:1080", username: "••••••••••••", password: "••••••••••••", environments: 1 };

it("shows the default proxy and switches the environment to a named proxy or direct", async () => {
  vi.mocked(ipc.saveEnvironmentProxy).mockClear();
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office, vpn], default: "office" });
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", id: null });
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  expect(await within(proxyForm()).findByText("Default (Office)")).toBeTruthy();
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
  fireEvent.click(await within(proxyForm()).findByRole("combobox", { name: "Environment proxy" }));
  expect(within(screen.getByRole("listbox", { name: "Environment proxy" })).getAllByRole("option").map((o) => o.textContent)).toEqual(["Default (Office)", "Office", "VPN", "Direct connection"]);
  fireEvent.mouseDown(screen.getByRole("option", { name: "VPN" }));
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "profile", id: "vpn" });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(ipc.saveEnvironmentProxy).toHaveBeenCalledWith("fixture", { mode: "profile", id: "vpn" }));
  expect(update).toHaveBeenCalledTimes(1);
  expect(await within(proxyForm()).findByText("VPN")).toBeTruthy();
  // Credentials are never part of the environment form.
  expect(within(proxyForm()).queryByLabelText("Proxy password")).toBeNull();
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [], default: null });
});

it("only saves the proxy when it changed", async () => {
  vi.mocked(ipc.saveEnvironmentProxy).mockClear();
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", id: null });
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  expect(await within(proxyForm()).findByText("Default (direct)")).toBeTruthy();
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
  fireEvent.change(within(proxyForm()).getByRole("textbox", { name: "Name" }), { target: { value: "Renamed" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(ipc.saveEnvironmentProxy).not.toHaveBeenCalled();
});

it("asks before saving the connection when the environment has pending requests", async () => {
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", id: null });
  const update = vi.fn().mockResolvedValue({ ...env });
  const pending = [{ id: "p1", environment_id: "fixture", environment_name: "Fixture", model: "res.partner", method: "write", args: [], kwargs: {}, context: {}, created_at: "2026-10-16T00:00:00Z", expires_at: "2026-10-16T00:00:00Z" }];
  render(withProps({ update, pending }));
  editConnection();
  // Editing before the proxy loads still shows the selector when ready.
  expect(await within(proxyForm()).findByRole("combobox", { name: "Environment proxy" })).toBeTruthy();
  fireEvent.change(within(proxyForm()).getByRole("textbox", { name: "Name" }), { target: { value: "Renamed" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  const confirm = await screen.findByRole("alertdialog");
  expect(confirm.textContent).toContain("There is 1 pending approval");
  expect(update).not.toHaveBeenCalled();
  fireEvent.click(within(confirm).getByRole("button", { name: "Save and revoke" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
});

it("keeps the environment being edited when the filter excludes it", () => {
  localStorage.setItem("odx.collapsed.connection", "0");
  const other: Environment = { ...env, id: "other", name: "Other", database: "other-db" };
  render(withProps({ envs: [env, other] }));
  fireEvent.click(within(screen.getByRole("form", { name: "Connection" })).getByRole("button", { name: "Edit" }));
  fireEvent.change(screen.getByRole("textbox", { name: "Name" }), { target: { value: "Renamed" } });
  fireEvent.change(screen.getByRole("combobox", { name: "Filter environments" }), { target: { value: "other" } });
  expect((screen.getByRole("textbox", { name: "Name" }) as HTMLInputElement).value).toBe("Renamed");
  fireEvent.change(screen.getByRole("combobox", { name: "Filter environments" }), { target: { value: "nothing" } });
  expect(screen.getByText("No matching environments")).toBeTruthy();
  expect((screen.getByRole("textbox", { name: "Name" }) as HTMLInputElement).value).toBe("Renamed");
});

it("filters by any of the chosen write modes and shows them as chips", () => {
  const envs: Environment[] = (["read_only", "approval", "temporary", "permanent"] as const).map((write_mode) => ({ ...env, id: write_mode, name: write_mode, write_mode }));
  render(withProps({ envs }));
  const list = screen.getByRole("listbox", { name: "Environments" });
  fireEvent.click(screen.getByRole("button", { name: "Filters" }));
  const modes = within(screen.getByRole("dialog", { name: "Filters" })).getByRole("list", { name: "Filter by write mode" });
  expect(within(modes).getAllByRole("checkbox").map((box) => box.closest("label")!.textContent)).toEqual(["Read-only1", "Approval required1", "Temporary writes1", "Permanent writes1"]);
  fireEvent.click(within(modes).getByRole("checkbox", { name: /Temporary/ }));
  fireEvent.click(within(modes).getByRole("checkbox", { name: /Permanent/ }));
  expect(within(list).getAllByRole("option").map((o) => o.querySelector("strong")!.textContent)).toEqual(["permanent", "temporary"]);
  fireEvent.click(screen.getByRole("button", { name: "Remove filter Permanent writes" }));
  expect(within(list).getAllByRole("option")).toHaveLength(1);
  fireEvent.keyDown(screen.getByRole("combobox", { name: "Filter environments" }), { key: "Backspace" });
  expect(within(list).getAllByRole("option")).toHaveLength(4);
});

it("adds tag filters from the search with # and from the list, and removes them with Backspace", () => {
  const a: Environment = { ...env, id: "a", name: "Alpha", tags: ["prod", "Cliente A"] };
  const b: Environment = { ...env, id: "b", name: "Beta", tags: ["staging", "Cliente A"] };
  render(withProps({ envs: [a, b] }));
  const list = screen.getByRole("listbox", { name: "Environments" });
  const search = screen.getByRole("combobox", { name: "Filter environments" });
  fireEvent.change(search, { target: { value: "#st" } });
  // A tag query does not filter the list by its text.
  expect(within(list).getAllByRole("option")).toHaveLength(2);
  const suggestions = screen.getByRole("listbox", { name: "Filter by tag" });
  expect(within(suggestions).getAllByRole("option").map((o) => o.textContent)).toEqual(["staging"]);
  fireEvent.keyDown(search, { key: "Enter" });
  expect((search as HTMLInputElement).value).toBe("");
  expect(screen.queryByRole("listbox", { name: "Filter by tag" })).toBeNull();
  expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual([expect.stringContaining("Beta")]);
  expect(screen.getByRole("status").textContent).toContain("1 of 2 environments");
  fireEvent.keyDown(search, { key: "Backspace" });
  expect(within(list).getAllByRole("option")).toHaveLength(2);
  fireEvent.click(within(within(list).getAllByRole("option")[0]).getByText("prod"));
  expect(screen.getByRole("button", { name: "Remove filter prod" })).toBeTruthy();
  expect(within(list).getAllByRole("option")).toHaveLength(1);
});

it("filters by tags and edits the selected environment's tags", async () => {
  const a: Environment = { ...env, id: "a", name: "Alpha", tags: ["prod", "Cliente A"] };
  const b: Environment = { ...env, id: "b", name: "Beta", tags: ["staging", "Cliente A"] };
  const refresh = vi.fn().mockResolvedValue(undefined);
  render(withProps({ envs: [a, b], refresh }));
  const list = screen.getByRole("listbox", { name: "Environments" });
  const filterButton = screen.getByRole("button", { name: "Filters" });
  fireEvent.click(filterButton);
  const panel = screen.getByRole("dialog", { name: "Filters" });
  const tagBoxes = within(within(panel).getByRole("list", { name: "Filter by tag" })).getAllByRole("checkbox");
  expect(tagBoxes.map((box) => box.closest("label")!.textContent)).toEqual(["Cliente A2", "prod1", "staging1"]);
  fireEvent.click(within(panel).getByRole("checkbox", { name: /Cliente A/ }));
  expect(within(list).getAllByRole("option")).toHaveLength(2);
  fireEvent.click(within(panel).getByRole("checkbox", { name: /staging/ }));
  expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual([expect.stringContaining("Beta")]);
  expect(filterButton.textContent).toContain("2");
  // Escape closes the panel; active filters stay removable from the toolbar.
  fireEvent.keyDown(panel, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Filters" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Remove filter staging" }));
  expect(within(list).getAllByRole("option")).toHaveLength(2);
  fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
  expect(within(list).getAllByRole("option")).toHaveLength(2);
  expect(screen.queryByRole("button", { name: "Remove filter Cliente A" })).toBeNull();
  // Text search also matches tags.
  fireEvent.change(screen.getByRole("combobox", { name: "Filter environments" }), { target: { value: "staging" } });
  expect(within(list).getAllByRole("option")).toHaveLength(1);

  fireEvent.click(screen.getByRole("button", { name: "Remove tag prod" }));
  await waitFor(() => expect(ipc.setTags).toHaveBeenCalledWith("a", ["Cliente A"]));
  await waitFor(() => expect(refresh).toHaveBeenCalled());
});

it("summarizes and clears filters, shows host and tags, and moves with arrow keys", () => {
  const envs: Environment[] = ["Gamma", "Alpha", "Beta"].map((name) => ({ ...env, id: name, name, url: `https://${name.toLowerCase()}.example.test`, tags: name === "Beta" ? ["a", "b", "c", "d"] : [] }));
  render(withProps({ envs }));
  const list = screen.getByRole("listbox", { name: "Environments" });
  const options = () => within(list).getAllByRole("option");
  expect(options().map((o) => o.querySelector("strong")!.textContent)).toEqual(["Alpha", "Beta", "Gamma"]);
  expect(options()[1].textContent).toContain("beta.example.test · fixture-db");
  expect(options()[1].textContent).toContain("+1");
  expect(options().map((o) => o.tabIndex)).toEqual([0, -1, -1]);
  options()[0].focus();
  fireEvent.keyDown(options()[0], { key: "ArrowDown" });
  expect(document.activeElement).toBe(options()[1]);
  expect(options()[1].getAttribute("aria-selected")).toBe("true");

  const search = screen.getByRole("combobox", { name: "Filter environments" });
  fireEvent.change(search, { target: { value: "zzz" } });
  expect(screen.getByRole("status").textContent).toContain("0 of 3 environments");
  fireEvent.click(within(list).getByRole("button", { name: "Clear filters" }));
  expect((search as HTMLInputElement).value).toBe("");
  fireEvent.change(search, { target: { value: "gam" } });
  fireEvent.keyDown(search, { key: "Escape" });
  expect((search as HTMLInputElement).value).toBe("");
});

it("resizes the list with the keyboard, clamps and remembers the width", () => {
  localStorage.removeItem("odx.environments.listWidth");
  render(withProps({}));
  const handle = screen.getByRole("separator", { name: "Resize environment list" });
  expect(handle.getAttribute("aria-valuenow")).toBe("300");
  fireEvent.keyDown(handle, { key: "ArrowRight", shiftKey: true });
  expect(handle.getAttribute("aria-valuenow")).toBe("364");
  fireEvent.keyDown(handle, { key: "End" });
  expect(handle.getAttribute("aria-valuenow")).toBe("520");
  fireEvent.keyDown(handle, { key: "ArrowRight" });
  expect(handle.getAttribute("aria-valuenow")).toBe("520");
  expect(localStorage.getItem("odx.environments.listWidth")).toBe("520");
  cleanup();
  render(withProps({}));
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("520");
  fireEvent.doubleClick(screen.getByRole("separator"));
  expect(screen.getByRole("separator").getAttribute("aria-valuenow")).toBe("300");
  localStorage.removeItem("odx.environments.listWidth");
});

it("focuses the search with Ctrl+F or / and starts a new environment with Ctrl+N", () => {
  const setCreating = vi.fn();
  render(withProps({ setCreating }));
  const search = screen.getByRole("combobox", { name: "Filter environments" });
  fireEvent.keyDown(window, { key: "f", ctrlKey: true });
  expect(document.activeElement).toBe(search);
  search.blur();
  fireEvent.keyDown(document.body, { key: "/" });
  expect(document.activeElement).toBe(search);
  // Spanish layouts type "/" with Shift+7.
  search.blur();
  fireEvent.keyDown(document.body, { key: "/", shiftKey: true });
  expect(document.activeElement).toBe(search);
  // While typing, / is just a character.
  fireEvent.change(search, { target: { value: "a" } });
  const event = new KeyboardEvent("keydown", { key: "/", bubbles: true, cancelable: true });
  search.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(false);
  fireEvent.keyDown(window, { key: "n", ctrlKey: true });
  expect(setCreating).toHaveBeenCalledWith(true);
});

it("keeps quick tag edits and treats tags differing in case as one", async () => {
  const a: Environment = { ...env, id: "a", name: "Alpha", tags: ["Prod", "x", "y"] };
  const b: Environment = { ...env, id: "b", name: "Beta", tags: ["prod"] };
  vi.mocked(ipc.setTags).mockClear();
  // Saves never finish, so props keep the old tags.
  vi.mocked(ipc.setTags).mockReturnValue(new Promise(() => {}));
  render(withProps({ envs: [a, b] }));
  fireEvent.click(screen.getByRole("button", { name: "Remove tag x" }));
  fireEvent.click(screen.getByRole("button", { name: "Remove tag y" }));
  expect(vi.mocked(ipc.setTags).mock.calls.at(-1)).toEqual(["a", ["Prod"]]);
  fireEvent.click(screen.getByRole("button", { name: "Filters" }));
  const box = screen.getByRole("checkbox", { name: /Prod/ });
  expect(box.closest("label")!.textContent).toBe("Prod2");
  fireEvent.click(box);
  expect(within(screen.getByRole("listbox", { name: "Environments" })).getAllByRole("option")).toHaveLength(2);
  vi.mocked(ipc.setTags).mockResolvedValue(undefined as never);
});

it("opens with the requested environment selected", () => {
  const other: Environment = { ...env, id: "other", name: "Other" };
  render(withProps({ envs: [env, other], focusId: "other" }));
  expect(screen.getByRole("option", { name: /Other/ }).getAttribute("aria-selected")).toBe("true");
});

it("keeps the create form and its draft when asked to show an environment, and selects it on cancel", () => {
  const other: Environment = { ...env, id: "other", name: "Other" };
  const setCreating = vi.fn();
  const { rerender } = render(withProps({ envs: [env, other], creating: true, setCreating }));
  rerender(withProps({ envs: [env, other], creating: true, setCreating, focusId: "other" }));
  expect(screen.getByRole("status").textContent).toBe("Finish or cancel the new environment to show Other.");
  expect(screen.getByRole("heading", { name: "New environment" })).toBeTruthy();
  expect(setCreating).not.toHaveBeenCalled();
  rerender(withProps({ envs: [env, other], creating: false, setCreating, focusId: "other" }));
  expect(screen.queryByText(/Finish or cancel/)).toBeNull();
  expect(screen.getByRole("option", { name: /Other/ }).getAttribute("aria-selected")).toBe("true");
});

it("groups databases of one server and adds another with its URL, user and proxy but no API key", async () => {
  // The detail pane reads the proxy too, so a one-off mock would be spent there.
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "profile", id: "vpn" });
  const sibling: Environment = { ...env, id: "sibling", name: "Sibling", database: "sibling-db", login: "other-user" };
  const lone: Environment = { ...env, id: "lone", name: "Lone", url: "https://lone.test" };
  const setCreating = vi.fn();
  const draftRef: Parameters<typeof EnvironmentsPage>[0]["draftRef"] = { current: null };
  render(withProps({ envs: [env, sibling, lone], setCreating, draftRef }));
  const group = screen.getByRole("group", { name: "example.test" });
  expect(within(group).getAllByRole("option").map((o) => o.dataset.id)).toEqual(["fixture", "sibling"]);
  expect(within(group).getByText("sibling-db · other-user")).toBeTruthy();
  expect(screen.queryByRole("group", { name: "lone.test" })).toBeNull();
  fireEvent.click(within(group).getByRole("button", { name: /example\.test/ }));
  expect(within(group).queryAllByRole("option")).toHaveLength(0);
  fireEvent.click(within(group).getByRole("button", { name: "Add another database" }));
  await waitFor(() => expect(setCreating).toHaveBeenCalledWith(true));
  expect(draftRef!.current).toEqual({ values: { name: "", url: env.url, database: "", login: env.login, secret: "", protocol: env.protocol }, tags: [], proxy: "p:vpn" });
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", id: null });
});

it("acts on the focused environment with Delete, F2 and Ctrl+D", async () => {
  render(withProps({}));
  const option = () => screen.getByRole("option", { name: /Fixture/ });
  fireEvent.keyDown(option(), { key: "Delete" });
  expect(dialog().textContent).toContain("This cannot be undone");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Back" }));
  fireEvent.keyDown(option(), { key: "d", ctrlKey: true });
  expect((within(dialog()).getByRole("textbox") as HTMLInputElement).value).toContain("Fixture");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Back" }));
  fireEvent.keyDown(option(), { key: "F2" });
  await waitFor(() => expect(document.activeElement?.getAttribute("aria-label")).toBe("Name"));
});

it("creating the first environment leaves the list empty instead of saying nothing matches", () => {
  render(withProps({ envs: [], creating: true }));
  expect(screen.queryByText("No matches")).toBeNull();
});
