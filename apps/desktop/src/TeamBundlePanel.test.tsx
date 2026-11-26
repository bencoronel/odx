import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import TeamBundlePanel from "./TeamBundlePanel";
import { ipc } from "./ipc";
import { envFixture } from "./test/fixtures";

vi.mock("./ipc", () => ({
  ipc: {
    environments: vi.fn(),
    views: vi.fn(),
    rules: vi.fn(),
    tagColors: vi.fn(),
    setTagColor: vi.fn().mockResolvedValue(undefined),
    saveView: vi.fn().mockResolvedValue({}),
    saveRule: vi.fn().mockResolvedValue({}),
    shellExport: vi.fn().mockResolvedValue("/tmp/odx-team-bundle.json"),
  },
}));

beforeEach(() => {
  vi.mocked(ipc.environments).mockResolvedValue([
    envFixture({ id: "e1", name: "Acme Pro", url: "https://erp.acme.es", database: "acme_prod", login: "secret-admin@acme.es", project: "Acme", stage: "pro", tags: ["erp"] }),
  ]);
  vi.mocked(ipc.views).mockResolvedValue([
    { id: "v1", name: "Failed emails", description: "", model: "mail.mail", domain: [["state", "=", "exception"]], fields: ["id", "subject"], order: "date desc", limit: 50, schedule: null, created_at: "", updated_at: "" },
  ]);
  vi.mocked(ipc.rules).mockResolvedValue([]);
  vi.mocked(ipc.tagColors).mockResolvedValue({ erp: "#14b8a6" });
  vi.mocked(ipc.shellExport).mockClear();
});
afterEach(cleanup);

it("exports a team bundle with zero users or API keys", async () => {
  render(<TeamBundlePanel />);
  fireEvent.click(screen.getByRole("button", { name: "Export JSON" }));
  await waitFor(() => expect(ipc.shellExport).toHaveBeenCalledOnce());
  const [, rawJson] = vi.mocked(ipc.shellExport).mock.calls[0];
  expect(rawJson).not.toContain("secret-admin@acme.es");
  expect(rawJson).not.toContain("\"login\"");
  expect(rawJson).not.toContain("\"secret\"");
  const parsed = JSON.parse(rawJson);
  expect(parsed.version).toBe(1);
  expect(parsed.environments).toEqual([
    { name: "Acme Pro", url: "https://erp.acme.es", database: "acme_prod", protocol: "legacy_jsonrpc", project: "Acme", stage: "pro", tags: ["erp"] },
  ]);
  expect(parsed.views).toHaveLength(1);
  expect(parsed.tag_colors).toEqual({ erp: "#14b8a6" });
});
