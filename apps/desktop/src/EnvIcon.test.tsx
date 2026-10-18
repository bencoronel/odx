import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { EnvIcon } from "./EnvIcon";
import { ipc } from "./ipc";
vi.mock("./ipc", () => ({ ipc: { faviconPage: vi.fn(), faviconImage: vi.fn() } }));
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.faviconImage).mockResolvedValue({ content_type: "image/png", data: [1, 2, 3] });
  vi.stubGlobal("Image", class {
    onload?: () => void;
    onerror?: () => void;
    set src(value: string) {
      queueMicrotask(() => value.endsWith("AA==") ? this.onerror?.() : this.onload?.());
    }
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const environment = (id: string) => ({ id, url: "https://odoo.test/prefix", database: "fixture" });

it("shares downloaded image bytes across views and remounts for the whole session", async () => {
  vi.mocked(ipc.faviconPage).mockResolvedValue({ url: "https://odoo.test/prefix/web/login", html:
    '<base href="/prefix/"><script>throw new Error("must not run")</script><link rel="shortcut ICON" href="web/image/website/2/favicon?unique=123">' });
  const env = environment("declared");
  const first = render(<><EnvIcon env={env} /><EnvIcon env={env} /></>);
  await waitFor(() => expect(first.container.querySelectorAll("img")).toHaveLength(2));
  const src = first.container.querySelector("img")!.src;
  expect(src).toBe("data:image/png;base64,AQID");
  expect(ipc.faviconImage).toHaveBeenCalledWith(env.id, "https://odoo.test/prefix/web/image/website/2/favicon?unique=123");
  expect(first.container.querySelector("script")).toBeNull();
  first.unmount();
  vi.spyOn(Date, "now").mockReturnValue(Date.now() + 600_000);
  const next = render(<EnvIcon env={env} />);
  await waitFor(() => expect(next.container.querySelector("img")?.src).toBe(src));
  expect(ipc.faviconPage).toHaveBeenCalledTimes(1);
  expect(ipc.faviconImage).toHaveBeenCalledTimes(1);
});

it("ignores unsafe URLs and remembers failed paths when using a fallback", async () => {
  vi.mocked(ipc.faviconPage).mockResolvedValue({ url: "https://odoo.test/", html:
    '<link rel="icon" href="javascript:alert(1)"><link rel="icon" href="https://user:secret@other.test/icon"><link rel="icon" href="/custom.ico">' });
  vi.mocked(ipc.faviconImage).mockRejectedValueOnce(new Error("404"));
  const env = environment("fallbacks");
  const first = render(<EnvIcon env={env} />);
  await waitFor(() => expect(first.container.querySelector("img")).not.toBeNull());
  expect(vi.mocked(ipc.faviconImage).mock.calls).toEqual([
    [env.id, "https://odoo.test/custom.ico"], [env.id, "https://odoo.test/favicon.ico"],
  ]);
  first.unmount();
  const next = render(<EnvIcon env={env} />);
  await waitFor(() => expect(next.container.querySelector("img")).not.toBeNull());
  expect(ipc.faviconImage).toHaveBeenCalledTimes(2);
});

it("tries the next URL when downloaded bytes cannot be decoded as an image", async () => {
  vi.mocked(ipc.faviconPage).mockResolvedValue({ url: "https://odoo.test/", html: "" });
  vi.mocked(ipc.faviconImage).mockResolvedValueOnce({ content_type: "image/png", data: [0] });
  const { container } = render(<EnvIcon env={environment("invalid-image")} />);
  await waitFor(() => expect(container.querySelector("img")?.src).toBe("data:image/png;base64,AQID"));
  expect(ipc.faviconImage).toHaveBeenCalledTimes(2);
});

it("caches failures briefly and retries after five minutes", async () => {
  vi.mocked(ipc.faviconPage).mockRejectedValue(new Error("offline"));
  vi.mocked(ipc.faviconImage).mockRejectedValue(new Error("offline"));
  const env = environment("offline");
  const first = render(<EnvIcon env={env} />);
  await waitFor(() => expect(ipc.faviconImage).toHaveBeenCalledTimes(3));
  first.unmount();
  const next = render(<EnvIcon env={env} />);
  await waitFor(() => expect(next.container.querySelector("svg")).not.toBeNull());
  expect(ipc.faviconPage).toHaveBeenCalledTimes(1);
  next.unmount();
  vi.spyOn(Date, "now").mockReturnValue(Date.now() + 300_001);
  render(<EnvIcon env={env} />);
  await waitFor(() => expect(ipc.faviconPage).toHaveBeenCalledTimes(2));
});

it("invalidates the image when URL or database changes and ignores stale discovery", async () => {
  let finish!: (page: {url: string; html: string}) => void;
  vi.mocked(ipc.faviconPage).mockReturnValueOnce(new Promise(resolve => { finish = resolve; }))
    .mockResolvedValue({ url: "https://new.test/", html: "" });
  const env = environment("changing");
  const { container, rerender } = render(<EnvIcon env={env} />);
  rerender(<EnvIcon env={{ ...env, url: "https://new.test" }} />);
  await waitFor(() => expect(container.querySelector("img")).not.toBeNull());
  finish({ url: "https://old.test", html: '<link rel="icon" href="/old.ico">' });
  rerender(<EnvIcon env={{ ...env, url: "https://new.test", database: "other" }} />);
  await waitFor(() => expect(ipc.faviconPage).toHaveBeenCalledTimes(3));
  await waitFor(() => expect(container.querySelector("img")?.src).toBe("data:image/png;base64,AQID"));
});
