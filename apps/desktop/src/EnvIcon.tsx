import { useEffect, useState } from "react";
import { Icon } from "./Icon";
import { ipc, type Environment } from "./ipc";

type IconEnvironment = Pick<Environment, "id" | "url" | "database">;
const cache = new Map<string, { until: number; image: Promise<string | null> }>();

function imageUrl(href: string, base: string) {
  try {
    const url = new URL(href, base);
    return ["http:", "https:"].includes(url.protocol) && !url.username && !url.password ? url.href : null;
  } catch { return null; }
}

async function discover(env: IconEnvironment) {
  const icons: string[] = [];
  try {
    const page = await ipc.faviconPage(env.id);
    // Keep the template detached to prevent scripts and resource loading.
    const template = document.createElement("template");
    template.innerHTML = page.html;
    const baseHref = template.content.querySelector("base[href]")?.getAttribute("href");
    const base = (baseHref && imageUrl(baseHref, page.url)) || page.url;
    for (const link of template.content.querySelectorAll("link[rel][href]")) {
      if (link.getAttribute("rel")!.toLowerCase().split(/\s+/).includes("icon")) {
        const url = imageUrl(link.getAttribute("href")!, base);
        if (url) icons.push(url);
      }
    }
  } catch { /* An inaccessible page may still have a public favicon. */ }
  for (const href of ["/favicon.ico", "favicon.ico", "web/static/img/favicon.ico"]) {
    const url = imageUrl(href, `${env.url.replace(/\/$/, "")}/`);
    if (url) icons.push(url);
  }
  return [...new Set(icons)];
}

async function loadIcon(env: IconEnvironment): Promise<string | null> {
  for (const url of await discover(env)) {
    try {
      const image = await ipc.faviconImage(env.id, url);
      const src = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(reader.result as string);
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(new Blob([new Uint8Array(image.data)], { type: image.content_type }));
      });
      await new Promise<void>((resolve, reject) => {
        const probe = new Image();
        probe.onload = () => resolve();
        probe.onerror = () => reject(new Error("invalid favicon"));
        probe.src = src;
      });
      return src;
    } catch { /* Try the next URL during shared loading only. */ }
  }
  return null;
}

export function EnvIcon({ env }: { env: IconEnvironment }) {
  const key = JSON.stringify([env.id, env.url, env.database]);
  const [state, setState] = useState({ key: "", src: null as string | null });
  useEffect(() => {
    let active = true;
    let entry = cache.get(key);
    if (!entry || entry.until <= Date.now()) {
      const fresh = { until: Infinity, image: loadIcon(env) };
      // Retry failures after five minutes; cache images for the session.
      void fresh.image.then(src => { if (!src) fresh.until = Date.now() + 300_000; });
      if (cache.size >= 128) cache.delete(cache.keys().next().value!);
      cache.set(key, fresh);
      entry = fresh;
    }
    void entry.image.then(src => { if (active) setState({ key, src }); });
    return () => { active = false; };
  }, [key, env.id, env.url, env.database]);
  const src = state.key === key ? state.src : null;
  if (!src) return <Icon name="server" />;
  return <img className="favicon object-contain rounded-[3px]" src={src} alt="" draggable={false} />;
}
