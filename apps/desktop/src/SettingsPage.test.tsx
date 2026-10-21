import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import SettingsPage from "./SettingsPage";
import { ipc, type Settings, type SettingsView } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { settings: vi.fn(), saveSettings: vi.fn() } }));
vi.mock("./SettingsTable", () => ({ default: () => null }));
vi.mock("./ProxyPanel", () => ({ default: () => null }));
afterEach(cleanup);

const defaults: Settings = { global_concurrency: 16, environment_concurrency: 4, upstream_timeout_secs: 60, approval_minutes: 5, activity_days: 30, activity_mib: 256, output_hours: 24, output_mib: 512 };
const bounds = { global_concurrency: { min: 1, max: 16 }, environment_concurrency: { min: 1, max: 4 }, upstream_timeout_secs: { min: 5, max: 60 }, approval_minutes: { min: 1, max: 5 }, activity_days: { min: 1, max: 30 }, activity_mib: { min: 16, max: 256 }, output_hours: { min: 1, max: 24 }, output_mib: { min: 32, max: 512 } };
const view = (settings: Settings = defaults): SettingsView => ({ settings, defaults, bounds });
beforeEach(() => {
  vi.mocked(ipc.settings).mockReset().mockResolvedValue(view());
  vi.mocked(ipc.saveSettings).mockReset().mockImplementation(async (settings) => view(settings));
  localStorage.removeItem("odx.collapsed.settings-fixed-limits");
});
function page() {
  return <SettingsPage theme="dark" setTheme={vi.fn()} busy={false} pendingCount={0} onContextSaved={vi.fn()} onHeadersSaved={vi.fn()} lock={vi.fn()} changePassword={vi.fn()} />;
}
const form = () => screen.getByRole("form", { name: "Limits and retention" });
const input = (label: RegExp) => within(form()).getByRole("spinbutton", { name: label }) as HTMLInputElement;
const save = () => within(form()).getByRole("button", { name: "Save" }) as HTMLButtonElement;

it("shows the four settings with their range and the rest as read-only limits", async () => {
  render(page());
  await screen.findByRole("form", { name: "Limits and retention" });
  for (const title of ["Concurrency", "Odoo timeout", "Approval expiry", "Activity retention", "Output files"])
    expect(within(form()).getByRole("group", { name: title })).toBeTruthy();
  expect(within(form()).getAllByRole("spinbutton")).toHaveLength(8);
  expect(input(/^Total \(calls\)/).value).toBe("16");
  expect(input(/^Total \(calls\)/).max).toBe("16");
  expect(form().textContent).toContain("Initial 5 · 1–5");
  expect(save().disabled).toBe(true);
  const fixed = screen.getByRole("region", { name: "Fixed limits" });
  const expand = within(fixed).getByRole("button", { name: "Expand Fixed limits" });
  expect(expand.getAttribute("aria-expanded")).toBe("false");
  expect(fixed.textContent).not.toContain("64 requests");
  fireEvent.click(expand);
  expect(within(fixed).queryAllByRole("spinbutton")).toHaveLength(0);
  expect(within(fixed).queryAllByRole("textbox")).toHaveLength(0);
  expect(fixed.textContent).toContain("64 requests total / 16 per environment");
  expect(fixed.textContent).toContain("1 MiB");
});

it("rejects out-of-range and inconsistent values before calling the backend", async () => {
  render(page());
  await screen.findByRole("form", { name: "Limits and retention" });
  for (const value of ["0", "17", "", "2.5"]) {
    fireEvent.change(input(/^Total \(calls\)/), { target: { value } });
    expect(save().disabled).toBe(true);
  }
  expect(form().textContent).toContain("Must be between 1 and 16");
  fireEvent.change(input(/^Total \(calls\)/), { target: { value: "2" } });
  fireEvent.change(input(/^Per environment/), { target: { value: "3" } });
  expect(save().disabled).toBe(true);
  expect(form().textContent).toContain("Cannot exceed the global limit");
  expect(input(/^Per environment/).getAttribute("aria-invalid")).toBe("true");
  fireEvent.submit(form());
  expect(ipc.saveSettings).not.toHaveBeenCalled();
});

it("saves valid values as numbers and applies them without restart", async () => {
  render(page());
  await screen.findByRole("form", { name: "Limits and retention" });
  fireEvent.change(input(/^Validity/), { target: { value: "2" } });
  fireEvent.change(input(/^Per request/), { target: { value: "30" } });
  fireEvent.click(save());
  await waitFor(() => expect(ipc.saveSettings).toHaveBeenCalledWith({ ...defaults, approval_minutes: 2, upstream_timeout_secs: 30 }));
  expect((await within(form()).findByRole("status")).textContent).toContain("saved and applied");
  expect(form().textContent).toContain("Changes apply without restarting");
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(save().disabled).toBe(true);
});

it("asks for confirmation before shrinking retention, which deletes history now", async () => {
  render(page());
  await screen.findByRole("form", { name: "Limits and retention" });
  fireEvent.change(within(screen.getByRole("group", { name: "Activity retention" })).getByRole("spinbutton", { name: /^Age/ }), { target: { value: "7" } });
  fireEvent.click(save());
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog.textContent).toContain("This cannot be undone");
  fireEvent.click(within(dialog).getByRole("button", { name: "Back" }));
  expect(ipc.saveSettings).not.toHaveBeenCalled();
  fireEvent.click(save());
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Reduce and delete" }));
  await waitFor(() => expect(ipc.saveSettings).toHaveBeenCalledWith({ ...defaults, activity_days: 7 }));
});

it("shows backend validation errors and keeps the draft", async () => {
  vi.mocked(ipc.saveSettings).mockRejectedValue({ code: "app.invalid_input" });
  render(page());
  await screen.findByRole("form", { name: "Limits and retention" });
  fireEvent.change(input(/^Validity/), { target: { value: "3" } });
  fireEvent.click(save());
  expect((await within(form()).findByRole("alert")).textContent).toContain("Some input is invalid or incomplete.");
  expect(input(/^Validity/).value).toBe("3");
});

it("reports a locked or failing backend instead of showing stale values", async () => {
  vi.mocked(ipc.settings).mockRejectedValue({ code: "app.locked" });
  render(page());
  expect((await screen.findByRole("alert")).textContent).toContain("The vault is locked.");
  expect(screen.queryAllByRole("spinbutton")).toHaveLength(0);
});

it("names each limit by its label only, describes it with its range, and jumps between sections", async () => {
  render(<SettingsPage theme="dark" setTheme={vi.fn()} busy={false} pendingCount={2} onContextSaved={vi.fn()} onHeadersSaved={vi.fn()} lock={vi.fn()} changePassword={vi.fn()} />);
  await screen.findByRole("form", { name: "Limits and retention" });
  const total = within(form()).getByRole("spinbutton", { name: "Total (calls)" });
  expect(total.getAttribute("aria-describedby")).toContain("setting-global_concurrency-hint");
  expect(screen.getByText(/2 pending approvals will be revoked/)).toBeTruthy();
  expect(within(form()).queryByText("Unsaved changes")).toBeNull();
  fireEvent.change(total, { target: { value: "8" } });
  expect(within(form()).getByText("Unsaved changes")).toBeTruthy();
  const nav = screen.getByRole("navigation", { name: "Settings sections" });
  fireEvent.click(within(nav).getByRole("button", { name: "Security" }));
  expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Security" }));
});
