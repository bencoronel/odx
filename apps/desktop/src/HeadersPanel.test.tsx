import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsTable, { HEADER_MASK } from "./SettingsTable";
import { headerNames } from "./HeaderInput";
import { ipc } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { headers: vi.fn(), saveHeaders: vi.fn() } }));
beforeEach(() => { localStorage.setItem("odx.collapsed.headers", "0"); });
afterEach(() => { cleanup(); vi.resetAllMocks(); localStorage.removeItem("odx.collapsed.headers"); });

describe("HeadersPanel", () => {
  it("autosaves on blur and grows the final row without losing focus or showing an unset user-agent", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    const last = await screen.findByLabelText("New header name");
    expect(screen.getAllByPlaceholderText("Key")).toHaveLength(1);
    expect(screen.getAllByPlaceholderText("Value")).toHaveLength(1);
    for (const name of ["Add header", "Save", "Cancel"]) expect(screen.queryByRole("button", { name })).toBeNull();
    expect(screen.queryByDisplayValue("User-Agent")).toBeNull();
    expect(screen.queryByRole("button", { name: /Hidden/ })).toBeNull();
    expect(screen.getByLabelText("Automatic header name Host")).toBeTruthy();
    expect((screen.getByLabelText("Header value 2") as HTMLInputElement).value).toBe("application/json");
    fireEvent.change(screen.getByLabelText("Header value 1"), { target: { value: "application/json" } });
    last.focus();
    fireEvent.change(last, { target: { value: "   " } });
    expect(screen.queryByLabelText("New header name")).toBeNull();
    fireEvent.change(last, { target: { value: "U" } });
    expect(document.activeElement).toBe(last);
    expect(screen.getByLabelText("New header name")).not.toBe(last);
    expect((screen.getByLabelText("Header value 3") as HTMLInputElement).value).toBe("");
    expect(last.getAttribute("placeholder")).toBeNull();
    expect(screen.getByLabelText("Header value 3").getAttribute("placeholder")).toBeNull();
    expect(screen.getAllByPlaceholderText("Key")).toHaveLength(1);
    expect(screen.getAllByPlaceholderText("Value")).toHaveLength(1);
    fireEvent.change(last, { target: { value: "User-Agent" } });
    const value = screen.getByLabelText("Header value 3");
    fireEvent.change(value, { target: { value: "   " } });
    expect(screen.getByLabelText("New header name")).not.toBe(last);
    fireEvent.change(value, { target: { value: "fixture-agent" } });
    expect(screen.getByLabelText("New header name")).not.toBe(last);
    expect(ipc.saveHeaders).not.toHaveBeenCalled();
    fireEvent.blur(value);
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(ipc.saveHeaders).toHaveBeenCalledWith([
      { name: "Accept", value: "application/json", enabled: true },
      { name: "User-Agent", value: "fixture-agent", enabled: true },
    ], undefined);
    fireEvent.blur(value);
    expect(ipc.saveHeaders).toHaveBeenCalledTimes(1);
  });
  it("retains a failed edit and retries on the next edit without saving inherited defaults", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [{ name: "User-Agent", value: "global", enabled: true }] });
    vi.mocked(ipc.saveHeaders).mockRejectedValueOnce("invalid HTTP header value").mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" environmentId="fixture-env" />);
    const key = await screen.findByLabelText("New header name");
    fireEvent.change(key, { target: { value: "User-Agent" } });
    const agent = screen.getByLabelText("Header value 1");
    fireEvent.change(agent, { target: { value: "local" } });
    fireEvent.blur(agent);
    await screen.findByRole("alert");
    expect((agent as HTMLInputElement).value).toBe("local");
    fireEvent.change(agent, { target: { value: "corrected" } });
    fireEvent.blur(agent);
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(ipc.saveHeaders).toHaveBeenLastCalledWith([{ name: "User-Agent", value: "corrected", enabled: true }], "fixture-env");
  });
  it("serializes saves while keeping the latest edit intact", async () => {
    let release!: () => void;
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockImplementationOnce(() => new Promise<void>((resolve) => { release = resolve; })).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    await screen.findByLabelText("New header name");
    const accept = screen.getByLabelText("Header value 1");
    fireEvent.change(accept, { target: { value: "text/plain" } });
    fireEvent.blur(accept);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenCalledTimes(1));
    const contentType = screen.getByLabelText("Header value 2");
    fireEvent.change(contentType, { target: { value: "application/custom" } });
    fireEvent.blur(contentType);
    expect(ipc.saveHeaders).toHaveBeenCalledTimes(1);
    release();
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenCalledTimes(2));
    expect(ipc.saveHeaders).toHaveBeenLastCalledWith([
      { name: "Accept", value: "text/plain", enabled: true },
      { name: "Content-Type", value: "application/custom", enabled: true },
    ], undefined);
    expect((contentType as HTMLInputElement).value).toBe("application/custom");
  });
  it("saves toggles and deletions immediately", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [{ name: "X-Test", value: "fixture", enabled: true }], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    fireEvent.click(await screen.findByLabelText("Enable header 3"));
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([{ name: "X-Test", value: "fixture", enabled: false }], undefined));
    fireEvent.click(screen.getByRole("button", { name: "Delete header 3" }));
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([], undefined));
  });
  it("keeps incomplete rows unsaved until a name is entered", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    fireEvent.change(await screen.findByLabelText("New header value"), { target: { value: "draft" } });
    expect(screen.queryByLabelText("New header name")).toBeNull();
    fireEvent.blur(screen.getByLabelText("Header value 3"));
    expect(ipc.saveHeaders).not.toHaveBeenCalled();
    const key = screen.getByLabelText("Header name 3");
    fireEvent.change(key, { target: { value: "X-Test" } });
    expect(screen.getByLabelText("New header name")).toBeTruthy();
    fireEvent.blur(key);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenCalledWith([{ name: "X-Test", value: "draft", enabled: true }], undefined));
  });
  it("completes names by keyboard and values by mouse without changing defaults or submitting early", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    const key = await screen.findByRole("combobox", { name: "New header name" });
    fireEvent.focus(key);
    expect(headerNames).toHaveLength(190);
    expect(new Set(headerNames.map((name) => name.toLowerCase())).size).toBe(190);
    for (const name of ["Authorization", "Set-Cookie", "Content-Security-Policy"]) expect(headerNames).toContain(name);
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.keyDown(key, { key: "ArrowDown" });
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(key, { target: { value: "c" } });
    const automatic = screen.getByRole("option", { name: "Connection" });
    expect(automatic.getAttribute("aria-disabled")).toBe("true");
    fireEvent.mouseDown(automatic);
    expect((key as HTMLInputElement).value).toBe("c");
    expect(screen.getByRole("option", { name: "Accept-Encoding" }).getAttribute("aria-disabled")).toBe("true");
    expect(screen.queryByRole("option", { name: "Accept" })).toBeNull();
    expect(screen.queryByRole("option", { name: "Host" })).toBeNull();
    fireEvent.change(key, { target: { value: "" } });
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(key, { target: { value: "accept-l" } });
    fireEvent.keyDown(key, { key: "ArrowDown" });
    const selected = screen.getByRole("option", { name: "Accept-Language" });
    expect(key.getAttribute("aria-activedescendant")).toBe(selected.id);
    fireEvent.keyDown(key, { key: "Enter" });
    expect((key as HTMLInputElement).value).toBe("Accept-Language");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(ipc.saveHeaders).not.toHaveBeenCalled();
    const value = screen.getByRole("combobox", { name: "Header value 3" });
    fireEvent.focus(value);
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(value, { target: { value: "e" } });
    fireEvent.mouseDown(screen.getByRole("option", { name: "es-ES" }));
    expect((value as HTMLInputElement).value).toBe("es-ES");
    fireEvent.blur(value);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenCalledWith([{ name: "Accept-Language", value: "es-ES", enabled: true }], undefined));
  });
  it("allows custom names and values and dismisses suggestions with Escape", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" />);
    const key = await screen.findByRole("combobox", { name: "New header name" });
    fireEvent.focus(key);
    fireEvent.change(key, { target: { value: "Auth" } });
    expect(screen.getByRole("listbox")).toBeTruthy();
    fireEvent.keyDown(key, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(ipc.saveHeaders).not.toHaveBeenCalled();
    fireEvent.change(key, { target: { value: "constructor" } });
    const value = screen.getByRole("combobox", { name: "Header value 3" });
    fireEvent.focus(value);
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(value, { target: { value: "custom-value" } });
    fireEvent.blur(value);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenCalledWith([{ name: "constructor", value: "custom-value", enabled: true }], undefined));
  });

  it("follows newly created rows without scrolling on load or stealing focus", async () => {
    const scroll = vi.fn();
    const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollIntoView");
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", { configurable: true, value: scroll });
    try {
      vi.mocked(ipc.headers).mockResolvedValue({ headers: [{ name: "X-Existing", value: "fixture", enabled: true }], inherited: [] });
      render(<SettingsTable kind="headers" environmentId="fixture-env" />);
      const key = await screen.findByLabelText("New header name");
      expect(scroll).not.toHaveBeenCalled();
      key.focus();
      fireEvent.change(key, { target: { value: "X-First" } });
      expect(scroll).toHaveBeenCalledTimes(1);
      expect(scroll).toHaveBeenLastCalledWith({ block: "nearest" });
      expect(scroll.mock.contexts[0]).toBe(screen.getByLabelText("New header name").closest("tr"));
      expect(document.activeElement).toBe(key);
      fireEvent.change(key, { target: { value: "X-First-Changed" } });
      expect(scroll).toHaveBeenCalledTimes(1);
      fireEvent.change(screen.getByLabelText("New header name"), { target: { value: "X-Second" } });
      expect(scroll).toHaveBeenCalledTimes(2);
      expect(scroll.mock.contexts[1]).toBe(screen.getByLabelText("New header name").closest("tr"));
    } finally {
      if (original) Object.defineProperty(HTMLElement.prototype, "scrollIntoView", original);
      else Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
    }
  });

  it("reveals inherited headers as read-only and saves independent overrides with the same key", async () => {
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [{ name: "User-Agent", value: "global-agent", enabled: true }] });
    vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
    render(<SettingsTable kind="headers" environmentId="fixture-env" />);
    const key = await screen.findByLabelText("New header name");
    expect(screen.queryByDisplayValue("Accept")).toBeNull();
    expect(screen.queryByDisplayValue("global-agent")).toBeNull();
    const toggle = screen.getByRole("button", { name: "Hidden (5)" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect((screen.getByLabelText("Inherited header name Accept") as HTMLInputElement).readOnly).toBe(true);
    expect((screen.getByLabelText("Inherited header value Accept") as HTMLInputElement).readOnly).toBe(true);
    expect((screen.getByLabelText("User-Agent inherited") as HTMLInputElement).disabled).toBe(true);
    fireEvent.change(key, { target: { value: "a" } });
    expect(screen.getByRole("option", { name: "Accept" })).toBeTruthy();
    fireEvent.mouseDown(screen.getByRole("option", { name: "Accept" }));
    const value = screen.getByLabelText("Header value 1");
    fireEvent.change(value, { target: { value: "application/json" } });
    fireEvent.blur(value);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([{ name: "Accept", value: "application/json", enabled: true }], "fixture-env"));
    expect((screen.getByLabelText("Accept inherited") as HTMLInputElement).checked).toBe(false);
    expect((screen.getByLabelText("Inherited header value Accept") as HTMLInputElement).value).toBe("*/*");
    // Preserve explicit overrides even when they match inherited values.
    fireEvent.change(value, { target: { value: "*/*" } });
    fireEvent.blur(value);
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([{ name: "Accept", value: "*/*", enabled: true }], "fixture-env"));
    fireEvent.click(screen.getByRole("button", { name: "Delete header 1" }));
    await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([], "fixture-env"));
    expect((screen.getByLabelText("Accept inherited") as HTMLInputElement).checked).toBe(true);
    fireEvent.click(toggle);
    expect(screen.queryByLabelText("Inherited header value Accept")).toBeNull();
  });

  it("collapses environment headers and remembers the preference after remounting", async () => {
    localStorage.removeItem("odx.collapsed.headers");
    vi.mocked(ipc.headers).mockResolvedValue({ headers: [], inherited: [] });
    render(<SettingsTable kind="headers" environmentId="fixture-env" />);
    expect(screen.queryByRole("table")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Expand HTTP headers" }));
    await screen.findByLabelText("New header name");
    const collapse = screen.getByRole("button", { name: "Collapse HTTP headers" });
    expect(collapse.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(collapse);
    expect(screen.queryByRole("table")).toBeNull();
    expect(localStorage.getItem("odx.collapsed.headers")).toBe("1");
    cleanup();
    render(<SettingsTable kind="headers" environmentId="another-env" />);
    const expand = screen.getByRole("button", { name: "Expand HTTP headers" });
    expect(expand.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByRole("table")).toBeNull();
    fireEvent.click(expand);
    await screen.findByLabelText("New header name");
    expect(localStorage.getItem("odx.collapsed.headers")).toBe("0");
    cleanup();
    localStorage.setItem("odx.collapsed.headers", "1");
    render(<SettingsTable kind="headers" />);
    await screen.findByLabelText("New header name");
    expect(screen.queryByRole("button", { name: /(?:Collapse|Expand) Headers/ })).toBeNull();
  });

});

it("shows stored sensitive values as dots and sends the marker back unchanged", async () => {
  vi.mocked(ipc.headers).mockResolvedValue({ headers: [{ name: "Authorization", value: HEADER_MASK, enabled: true }], inherited: [] });
  vi.mocked(ipc.saveHeaders).mockResolvedValue(undefined);
  render(<SettingsTable kind="headers" environmentId="fixture-env" />);
  const secret = await screen.findByLabelText("Header value 1") as HTMLInputElement;
  expect(secret.value).toBe(HEADER_MASK);
  expect(secret.className).toContain("secret-mask");
  fireEvent.change(screen.getByLabelText("New header name"), { target: { value: " X-Trace " } });
  const trace = screen.getByLabelText("Header value 2");
  fireEvent.change(trace, { target: { value: "1" } });
  fireEvent.blur(trace);
  await waitFor(() => expect(ipc.saveHeaders).toHaveBeenLastCalledWith([
    { name: "Authorization", value: HEADER_MASK, enabled: true },
    { name: "X-Trace", value: "1", enabled: true },
  ], "fixture-env"));
  fireEvent.change(secret, { target: { value: `${HEADER_MASK}n` } });
  expect(secret.value).toBe("n");
});
