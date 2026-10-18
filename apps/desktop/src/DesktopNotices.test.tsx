import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { DesktopNoticeList } from "./DesktopNotices";
afterEach(cleanup);

it("shows nothing when tray and notifications work", () => {
  const { container } = render(<DesktopNoticeList status={{ tray: true }} />);
  expect(container.textContent).toBe("");
});

it("warns visibly without a usable tray and when notifications are unavailable", () => {
  render(<DesktopNoticeList status={{ tray: false, tray_error: "no StatusNotifierWatcher", notifications_error: "no server" }} />);
  const notices = screen.getAllByRole("status").map((n) => n.textContent);
  expect(notices[0]).toContain("Closing the window locks the vault and stops Odx");
  expect(notices[0]).not.toContain("no StatusNotifierWatcher");
  expect(notices[1]).toContain("appear only in Approvals and the status bar");
  expect(notices[1]).not.toContain("no server");
});
