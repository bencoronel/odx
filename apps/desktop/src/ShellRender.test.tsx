import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import ShellPage from "./ShellPage";
import { ipc } from "./ipc";
import { formatDateTime } from "./i18n/format";
import { envNamed, shellResult } from "./test/fixtures";
import { command, newSession, stubScroll } from "./test/terminal";

// Datetime cells format through formatDateTime on every render: its call count shows whether a table rendered again.
vi.mock("./i18n/format", async (original) => {
  const actual = await original<typeof import("./i18n/format")>();
  return { ...actual, formatDateTime: vi.fn(actual.formatDateTime) };
});
vi.mock("./ipc", async () => (await import("./test/terminal")).terminalIpc());
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => undefined)) }));
afterEach(cleanup);
const envs = [envNamed("A", { write_mode: "approval" })];

it("typing in the input leaves finished tables alone", async () => {
  stubScroll();
  const rows = Array.from({ length: 30 }, (_, i) => ({ id: i + 1, write_date: "2026-10-01 10:00:00" }));
  vi.mocked(ipc.shell).mockImplementation(async (call) =>
    shellResult(call.method === "fields_get" ? { write_date: { type: "datetime", string: "Updated" } } : call.model === "ir.model" ? [] : rows));
  render(<ShellPage envs={envs} />);
  newSession("A");
  await waitFor(() => expect(screen.getByRole("tabpanel").querySelector(".term-status .term-mode")).toBeTruthy());
  command("search res.partner -f write_date");
  const input = screen.getByRole("textbox", { name: "Command" });
  await waitFor(() => expect(within(screen.getByRole("tabpanel")).getByText(/\d+ rows/)).toBeTruthy());
  // Once the fields are known, cells show the date in local time.
  await waitFor(() => expect(vi.mocked(formatDateTime).mock.calls.length).toBeGreaterThanOrEqual(20));
  await new Promise((resolve) => setTimeout(resolve, 50));
  const before = vi.mocked(formatDateTime).mock.calls.length;
  for (const value of ["c", "co", "cou", "coun", "count"]) fireEvent.change(input, { target: { value } });
  expect(vi.mocked(formatDateTime).mock.calls.length).toBe(before);
});
