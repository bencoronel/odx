import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ContextMenu, { menuContext } from "./ContextMenu";
afterEach(() => { cleanup(); document.body.innerHTML = ""; window.getSelection()?.removeAllRanges(); });
// jsdom has no editing commands.
beforeEach(() => { document.execCommand = vi.fn(() => true); });

it("offers cut, copy and paste in text fields, copy on selected text and nothing elsewhere", () => {
  document.body.innerHTML = `<p id="p">Some text</p><button id="b">Save</button><input id="t" value="abc"><input id="pw" type="password" value="x">
    <input id="ro" readonly value="r"><input id="c" type="checkbox"><textarea id="ta"></textarea>`;
  const el = (id: string) => document.getElementById(id);
  const selection = window.getSelection()!;
  (el("t") as HTMLInputElement).setSelectionRange(0, 2);
  expect(menuContext(el("t"), selection)).toMatchObject({ canCut: true, canCopy: true, canPaste: true });
  (el("pw") as HTMLInputElement).setSelectionRange(0, 1);
  expect(menuContext(el("pw"), selection), "passwords are never copied").toMatchObject({ canCut: false, canCopy: false, canPaste: true });
  expect(menuContext(el("ro"), selection)).toMatchObject({ canCut: false, canPaste: false });
  expect(menuContext(el("ta"), selection)).toMatchObject({ canCopy: false, canPaste: true });
  for (const id of ["p", "b", "c"]) expect(menuContext(el(id), selection), id).toBeNull();

  selection.setBaseAndExtent(el("p")!.firstChild!, 0, el("p")!.firstChild!, 4);
  expect(menuContext(el("p"), selection)).toEqual({ field: null, canCut: false, canCopy: true, canPaste: false });
  expect(menuContext(el("b"), selection), "outside the selection").toBeNull();
});

it("replaces the native menu and runs editing commands on the field", () => {
  render(<><input aria-label="Name" defaultValue="abc" /><button>Save</button><ContextMenu dev={false} /></>);
  const field = screen.getByRole("textbox", { name: "Name" });
  expect(fireEvent.contextMenu(screen.getByRole("button", { name: "Save" }))).toBe(false);
  expect(screen.queryByRole("menu")).toBeNull();

  expect(fireEvent.contextMenu(field)).toBe(false);
  expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["CutCtrl+X", "CopyCtrl+C", "PasteCtrl+V", "Select allCtrl+A"]);
  fireEvent.click(screen.getByRole("menuitem", { name: /Paste/ }));
  expect(document.execCommand).toHaveBeenCalledWith("paste");
  expect(document.activeElement).toBe(field);
  expect(screen.queryByRole("menu")).toBeNull();

  fireEvent.contextMenu(field);
  act(() => { fireEvent.keyDown(window, { key: "Escape" }); });
  expect(screen.queryByRole("menu")).toBeNull();
});

it("keeps the native menu with Shift in dev builds", () => {
  render(<><button>Save</button><ContextMenu dev /></>);
  expect(fireEvent.contextMenu(screen.getByRole("button"), { shiftKey: true })).toBe(true);
});
