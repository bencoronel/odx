import type { Environment, WriteMode } from "./ipc";
import { stageRank } from "./StageBadge";

/** Host of an environment URL, or the raw text when it does not parse. */
export const hostOf = (url: string) => { try { return new URL(url).host; } catch { return url; } };

/** Group by host, keeping the given order: one Odoo server can hold several databases. */
export function groupByHost<T extends { url: string }>(items: T[]): [string, T[]][] {
  const byHost = new Map<string, T[]>();
  for (const item of items) byHost.set(hostOf(item.url), [...byHost.get(hostOf(item.url)) ?? [], item]);
  return [...byHost];
}

export type GroupBy = "project" | "host" | "mode" | "flat";

const MODE_ORDER: WriteMode[] = ["permanent", "temporary", "approval", "read_only"];

export type EnvironmentGroup = { key: string; label: string; kind: "project" | "host" | "mode" | "plain"; items: Environment[] };

/** Group environments according to the chosen view. Within a project, higher-risk stages (PRO → PRE → TEST → DEV) come first. */
export function groupEnvironments(
  items: Environment[],
  by: GroupBy,
  labels: { noProject: string; mode: (mode: WriteMode) => string },
): EnvironmentGroup[] {
  if (by === "flat") return items.map((env) => ({ key: env.id, label: env.name, kind: "plain", items: [env] }));
  if (by === "host") {
    return groupByHost(items).map(([host, list]) => ({
      key: `host:${host}`,
      label: host,
      kind: list.length > 1 ? "host" : "plain",
      items: list,
    }));
  }
  if (by === "mode") {
    const byMode = new Map<WriteMode, Environment[]>();
    for (const env of items) byMode.set(env.write_mode, [...byMode.get(env.write_mode) ?? [], env]);
    return MODE_ORDER.filter((m) => byMode.has(m)).map((m) => ({
      key: `mode:${m}`,
      label: labels.mode(m),
      kind: "mode",
      items: byMode.get(m)!,
    }));
  }
  // Group by project (case-insensitive, keeping first spelling); unassigned environments go at the end.
  const byProject = new Map<string, { label: string; items: Environment[] }>();
  const unassigned: Environment[] = [];
  for (const env of items) {
    const raw = env.project?.trim();
    if (!raw) { unassigned.push(env); continue; }
    const k = raw.toLowerCase();
    const existing = byProject.get(k);
    if (existing) existing.items.push(env);
    else byProject.set(k, { label: raw, items: [env] });
  }
  const sortByStage = (list: Environment[]) => [...list].sort((a, b) => stageRank(a.stage) - stageRank(b.stage) || a.name.localeCompare(b.name));
  const groups: EnvironmentGroup[] = [...byProject.entries()]
    .sort(([, a], [, b]) => a.label.localeCompare(b.label))
    .map(([k, g]) => ({ key: `project:${k}`, label: g.label, kind: "project", items: sortByStage(g.items) }));
  if (unassigned.length) {
    groups.push({ key: "project:__none__", label: labels.noProject, kind: groups.length ? "project" : "plain", items: sortByStage(unassigned) });
  }
  return groups;
}
