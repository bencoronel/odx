import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import CommandPalette from "./CommandPalette";
import type { Environment } from "./ipc";
import { envFixture } from "./test/fixtures";

afterEach(cleanup);
const envs = [
  envFixture({ id: "p", name: "Production", url: "https://erp.example.test", database: "prod", login: "fixture", protocol: "json2", tags: ["customer"] }),
  envFixture({ id: "s", name: "Staging", url: "https://staging.test", database: "stage", login: "fixture", protocol: "json2", write_mode: "approval" }),
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
  expect(screen.getByRole("option", { name: "Settings" })).toBeTruthy();
  expect(screen.getByText("Pages")).toBeTruthy();
  // One row per environment, with its server, instead of one row per action.
  fireEvent.change(input, { target: { value: "CUSTOMER" } });
  expect(screen.getAllByRole("option").map((o) => o.getAttribute("aria-label"))).toEqual(["Production"]);
  expect(screen.getByRole("option", { name: "Production" }).textContent).toContain("erp.example.test · prod");
  fireEvent.change(input, { target: { value: "stag" } });
  const options = screen.getAllByRole("option");
  expect(options.map((o) => o.getAttribute("aria-label"))).toEqual(["Staging"]);
  expect(input.getAttribute("aria-activedescendant")).toBe(options[0].id);
  // Enter shows it; Ctrl+Enter opens it in the Terminal.
  fireEvent.keyDown(input, { key: "Enter", ctrlKey: true });
  expect(props.openShell).toHaveBeenCalledWith(envs[1]);
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "production" } });
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
  expect(props.showEnvironment).toHaveBeenCalledWith(envs[0]);
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "staging" } });
  fireEvent.click(screen.getByRole("button", { name: "Open Staging in Terminal" }));
  expect(props.openShell).toHaveBeenLastCalledWith(envs[1]);
  fireEvent.keyDown(window, { key: "k", ctrlKey: true });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "set" } });
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowUp" });
  fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
  expect(props.setPage).toHaveBeenCalledWith("settings");
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
