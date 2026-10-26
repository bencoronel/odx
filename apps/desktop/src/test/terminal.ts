import { fireEvent, screen } from "@testing-library/react";
import { vi } from "vitest";

/** `vi.mock("./ipc", ...)` for Terminal tests: an idle core with no history or saved sessions and healthy environments.
 * Each test sets `shell` and whatever else it needs. */
export const terminalIpc = () => ({ ipc: {
  agentTake: vi.fn(async () => []), agentReply: vi.fn(async () => undefined), openTableWindow: vi.fn(async () => undefined),
  shellHistory: vi.fn(async () => []), shellSessions: vi.fn(async () => null), saveShellSessions: vi.fn(async () => undefined), shellHistoryAdd: vi.fn(async () => undefined),
  check: vi.fn(async (id: string) => ({ environment_id: id, status: "ok", latency_ms: 1 })), shell: vi.fn(), shellExport: vi.fn(), openUrl: vi.fn(),
  // The app around the Terminal, for tests that render it.
  status: vi.fn(), environments: vi.fn(), approvals: vi.fn(), clients: vi.fn(), serviceStatus: vi.fn(), lock: vi.fn(), unlock: vi.fn(),
} });
/** jsdom has no scrollTo; the output scrolls to each new line. */
export const stubScroll = () => Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, value: vi.fn() });
/** Types `line` in the active session's input and runs it. */
export const command = (line: string) => {
  const input = screen.getByRole("textbox", { name: "Command" });
  fireEvent.change(input, { target: { value: line } });
  fireEvent.keyDown(input, { key: "Enter" });
};
/** Opens a session from the "+" menu, bound to `env`. The tab strip's "+" comes first; the empty state repeats it. */
export const newSession = (env = "A") => {
  fireEvent.click(screen.getAllByRole("button", { name: "New Terminal session" })[0]);
  fireEvent.click(screen.getByRole("menuitem", { name: env }));
};
