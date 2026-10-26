import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CellActions, TableBlock } from "./TableBlock";
import { rowsTable, toCsv, type Link } from "./table";

afterEach(cleanup);
const clipboard = () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  return writeText;
};
/** A result table as the Terminal shows it, with the session's cell actions. */
const show = (rows: Record<string, unknown>[], model = "res.partner", follow = vi.fn<(link: Link) => void>()) => {
  render(<CellActions.Provider value={{ fields: () => undefined, follow }}>
    <TableBlock table={{ ...rowsTable(rows), model }} expanded={false} onToggle={() => undefined} />
  </CellActions.Provider>);
  return follow;
};

it("copies tables as JSON and Markdown from the formats menu", async () => {
  const writeText = clipboard();
  show([{ id: 7, name: "Ada | Lovelace" }]);
  const more = screen.getByRole("button", { name: "More formats" });
  fireEvent.click(more);
  fireEvent.click(screen.getByRole("menuitem", { name: "Copy as JSON" }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(JSON.stringify([{ id: 7, name: "Ada | Lovelace" }], null, 2)));
  fireEvent.click(more);
  fireEvent.click(screen.getByRole("menuitem", { name: "Copy as Markdown" }));
  await waitFor(() => expect(writeText).toHaveBeenLastCalledWith("| id | name |\n| --- | --- |\n| 7 | Ada \\| Lovelace |"));
  fireEvent.click(more);
  fireEvent.mouseDown(document.body);
  expect(screen.queryByRole("menu")).toBeNull();
});

it("adds up numeric columns of the rows shown, with the average on hover, and not for a single row", () => {
  show([{ id: 1, name: "A", amount: 10.5, qty: 2 }, { id: 2, name: "B", amount: 4, qty: false }, { id: 3, name: "C", amount: 1.25, qty: 4 }], "sale.order");
  const table = screen.getByRole("table");
  expect([...table.querySelectorAll("tfoot td")].map((td) => td.textContent)).toEqual(["Σ", "", "", "15.75", "6"]);
  expect(table.querySelector("tfoot td:nth-child(4) span")?.getAttribute("data-tip")).toBe("Average: 5.25");
  cleanup();
  show([{ id: 3, name: "C", amount: 1.25 }], "sale.order");
  expect(screen.getByRole("table").querySelector("tfoot")).toBeNull();
});

it("selects cells, columns and rows instead of text and copies a cell as its value, more as TSV with headers", async () => {
  const writeText = clipboard();
  const follow = show([{ id: 7, name: "Ada", email: "a@x" }, { id: 8, name: "Grace", email: "g@x" }, { id: 9, name: "Hopper", email: "h@x" }]);
  const table = screen.getByRole("table");
  const scroller = document.querySelector<HTMLElement>(".term-scroll")!;
  const cell = (r: number, c: number) => table.querySelector<HTMLElement>(`[data-cell="${r}:${c}"]`)!;
  const copy = () => { fireEvent.keyDown(scroller, { key: "c", ctrlKey: true }); return writeText.mock.calls.at(-1)?.[0]; };

  fireEvent.mouseDown(cell(0, 1));
  await waitFor(() => expect(copy()).toBe("Ada"));
  // Shift extends a box from the anchor; the copy carries the headers of its columns.
  fireEvent.mouseDown(cell(1, 2), { shiftKey: true });
  await waitFor(() => expect(copy()).toBe("name\temail\nAda\ta@x\nGrace\tg@x"));
  // No cursor frame over a mouse selection; the keyboard brings it back.
  expect(table.querySelector("[data-active]")).toBeNull();
  fireEvent.keyDown(scroller, { key: "ArrowDown", shiftKey: true });
  expect(table.querySelector("[data-active]")).not.toBeNull();
  // Ctrl adds loose cells; rows and columns they skip stay blank.
  fireEvent.mouseDown(cell(0, 0));
  fireEvent.mouseDown(cell(2, 2), { ctrlKey: true });
  await waitFor(() => expect(copy()).toBe("id\temail\n7\t\n\th@x"));
  fireEvent.mouseDown(within(table).getByRole("columnheader", { name: /^email/ }));
  await waitFor(() => expect(copy()).toBe("email\na@x\ng@x\nh@x"));
  fireEvent.mouseDown(table.querySelectorAll("tbody .term-row-head")[1]);
  await waitFor(() => expect(copy()).toBe("id\tname\temail\n8\tGrace\tg@x"));
  expect(table.querySelectorAll("[data-selected]").length).toBe(4);
  // A click outside the table clears it.
  fireEvent.mouseDown(document.body);
  expect(table.querySelectorAll("[data-selected]")).toHaveLength(0);
  // A double click still acts: an id opens its record.
  fireEvent.doubleClick(within(cell(2, 0)).getByRole("button"));
  expect(follow).toHaveBeenCalledWith(expect.objectContaining({ model: "res.partner", column: "id", value: 9 }), false);
});

it("exports negative numbers as numbers and formula-like text as text", () => {
  expect(toCsv({ columns: ["amount", "note"], rows: [[-150.5, "-1+1"]] })).toBe("﻿amount,note\r\n-150.5,'-1+1");
});
