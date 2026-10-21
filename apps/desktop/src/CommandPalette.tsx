import * as Dialog from "@radix-ui/react-dialog";
import { useEffect, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { Icon, type IconName } from "./Icon";
import { PAGE_ORDER, pages, shortcut, type Page } from "./Sidebar";
import type { Environment } from "./ipc";

type Action = { id: string; label: string; icon: IconName; hint?: string; keywords?: string; run: () => void };

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
    ...PAGE_ORDER.map((page) => ({ id: `page-${page}`, label: t("palette.goTo", { page: t(pages[page].title) }), icon: pages[page].icon, hint: shortcut(page), run: () => setPage(page) })),
    ...envs.flatMap((env) => {
      const keywords = `${env.url} ${env.database} ${env.tags.join(" ")}`;
      return [
        { id: `env-${env.id}`, label: t("palette.showEnvironment", { name: env.name }), icon: "server" as const, keywords, run: () => showEnvironment(env) },
        { id: `shell-${env.id}`, label: t("palette.openInShell", { name: env.name }), icon: "terminal" as const, keywords, run: () => openShell(env) },
      ];
    }),
    { id: "lock", label: t("palette.lock"), icon: "lock", run: lock },
  ];
  // Every word must appear in the label or keywords, in any order.
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const shown = actions.filter((action) => { const text = `${action.label} ${action.keywords ?? ""}`.toLowerCase(); return words.every((word) => text.includes(word)); });
  const active = shown[Math.min(index, shown.length - 1)];
  useEffect(() => { if (active) document.getElementById(`palette-${active.id}`)?.scrollIntoView?.({ block: "nearest" }); }, [active]);

  const choose = (action: Action) => { setOpen(false); action.run(); };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    const at = active ? shown.indexOf(active) : 0;
    const move = (next: number) => { event.preventDefault(); setIndex(Math.max(0, Math.min(shown.length - 1, next))); };
    if (event.key === "ArrowDown") move(at + 1);
    else if (event.key === "ArrowUp") move(at - 1);
    else if (event.key === "Home" && event.ctrlKey) move(0);
    else if (event.key === "End" && event.ctrlKey) move(shown.length - 1);
    else if (event.key === "Enter" && active) { event.preventDefault(); choose(active); }
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
          {shown.map((action) => <li key={action.id} id={`palette-${action.id}`} role="option" aria-selected={action === active} className={`select-option ${action === active ? "active" : ""}`}
            onMouseMove={() => setIndex(shown.indexOf(action))} onClick={() => choose(action)}>
            <Icon name={action.icon} /><span className="grow">{action.label}</span>{action.hint && <kbd>{action.hint}</kbd>}
          </li>)}
          {!shown.length && <li className="hint" role="presentation">{t("palette.empty")}</li>}
        </ul>
      </Dialog.Content>
    </Dialog.Portal>
  </Dialog.Root>;
}
