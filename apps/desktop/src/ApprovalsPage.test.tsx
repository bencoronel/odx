import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import ApprovalsPage from "./ApprovalsPage";
import type { Pending } from "./ipc";
afterEach(cleanup);
// jsdom lacks scrollIntoView, which the page uses for keyboard selection.
Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: vi.fn() });
const unlink = (id: string, args: unknown, kwargs: unknown = {}): Pending => ({
  id, environment_id: "env-A", environment_name: "A", model: "res.partner", method: "unlink",
  args, kwargs, context: {}, expires_at: new Date(Date.now() + 600_000).toISOString(),
});

it("states how many records an unlink approval deletes", () => {
  render(<ApprovalsPage envs={[]} busy={false} decide={vi.fn()} pending={[
    unlink("one", [[7]]),
    unlink("many", [[1, 2, 3]]),
    unlink("named", [], { ids: [4, 5] }),
  ]} />);
  expect(screen.getByText("Irreversible deletion: it cannot be undone.")).toBeTruthy();
  expect(screen.getByText("Irreversible deletion of 3 records: it cannot be undone.")).toBeTruthy();
  expect(screen.getByText("Irreversible deletion of 2 records: it cannot be undone.")).toBeTruthy();
});
