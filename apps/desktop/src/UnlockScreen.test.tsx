import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import UnlockScreen from "./UnlockScreen";
import { setLanguage } from "./i18n";
afterEach(async () => { cleanup(); await setLanguage("en"); localStorage.clear(); });

it("lets the user pick the language before a vault exists", async () => {
  render(<UnlockScreen status="absent" busy={false} error="" retry={vi.fn()} submit={vi.fn()} />);
  expect(screen.getByRole("heading", { name: "Create vault" })).toBeTruthy();
  fireEvent.click(screen.getByRole("combobox", { name: "Interface language" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "Spanish" }));
  expect(await screen.findByRole("heading", { name: "Crear almacén" })).toBeTruthy();
  expect(localStorage.getItem("odx.language")).toBe("es");
});
