import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ProxyPanel, { PROXY_MASK, draftFromView, proxyError, proxyInput } from "./ProxyPanel";
import { ipc } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { proxies: vi.fn(), saveProxy: vi.fn(), deleteProxy: vi.fn(), setDefaultProxy: vi.fn(), environments: vi.fn(), testProxy: vi.fn() } }));
afterEach(cleanup);
const office = { id: "office", name: "Office", url: "http://proxy.example.test:3128", username: PROXY_MASK, password: PROXY_MASK, environments: 0 };
beforeEach(() => {
  vi.mocked(ipc.proxies).mockReset().mockResolvedValue({ proxies: [], default: null });
  vi.mocked(ipc.saveProxy).mockReset().mockResolvedValue("new");
  vi.mocked(ipc.deleteProxy).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.setDefaultProxy).mockReset().mockResolvedValue(undefined);
  vi.mocked(ipc.environments).mockReset().mockResolvedValue([{ id: "e1", name: "Prod", url: "https://prod.example.test" }, { id: "e2", name: "Staging", url: "https://staging.example.test" }] as never);
  vi.mocked(ipc.testProxy).mockReset();
});
const panel = () => screen.getByRole("region", { name: "Proxies" });
const editor = (name: string) => screen.getByRole("form", { name });
const field = (form: HTMLElement, name: string) => within(form).getByLabelText(name) as HTMLInputElement;
const choose = (label: string, option: string) => { fireEvent.click(screen.getByRole("combobox", { name: label })); fireEvent.mouseDown(screen.getByRole("option", { name: option })); };

it("adds a named SOCKS5 proxy (socks5h) from the empty state", async () => {
  render(<ProxyPanel />);
  expect(await within(await screen.findByRole("region", { name: "Proxies" })).findByText("No proxies.")).toBeTruthy();
  fireEvent.click(within(panel()).getByRole("button", { name: "Add proxy" }));
  const form = editor("New proxy");
  const create = within(form).getByRole("button", { name: "Create proxy" }) as HTMLButtonElement;
  choose("Proxy type", "SOCKS5");
  expect(field(form, "Proxy port").value).toBe("1080");
  fireEvent.change(field(form, "Proxy host"), { target: { value: "proxy.example.test" } });
  expect(create.disabled).toBe(true);
  fireEvent.change(within(form).getByRole("textbox", { name: "Name" }), { target: { value: " VPN " } });
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [{ id: "new", name: "VPN", url: "socks5h://proxy.example.test:1080", username: "", password: "", environments: 0 }], default: null });
  fireEvent.click(create);
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledWith(null, { name: "VPN", url: "socks5h://proxy.example.test:1080", username: "", password: "" }));
  expect(await within(panel()).findByText("SOCKS5 · proxy.example.test:1080")).toBeTruthy();
  expect(screen.queryByRole("form", { name: "New proxy" })).toBeNull();
});

it("edits a proxy keeping masked credentials, and sets or clears the default", async () => {
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office], default: null });
  const { container } = render(<ProxyPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "Edit Office" }));
  const form = editor("Edit Office");
  expect(field(form, "Proxy username").value).toBe(PROXY_MASK);
  expect(field(form, "Proxy password").type).toBe("password");
  fireEvent.change(field(form, "Proxy port"), { target: { value: "8080" } });
  fireEvent.click(within(form).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledWith("office", { name: "Office", url: "http://proxy.example.test:8080", username: PROXY_MASK, password: PROXY_MASK }));
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office], default: "office" });
  fireEvent.click(await screen.findByRole("button", { name: "Make default" }));
  await waitFor(() => expect(ipc.setDefaultProxy).toHaveBeenCalledWith("office"));
  fireEvent.click(await screen.findByRole("button", { name: "Remove default" }));
  await waitFor(() => expect(ipc.setDefaultProxy).toHaveBeenLastCalledWith(null));
  expect(container.innerHTML).not.toContain("fixture");
});

