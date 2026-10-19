import { cleanup, fireEvent, render, screen, waitFor, act } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { discardRestoredWindowState, captureWindowState, preventUnloadUntil, restoreWindowState, usePreventUnload, useWindowLifecycle, useWindowState } from "./windowState";

const { events } = vi.hoisted(() => ({ events: new Map<string, (event: { payload: number }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, callback) => {
  events.set(name, callback);
  return () => { if (events.get(name) === callback) events.delete(name); };
}) }));
let snapshot: string | null = null;
beforeEach(async () => {
  vi.clearAllMocks();
  snapshot = null;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "desktop_restore") return snapshot;
    if (command === "desktop_ready") return null;
    if (command === "desktop_suspend") { snapshot = (args as { snapshot: string }).snapshot; return true; }
    return undefined;
  });
  await restoreWindowState();
});
afterEach(() => { cleanup(); events.clear(); });
function Work({ busy = false }: { busy?: boolean }) {
  const [input, setInput] = useWindowState("input", "");
  usePreventUnload(busy);
  useWindowLifecycle(() => undefined);
  return <input aria-label="Draft" value={input} onChange={(event) => setInput(event.target.value)} />;
}
const close = async () => {
  await waitFor(() => expect(events.has("desktop-suspend")).toBe(true));
  act(() => { events.get("desktop-suspend")!({ payload: 7 }); });
};
const saves = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === "desktop_suspend");

it("waits for ongoing work, then saves the latest draft and restores it in a new WebView", async () => {
  const view = render(<Work busy />);
  fireEvent.change(screen.getByLabelText("Draft"), { target: { value: "unfinished command" } });
  await close();
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(saves()).toHaveLength(0);
  view.rerender(<Work />);
  await waitFor(() => expect(saves()).toHaveLength(1));
  expect(JSON.parse(snapshot!)).toEqual({ input: "unfinished command" });
  view.unmount();
  await restoreWindowState();
  render(<Work />);
  expect((screen.getByLabelText("Draft") as HTMLInputElement).value).toBe("unfinished command");
});

it("cancels a deferred unload when the tray opens the existing window", async () => {
  const view = render(<Work busy />);
  await close();
  act(() => { events.get("desktop-resume")!({ payload: 0 }); });
  view.rerender(<Work />);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(saves()).toHaveLength(0);
});

it("releases a pending write's unload guard when the write fails", async () => {
  let fail!: (error: Error) => void;
  const work = preventUnloadUntil(new Promise<void>((_, reject) => { fail = reject; }));
  const settled = work.catch(() => undefined);
  render(<Work />);
  await close();
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(saves()).toHaveLength(0);
  await act(async () => { fail(new Error("save failed")); await settled; });
  await waitFor(() => expect(saves()).toHaveLength(1));
});

it("consumes restored state so locking and remounting cannot resurrect old work", async () => {
  snapshot = JSON.stringify({ input: "previous session" });
  await restoreWindowState();
  const view = render(<Work />);
  expect((screen.getByLabelText("Draft") as HTMLInputElement).value).toBe("previous session");
  view.unmount();
  expect(captureWindowState()).toEqual({});
  render(<Work />);
  expect((screen.getByLabelText("Draft") as HTMLInputElement).value).toBe("");
});

it("cancels suspension and reports a failed save without dropping the live state", async () => {
  const report = vi.fn();
  function Window() {
    const [value] = useWindowState("input", "preserved");
    useWindowLifecycle(report);
    return <span>{value}</span>;
  }
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "desktop_suspend") throw new Error("save failed");
    return null;
  });
  render(<Window />);
  await close();
  await waitFor(() => expect(report).toHaveBeenCalled());
  expect(invoke).toHaveBeenCalledWith("desktop_cancel_suspend", { ticket: 7 });
  expect(captureWindowState()).toEqual({ input: "preserved" });
});

it("drops a restored snapshot if the vault locks before Shell mounts", async () => {
  snapshot = JSON.stringify({ input: "old private work" });
  await restoreWindowState();
  discardRestoredWindowState();
  render(<Work />);
  expect((screen.getByLabelText("Draft") as HTMLInputElement).value).toBe("");
});
