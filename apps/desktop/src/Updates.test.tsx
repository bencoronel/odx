import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getBundleType, getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/app", async (original) => ({ ...await original<typeof import("@tauri-apps/api/app")>(), getBundleType: vi.fn(), getVersion: vi.fn() }));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
afterEach(cleanup);

const downloadAndInstall = vi.fn();
const update = () => ({ version: "0.1.0-alpha.4", downloadAndInstall }) as unknown as Update;
beforeEach(() => {
  vi.resetModules();
  vi.mocked(check).mockReset().mockResolvedValue(update());
  vi.mocked(getBundleType).mockReset().mockResolvedValue("appimage" as never);
  vi.mocked(getVersion).mockReset().mockResolvedValue("0.1.0-alpha.3");
  vi.mocked(invoke).mockReset().mockResolvedValue(undefined);
  downloadAndInstall.mockReset().mockResolvedValue(undefined);
});
// Fresh module per test: the startup check is cached for the app's lifetime.
const load = () => import("./Updates");

it("checks once at startup and installs, flushes the Shell and restarts an AppImage", async () => {
  const flush = vi.fn(async () => { expect(invoke).not.toHaveBeenCalledWith("desktop_restart"); });
  const { UpdateNotice } = await load();
  const { registerShellFlush } = await import("./shellFlush");
  registerShellFlush(flush);
  const { unmount } = render(<UpdateNotice />);
  fireEvent.click(await screen.findByRole("button", { name: "Update and restart" }));
  expect(screen.getByRole("status").textContent).toContain("Odx 0.1.0-alpha.4 is available.");
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("desktop_restart"));
  expect(downloadAndInstall).toHaveBeenCalledOnce();
  expect(flush).toHaveBeenCalledOnce();
  registerShellFlush(undefined);
  unmount();
  render(<UpdateNotice />);
  await screen.findByRole("button", { name: "Update and restart" });
  expect(check).toHaveBeenCalledOnce();
});

it("stays silent when the startup check fails and shows install errors without closing", async () => {
  vi.mocked(check).mockRejectedValueOnce("offline");
  const { UpdateNotice } = await load();
  const { container } = render(<UpdateNotice />);
  await waitFor(() => expect(check).toHaveBeenCalled());
  expect(container.textContent).toBe("");

  vi.resetModules();
  downloadAndInstall.mockRejectedValueOnce("bad signature");
  const fresh = await load();
  render(<fresh.UpdateNotice />);
  fireEvent.click(await screen.findByRole("button", { name: "Update and restart" }));
  expect((await screen.findByRole("alert")).textContent).toBe("The update could not be installed. The current version keeps running.");
  expect(invoke).not.toHaveBeenCalledWith("desktop_restart");
  expect((screen.getByRole("button", { name: "Update and restart" }) as HTMLButtonElement).disabled).toBe(false);
});

it("offers only the release page outside an AppImage", async () => {
  vi.mocked(getBundleType).mockResolvedValue("deb" as never);
  const { UpdateNotice } = await load();
  render(<UpdateNotice />);
  fireEvent.click(await screen.findByRole("button", { name: "Open release page" }));
  expect(invoke).toHaveBeenCalledWith("updates_open_releases");
  expect(screen.queryByRole("button", { name: "Update and restart" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.queryByRole("status")).toBeNull();
});

it("checks manually in Settings with version, up-to-date, error and available results", async () => {
  const { UpdatesPanel } = await load();
  vi.mocked(check).mockResolvedValueOnce(null).mockRejectedValueOnce("offline");
  render(<UpdatesPanel />);
  await screen.findByText("Version 0.1.0-alpha.3");
  expect(check).not.toHaveBeenCalled();
  const button = screen.getByRole("button", { name: "Check for updates" });
  fireEvent.click(button);
  expect((await screen.findByRole("status")).textContent).toBe("Odx is up to date.");
  fireEvent.click(button);
  expect((await screen.findByRole("alert")).textContent).toBe("Could not check for updates. Check your connection and try again.");
  fireEvent.click(button);
  await screen.findByRole("button", { name: "Update and restart" });
  expect(screen.getByRole("status").textContent).toBe("Odx 0.1.0-alpha.4 is available.");
});
