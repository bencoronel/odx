import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import SettingsTable from "./SettingsTable";
import { ipc } from "./ipc";
import { formatContextValue, parseContextValue } from "./contextValues";
vi.mock("./ipc", () => ({ ipc: { context: vi.fn(), saveContext: vi.fn() } }));
afterEach(() => { cleanup(); vi.resetAllMocks(); localStorage.removeItem("odx.collapsed.context"); });

it("round-trips context value types, including strings that look like JSON", () => {
  for (const value of ["es_ES", "false", "3", "[unfinished", "", "   ", false, true, null, 3, [1,2], { nested: [false] }]) {
    expect(parseContextValue(formatContextValue(value))).toEqual(value);
  }
  expect(() => parseContextValue("[1,")).toThrow();
  expect(() => parseContextValue("[1,")).toThrow(expect.objectContaining({ code: "validation.context.json_invalid" }));
});
it("edits global context in the shared table and sends typed entries", async () => {
  vi.mocked(ipc.context).mockResolvedValue({ entries: [], inherited: {} });
  vi.mocked(ipc.saveContext).mockResolvedValue(undefined);
  render(<SettingsTable kind="context" />);
  await screen.findByLabelText("New context key");
  expect(screen.queryByRole("button", { name: /Hidden|Save|Edit|Cancel/ })).toBeNull();
  expect(screen.queryByDisplayValue("Host")).toBeNull();
  const values = [["lang", "es_ES", "es_ES"], ["active_test", "false", false], ["allowed_company_ids", "[1,2]", [1,2]], ["number", "3", 3], ["data", '{"nested":true}', { nested: true }], ["literal", '"false"', "false"]] as const;
  for (const [i, [name, text]] of values.entries()) {
    fireEvent.change(screen.getByLabelText("New context key"), { target: { value: name } });
    const input = screen.getByLabelText(`Context value ${i + 1}`);
    fireEvent.change(input, { target: { value: text } });
    fireEvent.blur(input);
    await waitFor(() => expect(ipc.saveContext).toHaveBeenCalledTimes(i + 1));
  }
  expect(ipc.saveContext).toHaveBeenLastCalledWith(values.map(([name, , value]) => ({ name, value, enabled: true })), undefined);
});
it("uses read-only global context as a fallback and keeps invalid local values unsaved", async () => {
  localStorage.setItem("odx.collapsed.context", "0");
  vi.mocked(ipc.context).mockResolvedValue({ entries: [], inherited: { allowed_company_ids: [1,2], lang: "es_ES" } });
  vi.mocked(ipc.saveContext).mockResolvedValue(undefined);
  render(<SettingsTable kind="context" environmentId="fixture-env" />);
  const key = await screen.findByLabelText("New context key");
  fireEvent.click(screen.getByRole("button", { name: "Hidden (2)" }));
  expect((screen.getByLabelText("Inherited context value allowed_company_ids") as HTMLInputElement).readOnly).toBe(true);
  fireEvent.change(key, { target: { value: "allowed_company_ids" } });
  const value = screen.getByLabelText("Context value 1");
  fireEvent.change(value, { target: { value: "[2," } });
  fireEvent.blur(value);
  await screen.findByRole("alert");
  expect(ipc.saveContext).not.toHaveBeenCalled();
  expect((value as HTMLInputElement).value).toBe("[2,");
  fireEvent.change(value, { target: { value: "[2]" } });
  fireEvent.blur(value);
  await waitFor(() => expect(ipc.saveContext).toHaveBeenCalledWith([{ name: "allowed_company_ids", value: [2], enabled: true }], "fixture-env"));
  fireEvent.click(screen.getByLabelText("Enable context 1"));
  await waitFor(() => expect(ipc.saveContext).toHaveBeenLastCalledWith([{ name: "allowed_company_ids", value: [2], enabled: false }], "fixture-env"));
  expect((screen.getByLabelText("allowed_company_ids inherited") as HTMLInputElement).checked).toBe(true);
});

