import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import SettingsTable from "./SettingsTable";
import { ipc } from "./ipc";
import { useWindowLifecycle } from "./windowState";

const { events } = vi.hoisted(() => ({ events: new Map<string, (event: { payload: number }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, callback) => {
  events.set(name, callback);
  return () => { events.delete(name); };
}) }));
vi.mock("./ipc", () => ({ ipc: { context: vi.fn(), headers: vi.fn(), saveContext: vi.fn(), saveHeaders: vi.fn() } }));
afterEach(() => {
  cleanup(); events.clear(); vi.clearAllMocks();
  localStorage.removeItem("odx.collapsed.context");
  localStorage.removeItem("odx.collapsed.headers");
});

it.each(["context", "headers"] as const)("waits for the entire %s save queue after navigating away", async (kind) => {
  const entry = { name: kind === "context" ? "lang" : "User-Agent", value: "original", enabled: true };
  vi.mocked(ipc.context).mockResolvedValue({ entries: [entry], inherited: {} });
  vi.mocked(ipc.headers).mockResolvedValue({ headers: [entry], inherited: [] });
  const save = kind === "context" ? vi.mocked(ipc.saveContext) : vi.mocked(ipc.saveHeaders);
  let finishFirst!: () => void;
  let finishSecond!: () => void;
  save.mockImplementationOnce(() => new Promise<void>((resolve) => { finishFirst = resolve; }))
    .mockImplementationOnce(() => new Promise<void>((resolve) => { finishSecond = resolve; }));
  function Window() {
    const [show, setShow] = useState(true);
    useWindowLifecycle(() => undefined);
    return <><button onClick={() => setShow(false)}>Navigate</button>
      {show && <SettingsTable kind={kind} environmentId="fixture-env" />}</>;
  }
  localStorage.setItem(`odx.collapsed.${kind}`, "0");
  render(<Window />);
  const input = await screen.findByLabelText(kind === "context" ? "Context value 1" : "Header value 1");
  fireEvent.change(input, { target: { value: "first" } });
  fireEvent.blur(input);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  fireEvent.change(input, { target: { value: "second" } });
  fireEvent.blur(input);
  fireEvent.click(screen.getByText("Navigate"));
  await waitFor(() => expect(events.has("desktop-suspend")).toBe(true));
  act(() => { events.get("desktop-suspend")!({ payload: 7 }); });
  const suspends = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === "desktop_suspend");
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  expect(suspends()).toHaveLength(0);
  await act(async () => { finishFirst(); });
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  expect(suspends()).toHaveLength(0);
  expect(save).toHaveBeenLastCalledWith([{ ...entry, value: "second" }], "fixture-env");
  await act(async () => { finishSecond(); });
  await waitFor(() => expect(suspends()).toHaveLength(1));
});

it("suspends after deleting a global default that is not stored", async () => {
  vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
  function Window() {
    useWindowLifecycle(() => undefined);
    return <SettingsTable kind="headers" />;
  }
  render(<Window />);
  fireEvent.click(await screen.findByLabelText("Delete header 1"));
  await waitFor(() => expect(events.has("desktop-suspend")).toBe(true));
  act(() => { events.get("desktop-suspend")!({ payload: 7 }); });
  await waitFor(() => expect(vi.mocked(invoke)).toHaveBeenCalledWith("desktop_suspend", expect.objectContaining({ ticket: 7 })));
  expect(ipc.saveHeaders).not.toHaveBeenCalled();
});
