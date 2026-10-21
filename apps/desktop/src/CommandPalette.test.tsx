import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import CommandPalette from "./CommandPalette";
import type { Environment } from "./ipc";

afterEach(cleanup);
const envs: Environment[] = [
  { id: "p", name: "Production", url: "https://erp.example.test", database: "prod", login: "fixture", protocol: "json2", context: {}, write_mode: "read_only", has_secret: true, tags: ["customer"] },
  { id: "s", name: "Staging", url: "https://staging.test", database: "stage", login: "fixture", protocol: "json2", context: {}, write_mode: "approval", has_secret: true, tags: [] },
];

function Harness(props: { setPage: () => void; showEnvironment: () => void; openShell: (env: Environment) => void; lock: () => void }) {
  const [open, setOpen] = useState(false);
  return <CommandPalette open={open} setOpen={setOpen} envs={envs} {...props} />;
}

it("opens with Ctrl+K, filters by name, URL or tag and runs the active item from the keyboard", () => {
  const props = { setPage: vi.fn(), showEnvironment: vi.fn(), openShell: vi.fn(), lock: vi.fn() };
  render(<Harness {...props} />);
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  const input = screen.getByRole("combobox", { name: "Search pages, environments and actions" });
  expect(document.activeElement).toBe(input);
  expect(screen.getByRole("option", { name: /Go to Settings/ })).toBeTruthy();
  fireEvent.change(input, { target: { value: "CUSTOMER terminal" } });
  expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Open Production in Terminal"]);
  fireEvent.change(input, { target: { value: "staging.test" } });
  const options = screen.getAllByRole("option");
  expect(options.map((o) => o.textContent)).toEqual(["Show Staging", "Open Staging in Terminal"]);
  expect(input.getAttribute("aria-activedescendant")).toBe(options[0].id);
  fireEvent.keyDown(input, { key: "ArrowDown" });
  expect(options[1].getAttribute("aria-selected")).toBe("true");
  expect(input.getAttribute("aria-activedescendant")).toBe(options[1].id);
  fireEvent.keyDown(input, { key: "Enter" });
  expect(props.openShell).toHaveBeenCalledWith(envs[1]);
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "nothing like this" } });
  expect(screen.getByText("No matches.")).toBeTruthy();
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(props.lock).not.toHaveBeenCalled();
});

it("leaves Ctrl+K alone while a confirmation dialog is open and ignores other modifiers", () => {
  render(<><div role="alertdialog" /><Harness setPage={vi.fn()} showEnvironment={vi.fn()} openShell={vi.fn()} lock={vi.fn()} /></>);
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  expect(screen.queryByRole("dialog")).toBeNull();
  cleanup();
  render(<Harness setPage={vi.fn()} showEnvironment={vi.fn()} openShell={vi.fn()} lock={vi.fn()} />);
  fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true });
  expect(screen.queryByRole("dialog")).toBeNull();
});
