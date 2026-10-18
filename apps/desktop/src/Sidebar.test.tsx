import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import Sidebar from "./Sidebar";
afterEach(cleanup);

it("shows the unlocked state and how many pending requests locking revokes", () => {
  const lock = vi.fn();
  const { rerender } = render(<Sidebar page="mcp" setPage={vi.fn()} busy={false} lock={lock} counts={{ environments: 1, approvals: 0 }} />);
  const button = screen.getByRole("button", { name: "Lock vault" });
  expect(button.textContent).toBe("UnlockedLock vault");
  rerender(<Sidebar page="mcp" setPage={vi.fn()} busy={false} lock={lock} counts={{ environments: 1, approvals: 3 }} />);
  expect(button.textContent).toContain("Lock · 3 pending");
  expect(button.dataset.tip).toContain("Revokes 3 pending requests");
  fireEvent.click(button);
  expect(lock).toHaveBeenCalledOnce();
});
