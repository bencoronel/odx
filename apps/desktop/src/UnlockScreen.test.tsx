import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import UnlockScreen, { passwordStrength } from "./UnlockScreen";
import { setLanguage } from "./i18n";
afterEach(async () => { cleanup(); await setLanguage("en"); localStorage.clear(); });

it("lets the user pick the language before a vault exists", async () => {
  render(<UnlockScreen status="absent" busy={false} error="" retry={vi.fn()} submit={vi.fn()} reset={vi.fn()} />);
  expect(screen.getByRole("heading", { name: "Create vault" })).toBeTruthy();
  fireEvent.click(screen.getByRole("combobox", { name: "Interface language" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "Spanish" }));
  expect(await screen.findByRole("heading", { name: "Crear almacén" })).toBeTruthy();
  expect(localStorage.getItem("odx.language")).toBe("es");
});

it("requires a repeated password and keeps it after a failed create", async () => {
  const submit = vi.fn().mockResolvedValue(false);
  render(<UnlockScreen status="absent" busy={false} error="" retry={vi.fn()} submit={submit} reset={vi.fn()} />);
  const [password, repeat] = [screen.getByLabelText("Master password"), screen.getByLabelText("Repeat master password")] as HTMLInputElement[];
  const create = screen.getByRole("button", { name: "Create vault" });
  fireEvent.change(password, { target: { value: "secret-123" } });
  fireEvent.change(repeat, { target: { value: "secret-12" } });
  fireEvent.click(screen.getByRole("checkbox"));
  expect(screen.getByText("The passwords do not match.")).toBeTruthy();
  expect((create as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(repeat, { target: { value: "secret-123" } });
  fireEvent.click(create);
  await Promise.resolve();
  expect(submit).toHaveBeenCalledWith("secret-123");
  expect(password.value).toBe("secret-123");
});

it("hides the unlock error once the user types again and refocuses the field", () => {
  const props = { status: "locked", error: "Wrong password", retry: vi.fn(), submit: vi.fn(), reset: vi.fn() };
  const { rerender } = render(<UnlockScreen {...props} busy />);
  rerender(<UnlockScreen {...props} busy={false} />);
  const password = screen.getByLabelText("Master password");
  expect(document.activeElement).toBe(password);
  expect(screen.getByRole("alert").textContent).toContain("Wrong password");
  fireEvent.change(password, { target: { value: "x" } });
  expect(screen.queryByRole("alert")).toBeNull();
});

it("warns when Caps Lock is on", () => {
  render(<UnlockScreen status="locked" busy={false} error="" retry={vi.fn()} submit={vi.fn()} reset={vi.fn()} />);
  fireEvent.keyUp(screen.getByLabelText("Master password"), { key: "A", modifierCapsLock: true });
  expect(screen.getByRole("img", { name: "Caps Lock is on." })).toBeTruthy();
});

it("resets a forgotten vault only after typing the confirmation word", () => {
  const reset = vi.fn().mockResolvedValue(true);
  render(<UnlockScreen status="locked" busy={false} error="" retry={vi.fn()} submit={vi.fn()} reset={reset} />);
  fireEvent.click(screen.getByRole("button", { name: "Forgot the password?" }));
  const action = screen.getByRole("button", { name: "Reset vault" }) as HTMLButtonElement;
  expect(action.disabled).toBe(true);
  fireEvent.change(screen.getByLabelText("Type RESET to confirm"), { target: { value: "reset" } });
  fireEvent.click(action);
  expect(reset).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.getByRole("heading", { name: "Vault locked" })).toBeTruthy();
});

it("scores passwords by length and variety", () => {
  expect(passwordStrength("12345678")).toBe(1);
  expect(passwordStrength("Correct-horse-42")).toBe(4);
});

it("does not carry a failed reset error back to the unlock form", () => {
  render(<UnlockScreen status="locked" busy={false} error="Reset failed" retry={vi.fn()} submit={vi.fn()} reset={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Forgot the password?" }));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("alert")).toBeNull();
});
