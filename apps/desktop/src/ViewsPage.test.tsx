import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ViewsPage from "./ViewsPage";
import { ipc, type SavedView } from "./ipc";
import { envFixture } from "./test/fixtures";

vi.mock("./ipc", () => ({
  ipc: {
    views: vi.fn(),
    saveView: vi.fn(),
    deleteView: vi.fn(),
    runView: vi.fn(),
    openTableWindow: vi.fn().mockResolvedValue(undefined),
    shellExport: vi.fn().mockResolvedValue("/tmp/export.csv"),
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));

const env = envFixture({ id: "env-1", name: "Acme Pro", stage: "pro" });
const sampleView: SavedView = {
  id: "v-1",
  name: "Failed emails",
  description: "Outgoing emails in exception",
  model: "mail.mail",
  domain: [["state", "=", "exception"]],
  fields: ["id", "subject", "state"],
  order: "date desc",
  limit: 50,
  schedule: {
    enabled: true,
    environment_ids: ["env-1"],
    interval_minutes: 15,
    alert_on: { kind: "any_rows" },
    last_run_at: "2026-11-26T12:00:00Z",
    last_runs: [{ environment_id: "env-1", environment_name: "Acme Pro", at: "2026-11-26T12:00:00Z", count: 2, triggered: true, error: null, duration_ms: 48 }],
  },
  created_at: "2026-11-26T10:00:00Z",
  updated_at: "2026-11-26T10:00:00Z",
};

beforeEach(() => {
  vi.mocked(ipc.views).mockReset().mockResolvedValue([]);
  vi.mocked(ipc.saveView).mockReset().mockResolvedValue(sampleView);
  vi.mocked(ipc.deleteView).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.runView).mockReset().mockResolvedValue({
    view_id: "v-1",
    environment_id: "env-1",
    environment_name: "Acme Pro",
    rows: [{ id: 10, subject: "Invoice #42", state: "exception" }],
    total_count: 1,
    duration_ms: 35,
    at: "2026-11-26T12:05:00Z",
  });
});
afterEach(cleanup);

it("offers Odoo templates in the empty state and saves a new view with schedule", async () => {
  render(<ViewsPage envs={[env]} />);
  expect(await screen.findByRole("heading", { name: "No saved views yet" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Failed emails/ }));
  const form = await screen.findByRole("form", { name: "New saved view" });
  fireEvent.click(within(form).getByRole("checkbox", { name: /Run automatically/ }));
  fireEvent.click(within(form).getByRole("button", { name: "New view" }));
  await waitFor(() => expect(ipc.saveView).toHaveBeenCalledWith(expect.objectContaining({
    name: "Failed emails",
    model: "mail.mail",
    schedule: expect.objectContaining({ enabled: true, environment_ids: ["env-1"] }),
  })));
});

it("runs a saved view and renders results with table export actions", async () => {
  vi.mocked(ipc.views).mockResolvedValue([sampleView]);
  render(<ViewsPage envs={[env]} />);
  expect(await screen.findByRole("heading", { name: /Failed emails/ })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  await waitFor(() => expect(ipc.runView).toHaveBeenCalledWith("v-1", "env-1"));
  expect(await screen.findByText("Invoice #42")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Open as table" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "CSV" })).toBeTruthy();
});

it("supports OR filter combinator and multi-select target environments dropdown", async () => {
  const env2 = envFixture({ id: "env-2", name: "Beta Pre", stage: "pre" });
  render(<ViewsPage envs={[env, env2]} />);
  expect(await screen.findByRole("heading", { name: "No saved views yet" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Failed emails/ }));
  const form = await screen.findByRole("form", { name: "New saved view" });
  // Failed emails template has 2 filters, so the AND/OR combinator is visible
  fireEvent.click(within(form).getByRole("radio", { name: "Any (OR)" }));
  // Enable schedule and pick both environments via the multi-select dropdown
  fireEvent.click(within(form).getByRole("checkbox", { name: /Run automatically/ }));
  fireEvent.click(within(form).getByRole("button", { name: "Target environments" }));
  const dialog = within(form).getByRole("dialog", { name: "Target environments" });
  fireEvent.click(within(dialog).getByRole("checkbox", { name: /Beta Pre/ }));
  fireEvent.click(within(form).getByRole("button", { name: "New view" }));
  await waitFor(() => expect(ipc.saveView).toHaveBeenCalledWith(expect.objectContaining({
    domain: ["|", ["state", "=", "exception"], ["date", ">=", "{{days_ago:7}}"]],
    schedule: expect.objectContaining({ enabled: true, environment_ids: ["env-1", "env-2"] }),
  })));
});

it("supports nested AND/OR filter groups and round-trips with Odoo prefix domains", async () => {
  const { domainToTree, treeToDomain, formatDomainReadable } = await import("./ViewFilterBuilder");
  let id = 1;
  // A AND (B OR C)
  const mixed = [["state", "=", "exception"], "|", ["date", ">=", "{{days_ago:7}}"], ["retry_count", ">", 3]];
  const tree = domainToTree(mixed, () => id++);
  expect(tree).not.toBeNull();
  expect(treeToDomain(tree!)).toEqual(mixed);
  expect(formatDomainReadable(mixed)).toBe("state = exception  AND  (date >= {{days_ago:7}}  OR  retry_count > 3)");

  // A OR (B AND C)
  const orWithNestedAnd = ["|", ["state", "=", "exception"], "&", ["active", "=", true], ["retry_count", ">", 0]];
  const tree2 = domainToTree(orWithNestedAnd, () => id++);
  expect(tree2).not.toBeNull();
  expect(treeToDomain(tree2!)).toEqual(orWithNestedAnd);
  expect(formatDomainReadable(orWithNestedAnd)).toBe("state = exception  OR  (active = true  AND  retry_count > 0)");

  // UI: add a nested OR group inside the root AND group
  render(<ViewsPage envs={[env]} />);
  expect(await screen.findByRole("heading", { name: "No saved views yet" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /Failed emails/ }));
  const form = await screen.findByRole("form", { name: "New saved view" });
  fireEvent.click(within(form).getByRole("button", { name: "Add group" }));
  const subgroup = within(form).getByRole("group", { name: "Group" });
  const fieldInputs = within(subgroup).getAllByLabelText("Field");
  const valueInputs = within(subgroup).getAllByLabelText("Value");
  fireEvent.change(fieldInputs[0], { target: { value: "email_from" } });
  fireEvent.change(valueInputs[0], { target: { value: "alerts@acme.com" } });
  fireEvent.change(fieldInputs[1], { target: { value: "auto_delete" } });
  fireEvent.change(valueInputs[1], { target: { value: "false" } });
  fireEvent.click(within(form).getByRole("button", { name: "New view" }));
  await waitFor(() => expect(ipc.saveView).toHaveBeenCalledWith(expect.objectContaining({
    domain: [
      ["state", "=", "exception"],
      ["date", ">=", "{{days_ago:7}}"],
      "|",
      ["email_from", "=", "alerts@acme.com"],
      ["auto_delete", "=", false],
    ],
  })));
});
