import { render, screen, waitFor, within, fireEvent } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { ipc } from "./ipc";
import { shellResult } from "./test/fixtures";

vi.mock("./WindowTitleBar", () => ({ WindowTitleBar: ({ title }: { title?: string }) => <header>ODX · {title}</header> }));
vi.mock("./ipc", () => ({ ipc: { shell: vi.fn(), shellExport: vi.fn() } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async (command: string) => command === "table_window_take" ? {
  table: {
    columns: ["id", "name", "partner_id"], rows: [[8, "Grace", [1, "Main"]], [7, "Ada", false]], model: "res.partner",
    call: { model: "res.partner", method: "search_read", args: [], kwargs: { fields: ["id", "name", "partner_id"], limit: 3 } },
  },
  footer: "2 rows", title: "search res.partner · Prod · erp", envId: "env-1", context: { lang: "es_ES" },
  fields: [{ name: "name", label: "Name", type: "char" }, { name: "email", label: "Email", type: "char" }, { name: "partner_id", label: "Related", type: "many2one" }],
} : null) }));

it("shows a Terminal table with a window's controls: filter, view, columns, copy and save", async () => {
  const { default: TableWindow } = await import("./TableWindow");
  render(<TableWindow />);
  const table = await screen.findByRole("table");
  expect(screen.getByRole("banner").textContent).toBe("ODX · search res.partner · Prod · erp");
  expect(document.querySelector(".term-foot")).toBeNull();
  // Sorting and field labels come with the table; ids and relations stay text.
  expect(within(table).getByRole("button", { name: "Sort by name" }).closest("th")?.getAttribute("data-tip")).toBe("Name\nchar");
  fireEvent.click(within(table).getByRole("button", { name: "Sort by name" }));
  await waitFor(() => expect([...table.querySelectorAll("tbody tr td:nth-child(3)")].map((td) => td.textContent)).toEqual(["Ada", "Grace"]));
  expect(within(table).queryByRole("button", { name: "Main (1)" })).toBeNull();
  expect(screen.getByText("2 rows")).toBeTruthy();

  // Columns: the shown ones ticked, the model's other fields offered; hiding is local, adding repeats the read.
  fireEvent.click(screen.getByRole("button", { name: "Columns" }));
  const picker = screen.getByRole("dialog", { name: "Columns" });
  const box = (name: string) => within(picker).getByRole("checkbox", { name: new RegExp(`^${name}`) }) as HTMLInputElement;
  expect(["id", "name", "partner_id", "email"].map((name) => box(name).checked)).toEqual([true, true, true, false]);
  fireEvent.click(box("partner_id"));
  expect(within(screen.getByRole("table")).queryByRole("columnheader", { name: /partner_id/ })).toBeNull();
  vi.mocked(ipc.shell).mockResolvedValue(shellResult([{ id: 8, name: "Grace", partner_id: [1, "Main"], email: "g@x" }, { id: 7, name: "Ada", partner_id: false, email: "a@x" }, { id: 9, name: "Extra", partner_id: false, email: "" }]));
  fireEvent.click(box("email"));
  await waitFor(() => expect(within(screen.getByRole("table")).getByText("g@x")).toBeTruthy());
  expect(ipc.shell).toHaveBeenCalledWith({ environment_id: "env-1", model: "res.partner", method: "search_read", args: [], kwargs: { fields: ["id", "name", "partner_id", "email"], limit: 3 }, context: { lang: "es_ES" }, confirmed: false });
  // The rows stay the ones shown (the read asks one more, to know if there are more).
  expect(screen.getByRole("table").querySelectorAll("tbody tr")).toHaveLength(2);

  fireEvent.change(screen.getByRole("searchbox", { name: "Filter rows" }), { target: { value: "ada" } });
  await waitFor(() => expect(screen.getByRole("table").querySelectorAll("tbody tr")).toHaveLength(1));
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  fireEvent.click(screen.getByRole("button", { name: "More actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Copy as TSV" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith("id\tname\temail\n7\tAda\ta@x"));
  vi.mocked(ipc.shellExport).mockResolvedValue("/home/u/Downloads/odx-res.partner.csv");
  fireEvent.click(screen.getByRole("button", { name: "More actions" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Save as CSV in Downloads" }));
  await waitFor(() => expect(screen.getByText("Saved to /home/u/Downloads/odx-res.partner.csv")).toBeTruthy());
  fireEvent.click(screen.getByRole("radio", { name: "Records" }));
  expect(document.querySelectorAll(".term-table.expanded")).toHaveLength(1);
});
