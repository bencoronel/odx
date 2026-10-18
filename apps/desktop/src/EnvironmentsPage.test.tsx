import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import EnvironmentsPage from "./EnvironmentsPage";
import { ipc, type Environment } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { faviconPage: vi.fn().mockResolvedValue({ url: "https://example.test/", html: "" }), faviconImage: vi.fn().mockRejectedValue(new Error("no icon")), headers: vi.fn().mockResolvedValue({ headers: [], inherited: [] }), context: vi.fn().mockResolvedValue({ entries: [], inherited: {} }), environmentProxy: vi.fn().mockResolvedValue({ mode: "inherit", proxy: null, inherited: null }), saveEnvironmentProxy: vi.fn().mockResolvedValue(undefined) } }));
vi.mock("./HealthChip", () => ({ HealthChip: () => null, HealthDot: () => null, checkHealth: vi.fn(), forgetHealth: vi.fn() }));
afterEach(() => {
  cleanup();
  for (const key of ["connection", "context", "headers"]) localStorage.removeItem(`odx.collapsed.${key}`);
});
const env: Environment = { id: "fixture", name: "Fixture", url: "https://example.test", database: "fixture-db", login: "fixture-user", protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true };
function page() {
  return <EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={vi.fn()} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} writableOnly={false} setWritableOnly={vi.fn()} changeMode={vi.fn()} />;
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
  return <EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={vi.fn()} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} writableOnly={false} setWritableOnly={vi.fn()} changeMode={vi.fn()} {...props} />;
}
const dialog = () => screen.getByRole("alertdialog");

it("clones with a new name, starting read-only without the original's permissions", async () => {
  const other: Environment = { ...env, id: "other", name: "Fixture (copy)" };
  const clone = vi.fn(async (_id: string, name: string): Promise<Environment> => ({ ...env, id: "clone-id", name, write_mode: "read_only" }));
  render(withProps({ envs: [{ ...env, write_mode: "permanent" }, other], clone }));
  fireEvent.click(screen.getByRole("button", { name: "Duplicate environment" }));
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
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  expect(dialog().textContent).toContain("their pending requests will be revoked");
  expect(dialog().textContent).toContain("This cannot be undone");
  expect(dialog().textContent).toContain("fixture-db");
  fireEvent.click(within(dialog()).getByRole("button", { name: "Back" }));
  expect(remove).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  fireEvent.click(within(dialog()).getByRole("button", { name: "Delete environment" }));
  await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
});

