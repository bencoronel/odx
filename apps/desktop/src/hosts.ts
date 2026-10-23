/** Host of an environment URL, or the raw text when it does not parse. */
export const hostOf = (url: string) => { try { return new URL(url).host; } catch { return url; } };

/** Group by host, keeping the given order: one Odoo server can hold several databases. */
export function groupByHost<T extends { url: string }>(items: T[]): [string, T[]][] {
  const byHost = new Map<string, T[]>();
  for (const item of items) byHost.set(hostOf(item.url), [...byHost.get(hostOf(item.url)) ?? [], item]);
  return [...byHost];
}
