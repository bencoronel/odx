import * as Dialog from "@radix-ui/react-dialog";
import { useEffect, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Icon, type IconName } from "./Icon";
import { PAGE_ORDER, pages, shortcut, type Page } from "./Sidebar";
import type { Environment } from "./ipc";
import { modeTone } from "./ModeSelect";

type Group = "pages" | "environments" | "actions";
/** `env` rows show the server and offer the Terminal as a second action (Ctrl+Enter). */
type Action = { id: string; group: Group; label: string; icon: IconName; hint?: string; keywords?: string; env?: Environment; run: () => void; alt?: () => void };
const groups: Group[] = ["pages", "environments", "actions"];
const hostOf = (url: string) => { try { return new URL(url).host; } catch { return url; } };

/** Ctrl+K anywhere while unlocked; the Terminal binds no Ctrl+K, so it wins there too. Confirmation dialogs keep the keyboard. */
export default function CommandPalette({ open, setOpen, envs, setPage, showEnvironment, openShell, lock }: {
  open: boolean;
  setOpen: (open: boolean) => void;
  envs: Environment[];
  setPage: (page: Page) => void;
  showEnvironment: (env: Environment) => void;
  openShell: (env: Environment) => void;
  lock: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey || event.key.toLowerCase() !== "k") return;
      if (document.querySelector('[role="alertdialog"]')) return;
      event.preventDefault();
      setOpen(!open);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, setOpen]);
  useEffect(() => { if (open) { setQuery(""); setIndex(0); } }, [open]);

  const actions: Action[] = [
    ...PAGE_ORDER.map((page): Action => ({ id: `page-${page}`, group: "pages", label: t(pages[page].title), keywords: t("palette.goTo", { page: "" }), icon: pages[page].icon, hint: shortcut(page), run: () => setPage(page) })),
    ...envs.map((env): Action => ({ id: `env-${env.id}`, group: "environments", label: env.name, icon: "server", env,
      keywords: `${env.url} ${env.database} ${env.tags.join(" ")}`, run: () => showEnvironment(env), alt: () => openShell(env) })),
    { id: "lock", group: "actions", label: t("palette.lock"), icon: "lock", run: lock },
  ];
  // Every word must appear in the label or keywords, in any order.
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const matching = actions.filter((action) => { const text = `${action.label} ${action.keywords ?? ""}`.toLowerCase(); return words.every((word) => text.includes(word)); });
  // Environments first once the user types: that is what a search usually looks for.
  const order = words.length ? ["environments", "pages", "actions"] : groups;
  const shown = order.flatMap((group) => matching.filter((action) => action.group === group));
  const active = shown[Math.min(index, shown.length - 1)];
  useEffect(() => { if (active) document.getElementById(`palette-${active.id}`)?.scrollIntoView?.({ block: "nearest" }); }, [active]);

  const choose = (action: Action, alt = false) => { setOpen(false); (alt && action.alt ? action.alt : action.run)(); };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    const at = active ? shown.indexOf(active) : 0;
    const move = (next: number) => { event.preventDefault(); setIndex(Math.max(0, Math.min(shown.length - 1, next))); };
    if (event.key === "ArrowDown") move(at + 1);
    else if (event.key === "ArrowUp") move(at - 1);
    else if (event.key === "Home" && event.ctrlKey) move(0);
    else if (event.key === "End" && event.ctrlKey) move(shown.length - 1);
    else if (event.key === "Enter" && active) { event.preventDefault(); choose(active, event.ctrlKey || event.metaKey); }
  };

  return <Dialog.Root open={open} onOpenChange={setOpen}>
    <Dialog.Portal>
      <Dialog.Overlay className="dialog-overlay" />
      <Dialog.Content className="dialog-content palette" aria-describedby={undefined}>
        <Dialog.Title className="sr-only">{t("palette.title")}</Dialog.Title>
        <label className="search"><Icon name="search" />
          <input role="combobox" aria-expanded="true" aria-controls="palette-list" aria-label={t("palette.description")} aria-activedescendant={active ? `palette-${active.id}` : undefined} aria-autocomplete="list"
            placeholder={t("palette.placeholder")} value={query} onChange={(event) => { setQuery(event.target.value); setIndex(0); }} onKeyDown={onKeyDown} />
        </label>
        <ul id="palette-list" className="palette-list" role="listbox" aria-label={t("palette.results")}>
          {shown.map((action, i) => <PaletteRow key={action.id} action={action} active={action === active}
            heading={shown[i - 1]?.group !== action.group ? t(`palette.groups.${action.group}`) : undefined}
            hover={() => setIndex(i)} choose={(alt) => choose(action, alt)} />)}
          {!shown.length && <li className="hint" role="presentation">{t("palette.empty")}</li>}
        </ul>
        <footer className="palette-keys" aria-hidden="true">
          <span><kbd>↑</kbd><kbd>↓</kbd>{t("palette.keysMove")}</span>
          <span><kbd>↵</kbd>{t("palette.keysOpen")}</span>
          {active?.alt && <span><kbd>Ctrl</kbd><kbd>↵</kbd>{t("palette.keysShell")}</span>}
        </footer>
      </Dialog.Content>
    </Dialog.Portal>
  </Dialog.Root>;
}

function PaletteRow({ action, active, heading, hover, choose }: { action: Action; active: boolean; heading?: string; hover: () => void; choose: (alt: boolean) => void }) {
  const { t } = useTranslation();
  const env = action.env;
  return <>
    {heading && <li className="palette-heading" role="presentation">{heading}</li>}
    <li id={`palette-${action.id}`} role="option" aria-selected={active} aria-label={action.label}
      className={`select-option palette-row ${active ? "active" : ""}`} onMouseMove={hover} onClick={() => choose(false)}>
      {env
        ? <span className="palette-env-icon"><Icon name="server" /><span className={`status-dot tone-${modeTone[env.write_mode]}`} /></span>
        : <Icon name={action.icon} />}
      <span className="palette-text"><span className="truncate">{action.label}</span>
        {env && <small className="truncate">{hostOf(env.url)} · {env.database}</small>}</span>
      {action.hint && <kbd>{action.hint}</kbd>}
      {action.alt && <button type="button" className="btn sm ghost palette-alt" tabIndex={-1} aria-label={t("palette.openInShell", { name: action.label })}
        onClick={(event) => { event.stopPropagation(); choose(true); }}><Icon name="terminal" />{t("palette.shell")}</button>}
    </li>
  </>;
}