it("keeps the dialog open with an error when deletion fails", async () => {
  render(withProps({ remove: vi.fn().mockResolvedValue(false) }));
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
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

it("saving other connection fields with an empty secret keeps the stored key", async () => {
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  fireEvent.click(within(screen.getByRole("form", { name: "Connection" })).getByRole("button", { name: "Edit" }));
  fireEvent.change(screen.getByRole("textbox", { name: "User" }), { target: { value: "fixture-user-2" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(update.mock.calls[0][1]).toMatchObject({ secret: "", login: "fixture-user-2" });
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
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", proxy: null, inherited: null });
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
  chooseOption("Environment proxy", "Direct");
  expect(save.disabled).toBe(false);
  chooseOption("Environment proxy", "Inherit (no global proxy (direct))");
  expect(save.disabled).toBe(true);
  fireEvent.submit(proxyForm());
  expect(update).not.toHaveBeenCalled();
  expect(ipc.saveEnvironmentProxy).not.toHaveBeenCalled();
  expect(screen.queryByRole("alertdialog")).toBeNull();
});

it("shows which proxy is inherited and switches the environment to direct", async () => {
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", proxy: null, inherited: "http://proxy.example.test:3128" });
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  expect((await within(proxyForm()).findByText("Inherit · http://proxy.example.test:3128"))).toBeTruthy();
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
  fireEvent.click(within(proxyForm()).getByRole("combobox", { name: "Environment proxy" }));
  expect(within(screen.getByRole("listbox", { name: "Environment proxy" })).getAllByRole("option").map((o) => o.textContent)).toEqual(["Inherit (http://proxy.example.test:3128)", "Custom", "Direct"]);
  fireEvent.mouseDown(screen.getByRole("option", { name: "Direct" }));
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "direct", proxy: null, inherited: "http://proxy.example.test:3128" });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(ipc.saveEnvironmentProxy).toHaveBeenCalledWith("fixture", { mode: "direct" }));
  expect(update).toHaveBeenCalledTimes(1);
  expect(await within(proxyForm()).findByText("Direct")).toBeTruthy();
});

it("configures an own proxy with masked credentials and only saves the proxy when it changed", async () => {
  vi.mocked(ipc.saveEnvironmentProxy).mockClear();
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", proxy: null, inherited: null });
  const update = vi.fn().mockResolvedValue({ ...env });
  render(withProps({ update }));
  fireEvent.click(screen.getByRole("button", { name: "Expand Connection" }));
  expect(await within(proxyForm()).findByText("Inherit · no global proxy (direct)")).toBeTruthy();
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
  // An unchanged proxy only saves the edited connection.
  fireEvent.change(within(proxyForm()).getByRole("textbox", { name: "User" }), { target: { value: "fixture-user-2" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  expect(ipc.saveEnvironmentProxy).not.toHaveBeenCalled();
  await waitFor(() => expect(within(proxyForm()).queryByRole("combobox", { name: "Environment proxy" })).toBeNull());
  fireEvent.click(within(proxyForm()).getByRole("button", { name: "Edit" }));
  chooseOption("Environment proxy", "Custom");
  chooseOption("Proxy type", "SOCKS5");
  fireEvent.change(within(proxyForm()).getByLabelText("Proxy host"), { target: { value: "own.example.test" } });
  fireEvent.change(within(proxyForm()).getByLabelText("Proxy username"), { target: { value: "fixture-proxy-user" } });
  fireEvent.change(within(proxyForm()).getByLabelText("Proxy password"), { target: { value: "fixture-proxy-pass" } });
  expect((within(proxyForm()).getByLabelText("Proxy password") as HTMLInputElement).type).toBe("password");
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "custom", proxy: { url: "socks5h://own.example.test:1080", username: "••••••••••••", password: "••••••••••••" }, inherited: null });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(ipc.saveEnvironmentProxy).toHaveBeenCalledWith("fixture", { mode: "custom", url: "socks5h://own.example.test:1080", username: "fixture-proxy-user", password: "fixture-proxy-pass" }));
  expect(await within(proxyForm()).findByText("Custom · socks5h://own.example.test:1080")).toBeTruthy();
  expect(document.body.innerHTML).not.toContain("fixture-proxy-pass");
  expect(document.body.innerHTML).not.toContain("fixture-proxy-user");
});

it("asks before saving the connection when the environment has pending requests", async () => {
  vi.mocked(ipc.environmentProxy).mockResolvedValue({ mode: "inherit", proxy: null, inherited: null });
  const update = vi.fn().mockResolvedValue({ ...env });
  const pending = [{ id: "p1", environment_id: "fixture", environment_name: "Fixture", model: "res.partner", method: "write", args: [], kwargs: {}, context: {}, expires_at: "2026-10-16T00:00:00Z" }];
  render(withProps({ update, pending }));
  editConnection();
  // Editing before the proxy loads still shows the selector when ready.
  expect(await within(proxyForm()).findByRole("combobox", { name: "Environment proxy" })).toBeTruthy();
  fireEvent.change(within(proxyForm()).getByRole("textbox", { name: "User" }), { target: { value: "fixture-user-2" } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  const confirm = await screen.findByRole("alertdialog");
  expect(confirm.textContent).toContain("There is 1 pending approval");
  expect(update).not.toHaveBeenCalled();
  fireEvent.click(within(confirm).getByRole("button", { name: "Save and revoke" }));
  await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
});
