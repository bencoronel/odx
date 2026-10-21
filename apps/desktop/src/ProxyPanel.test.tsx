import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import ProxyPanel, { PROXY_MASK, draftFromView, proxyError, proxyInput } from "./ProxyPanel";
import { ipc } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { proxy: vi.fn(), saveProxy: vi.fn() } }));
afterEach(cleanup);
beforeEach(() => {
  vi.mocked(ipc.proxy).mockReset().mockResolvedValue(null);
  vi.mocked(ipc.saveProxy).mockReset().mockResolvedValue(undefined);
});
const form = () => screen.getByRole("form", { name: "Outgoing proxy" });
const kind = () => within(form()).getByRole("combobox", { name: "Proxy type" });
const chooseKind = (name: string) => { fireEvent.click(kind()); fireEvent.mouseDown(screen.getByRole("option", { name })); };
const field = (name: string) => within(form()).getByLabelText(name) as HTMLInputElement;
const save = () => within(form()).getByRole("button", { name: "Save" }) as HTMLButtonElement;

it("configures a SOCKS5 proxy (socks5h) from the empty state", async () => {
  render(<ProxyPanel />);
  await screen.findByRole("form", { name: "Outgoing proxy" });
  expect(kind().textContent).toBe("No proxy");
  fireEvent.click(kind());
  expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["No proxy", "HTTP", "HTTPS", "SOCKS5"]);
  fireEvent.keyDown(kind(), { key: "Escape" });
  expect(within(form()).queryByLabelText("Proxy host")).toBeNull();
  expect(save().disabled).toBe(true);
  expect(form().textContent).toContain("HTTP_PROXY");
  chooseKind("SOCKS5");
  expect(field("Proxy port").value).toBe("1080");
  expect(save().disabled).toBe(true);
  fireEvent.change(field("Proxy host"), { target: { value: "proxy.example.test" } });
  fireEvent.click(save());
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledWith({ url: "socks5h://proxy.example.test:1080", username: "", password: "" }));
  expect((await within(form()).findByRole("status")).textContent).toContain("Proxy saved");
});

it("shows stored credentials only as a mask and keeps them when saved unchanged", async () => {
  vi.mocked(ipc.proxy).mockResolvedValue({ url: "http://proxy.example.test:3128", username: PROXY_MASK, password: PROXY_MASK });
  const { container } = render(<ProxyPanel />);
  await screen.findByRole("form", { name: "Outgoing proxy" });
  expect(kind().textContent).toBe("HTTP");
  expect(field("Proxy host").value).toBe("proxy.example.test");
  expect(field("Proxy username").value).toBe(PROXY_MASK);
  expect(field("Proxy password").type).toBe("password");
  expect(field("Proxy password").value).toBe(PROXY_MASK);
  expect(save().disabled).toBe(true);
  fireEvent.change(field("Proxy port"), { target: { value: "8080" } });
  fireEvent.click(save());
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledWith({ url: "http://proxy.example.test:8080", username: PROXY_MASK, password: PROXY_MASK }));
  // Typing replaces the entire placeholder.
  fireEvent.change(field("Proxy password"), { target: { value: `${PROXY_MASK}n` } });
  expect(field("Proxy password").value).toBe("n");
  fireEvent.change(field("Proxy password"), { target: { value: "fixture-new-pass" } });
  fireEvent.click(save());
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenLastCalledWith(expect.objectContaining({ password: "fixture-new-pass", username: PROXY_MASK })));
  expect(container.innerHTML).not.toContain("fixture-new-pass");
});

it("rejects incomplete proxies before calling the backend and can disable the proxy", async () => {
  vi.mocked(ipc.proxy).mockResolvedValue({ url: "http://proxy.example.test:3128", username: "", password: "" });
  render(<ProxyPanel />);
  await screen.findByRole("form", { name: "Outgoing proxy" });
  fireEvent.change(field("Proxy password"), { target: { value: "x" } });
  expect(within(form()).getByRole("alert").textContent).toContain("requires a username");
  expect(save().disabled).toBe(true);
  fireEvent.change(field("Proxy password"), { target: { value: "" } });
  expect(field("Proxy port").getAttribute("aria-invalid")).toBeNull();
  fireEvent.change(field("Proxy port"), { target: { value: "70000" } });
  expect(save().disabled).toBe(true);
  expect(field("Proxy port").getAttribute("aria-invalid")).toBe("true");
  expect(field("Proxy port").getAttribute("aria-describedby")).toBe("proxy-error");
  expect(within(form()).getByText("Unsaved changes")).toBeTruthy();
  chooseKind("No proxy");
  fireEvent.click(save());
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledWith(null));
  expect((await within(form()).findByRole("status")).textContent).toContain("directly");
});

it("warns before revoking pending requests and does not save when going back", async () => {
  render(<ProxyPanel pendingCount={2} />);
  await screen.findByRole("form", { name: "Outgoing proxy" });
  chooseKind("HTTP");
  fireEvent.change(field("Proxy host"), { target: { value: "proxy.example.test" } });
  fireEvent.click(save());
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog.textContent).toContain("There are 2 pending approvals");
  expect(dialog.textContent).toContain("inheriting the global proxy");
  fireEvent.click(within(dialog).getByRole("button", { name: "Back" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect(ipc.saveProxy).not.toHaveBeenCalled();
  fireEvent.click(save());
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Save and revoke" }));
  await waitFor(() => expect(ipc.saveProxy).toHaveBeenCalledTimes(1));
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
