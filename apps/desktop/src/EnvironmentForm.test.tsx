import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import EnvironmentForm from "./EnvironmentForm";
import EnvironmentsPage from "./EnvironmentsPage";
import type { Environment } from "./ipc";
import { i18n } from "./i18n";
vi.mock("./ipc", () => ({ ipc: { faviconPage: vi.fn().mockResolvedValue({ url: "https://example.test/", html: "" }), faviconImage: vi.fn().mockRejectedValue(new Error("no icon")), headers: vi.fn().mockResolvedValue({ headers: [], inherited: [] }), context: vi.fn().mockResolvedValue({ entries: [], inherited: {} }), environmentProxy: vi.fn().mockResolvedValue({ mode: "inherit", proxy: null, inherited: null }) } }));
vi.mock("./HealthChip", () => ({ HealthChip: () => null, HealthDot: () => null, checkHealth: vi.fn(), forgetHealth: vi.fn() }));
afterEach(async () => { cleanup(); await i18n.changeLanguage("en"); localStorage.removeItem("odx.collapsed.connection"); });

it("creates with automatic detection without opening advanced options", async () => {
  const save = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentForm busy={false} save={save} cancel={vi.fn()} />);
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
  render(<EnvironmentForm busy={false} save={save} cancel={vi.fn()} />);
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
  const env: Environment = { id: "fixture", name: "Fixture", url: "https://example.test", database: "fixture-db", login: "fixture-user", protocol: "legacy_jsonrpc", context: {}, write_mode: "read_only", has_secret: true };
  const update = vi.fn().mockResolvedValue(undefined);
  render(<EnvironmentsPage refresh={vi.fn().mockResolvedValue(undefined)} envs={[env]} busy={false} creating={false} setCreating={vi.fn()} save={vi.fn()} update={update} remove={vi.fn()} clone={vi.fn()} openShell={vi.fn()} writableOnly={false} setWritableOnly={vi.fn()} changeMode={vi.fn()} />);
  const form = screen.getByRole("form", { name: "Connection" });
  fireEvent.click(within(form).getByRole("button", { name: "Edit" }));
  expect(within(form).queryByText("Advanced options")).toBeNull();
  fireEvent.click(within(form).getByRole("combobox", { name: "Protocol" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "XML-RPC" }));
  fireEvent.click(within(form).getByRole("button", { name: "Save" }));
  await vi.waitFor(() => expect(update).toHaveBeenCalledOnce());
  expect(update.mock.calls[0]).toEqual(["fixture", expect.objectContaining({ protocol: "xmlrpc", secret: "" })]);
});