it("suggests Odoo context keys only after typing and preserves case-sensitive keys", async () => {
  vi.mocked(ipc.context).mockResolvedValue({ entries: [{ name: "active_test", value: true, enabled: true }, { name: "Lang", value: "custom", enabled: true }], inherited: {} });
  vi.mocked(ipc.saveContext).mockResolvedValue(undefined);
  render(<SettingsTable kind="context" />);
  const key = await screen.findByLabelText("New context key");
  fireEvent.focus(key);
  expect(screen.queryByRole("listbox")).toBeNull();
  fireEvent.change(key, { target: { value: "act" } });
  expect(screen.getByRole("option", { name: "active_ids" })).toBeTruthy();
  expect(screen.queryByRole("option", { name: "active_test" })).toBeNull();
  expect(screen.queryByRole("option", { name: "Authorization" })).toBeNull();
  fireEvent.change(key, { target: { value: "LANG" } });
  fireEvent.keyDown(key, { key: "ArrowDown" });
  fireEvent.keyDown(key, { key: "Enter" });
  expect((key as HTMLInputElement).value).toBe("lang");
  expect(ipc.saveContext).not.toHaveBeenCalled();
  const value = screen.getByLabelText("Context value 3");
  fireEvent.change(value, { target: { value: "es_ES" } });
  fireEvent.blur(value);
  await waitFor(() => expect(ipc.saveContext).toHaveBeenCalledWith([
    { name: "active_test", value: true, enabled: true },
    { name: "Lang", value: "custom", enabled: true },
    { name: "lang", value: "es_ES", enabled: true },
  ], undefined));
});
it("suggests inherited context keys and allows model-specific default prefixes", async () => {
  localStorage.setItem("odx.collapsed.context", "0");
  vi.mocked(ipc.context).mockResolvedValue({ entries: [], inherited: { lang: "es_ES" } });
  vi.mocked(ipc.saveContext).mockResolvedValue(undefined);
  render(<SettingsTable kind="context" environmentId="fixture-env" />);
  const key = await screen.findByLabelText("New context key");
  fireEvent.change(key, { target: { value: "la" } });
  expect(screen.getByRole("option", { name: "lang" })).toBeTruthy();
  fireEvent.change(key, { target: { value: "default" } });
  fireEvent.mouseDown(screen.getByRole("option", { name: "default_" }));
  fireEvent.change(key, { target: { value: "default_name" } });
  const value = screen.getByLabelText("Context value 1");
  fireEvent.change(value, { target: { value: "Fixture" } });
  fireEvent.blur(value);
  await waitFor(() => expect(ipc.saveContext).toHaveBeenCalledWith([{ name: "default_name", value: "Fixture", enabled: true }], "fixture-env"));
});
it("trims context keys and asks before revoking pending approvals, discarding on cancel", async () => {
  vi.mocked(ipc.context).mockResolvedValue({ entries: [], inherited: {} });
  vi.mocked(ipc.saveContext).mockResolvedValue(undefined);
  render(<SettingsTable kind="context" pendingCount={2} />);
  expect(screen.getByLabelText("Save warning").textContent).toContain("Pending requests for all environments are revoked");
  const edit = async () => {
    fireEvent.change(await screen.findByLabelText("New context key"), { target: { value: "  lang " } });
    const value = screen.getByLabelText("Context value 1");
    fireEvent.change(value, { target: { value: "es_ES" } });
    fireEvent.blur(value);
    return screen.findByRole("alertdialog");
  };
  expect((await edit()).textContent).toContain("There are 2 pending approvals. Saving revokes them");
  expect((screen.getByLabelText("Context key 1") as HTMLInputElement).value).toBe("lang");
  fireEvent.click(screen.getByRole("button", { name: "Back" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(screen.queryByLabelText("Context key 1")).toBeNull();
  expect(ipc.saveContext).not.toHaveBeenCalled();
  await edit();
  fireEvent.click(screen.getByRole("button", { name: "Save and revoke" }));
  await waitFor(() => expect(ipc.saveContext).toHaveBeenCalledWith([{ name: "lang", value: "es_ES", enabled: true }], undefined));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect((screen.getByLabelText("Context key 1") as HTMLInputElement).value).toBe("lang");
});