it("tests the proxy being edited against a chosen environment without saving", async () => {
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office], default: null });
  vi.mocked(ipc.testProxy).mockResolvedValueOnce({ latency_ms: 84, server_version: "17.0" }).mockRejectedValueOnce({ code: "upstream.network", params: { kind: "proxy" } });
  render(<ProxyPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "Edit Office" }));
  const form = editor("Edit Office");
  const test = await within(form).findByRole("button", { name: "Test" });
  fireEvent.click(test);
  await waitFor(() => expect(ipc.testProxy).toHaveBeenCalledWith("office", { name: "", url: "http://proxy.example.test:3128", username: PROXY_MASK, password: PROXY_MASK }, "https://prod.example.test"));
  expect((await within(form).findByRole("status")).textContent).toContain("Reached Odoo 17.0 through the proxy · 84 ms");
  // Editing clears the stale result; another environment can be the target.
  fireEvent.change(field(form, "Proxy port"), { target: { value: "8080" } });
  expect(within(form).queryByRole("status")).toBeNull();
  choose("Test against", "Staging");
  fireEvent.click(test);
  expect(await within(form).findByRole("alert")).toBeTruthy();
  expect(vi.mocked(ipc.testProxy).mock.calls[1][2]).toBe("https://staging.example.test");
  expect(ipc.saveProxy).not.toHaveBeenCalled();
});

it("rejects incomplete proxies before calling the backend", async () => {
  render(<ProxyPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "Add proxy" }));
  const form = editor("New proxy");
  fireEvent.change(within(form).getByRole("textbox", { name: "Name" }), { target: { value: "Office" } });
  fireEvent.change(field(form, "Proxy host"), { target: { value: "proxy.example.test" } });
  fireEvent.change(field(form, "Proxy password"), { target: { value: "x" } });
  expect(form.textContent).toContain("requires a username");
  fireEvent.change(field(form, "Proxy password"), { target: { value: "" } });
  fireEvent.change(field(form, "Proxy port"), { target: { value: "70000" } });
  expect(field(form, "Proxy port").getAttribute("aria-invalid")).toBe("true");
  expect((within(form).getByRole("button", { name: "Create proxy" }) as HTMLButtonElement).disabled).toBe(true);
  expect(ipc.saveProxy).not.toHaveBeenCalled();
});

it("confirms before deleting and blocks deleting a proxy in use", async () => {
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office, { ...office, id: "vpn", name: "VPN", environments: 2 }], default: "office" });
  render(<ProxyPanel />);
  expect(await screen.findByText("Used by 2 environments")).toBeTruthy();
  expect((screen.getByRole("button", { name: "Delete VPN" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Delete Office" }));
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog.textContent).toContain("environments set to Default will connect directly");
  expect(ipc.deleteProxy).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(ipc.deleteProxy).toHaveBeenCalledWith("office"));
});

it("warns before revoking pending requests and does not save when going back", async () => {
  vi.mocked(ipc.proxies).mockResolvedValue({ proxies: [office], default: null });
  render(<ProxyPanel pendingCount={2} />);
  fireEvent.click(await screen.findByRole("button", { name: "Make default" }));
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog.textContent).toContain("There are 2 pending approvals");
  fireEvent.click(within(dialog).getByRole("button", { name: "Back" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(ipc.setDefaultProxy).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Make default" }));
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save and revoke" }));
  await waitFor(() => expect(ipc.setDefaultProxy).toHaveBeenCalledWith("office"));
});

it("round-trips IPv6 hosts and validates host and port", () => {
  const draft = draftFromView({ url: "socks5h://[::1]:1080", username: "", password: "" });
  expect(draft).toMatchObject({ kind: "socks5h", host: "[::1]", port: "1080" });
  expect(proxyInput({ ...draft, host: "::1" }).url).toBe("socks5h://[::1]:1080");
  expect(proxyError({ ...draft, host: "" })).not.toBeNull();
  expect(proxyError({ ...draft, host: "user@h" })).not.toBeNull();
  expect(proxyError({ ...draft, port: "0" })).not.toBeNull();
  expect(proxyError({ ...draft, username: "a:b" })).not.toBeNull();
  expect(proxyError(draft)).toBeNull();
});
