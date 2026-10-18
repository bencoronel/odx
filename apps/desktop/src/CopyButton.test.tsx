import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import CopyButton from "./CopyButton";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("copies the requested text and clears the success message after two seconds", async () => {
  vi.useFakeTimers();
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal("navigator", { clipboard: { writeText } });
  render(<CopyButton text={() => "name\tvalue"} />);
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy" })); });
  expect(writeText).toHaveBeenCalledWith("name\tvalue");
  expect(screen.getByRole("button", { name: "Copied" })).toBeTruthy();
  act(() => { vi.advanceTimersByTime(2000); });
  expect(screen.getByRole("button", { name: "Copy" })).toBeTruthy();
});

it("shows a safe error after rejection and allows retrying", async () => {
  const writeText = vi.fn().mockRejectedValueOnce(new Error("private clipboard details")).mockResolvedValue(undefined);
  vi.stubGlobal("navigator", { clipboard: { writeText } });
  render(<CopyButton text="fixture-token" />);
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy" })); });
  expect(screen.getByRole("alert").textContent).toContain("Could not copy to the clipboard");
  expect(document.body.textContent).not.toContain("private clipboard details");
  expect(document.body.textContent).not.toContain("fixture-token");
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy" })); });
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByRole("button", { name: "Copied" })).toBeTruthy();
});

it("reports an unavailable clipboard without an unhandled error", async () => {
  vi.stubGlobal("navigator", {});
  render(<CopyButton text="fixture" />);
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Copy" })); });
  expect(screen.getByRole("alert")).toBeTruthy();
});
