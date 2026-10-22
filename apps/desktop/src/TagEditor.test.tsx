import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import TagEditor from "./TagEditor";
import { loadTagColors, setTagColor } from "./tagColors";
import { ipc } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { tagColors: vi.fn().mockResolvedValue({}), setTagColor: vi.fn().mockResolvedValue(undefined) } }));
afterEach(() => { cleanup(); setTagColor("Prod", "neutral"); });

it("colors a tag by name, remembers it and shares it with other editors", () => {
  render(<><TagEditor tags={["Prod"]} suggestions={[]} onChange={vi.fn()} /><TagEditor tags={["prod"]} suggestions={[]} onChange={vi.fn()} /></>);
  const [first, second] = screen.getAllByRole("button", { name: /^Color of/ });
  fireEvent.click(first);
  const colors = screen.getByRole("radiogroup", { name: "Color of Prod" });
  expect(within(colors).getByRole("radio", { name: "Gray" }).getAttribute("aria-checked")).toBe("true");
  fireEvent.click(within(colors).getByRole("radio", { name: "Red" }));
  expect(screen.queryByRole("radiogroup")).toBeNull();
  // Same tag, different case: same color everywhere.
  expect(first.closest(".chip")!.className).toContain("tone-danger");
  expect(second.closest(".chip")!.className).toContain("tone-danger");
  expect(ipc.setTagColor).toHaveBeenLastCalledWith("Prod", "danger");
  fireEvent.click(second);
  fireEvent.click(within(screen.getByRole("radiogroup")).getByRole("radio", { name: "Gray" }));
  expect(first.closest(".chip")!.className).not.toContain("tone-");
  expect(ipc.setTagColor).toHaveBeenLastCalledWith("prod", null);
});

it("moves colors kept on this device into the vault once", async () => {
  localStorage.setItem("odx.tagColors", JSON.stringify({ prod: "danger", old: "pink" }));
  vi.mocked(ipc.tagColors).mockResolvedValueOnce({ staging: "info" });
  await loadTagColors();
  expect(ipc.setTagColor).toHaveBeenCalledWith("prod", "danger");
  expect(ipc.setTagColor).not.toHaveBeenCalledWith("old", expect.anything());
  expect(localStorage.getItem("odx.tagColors")).toBeNull();
  render(<TagEditor tags={["prod", "staging"]} suggestions={[]} onChange={vi.fn()} />);
  expect(screen.getByRole("button", { name: "Color of prod" }).closest(".chip")!.className).toContain("tone-danger");
  expect(screen.getByRole("button", { name: "Color of staging" }).closest(".chip")!.className).toContain("tone-info");
});
