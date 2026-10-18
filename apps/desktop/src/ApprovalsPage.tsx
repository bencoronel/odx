import { useEffect, useRef, useState } from "react";
import type { Environment, Pending } from "./ipc";
import { EnvIcon } from "./EnvIcon";
import { Icon } from "./Icon";
import { formatRemaining, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { CallDetails, methodTone } from "./ApprovalDetails";

export { CallDetails, methodTone };

export default function ApprovalsPage({ pending, envs, busy, decide }: {
  pending: Pending[];
  envs: Environment[];
  busy: boolean;
  decide: (approval: Pending, approve: boolean) => void;
}) {
  const { t } = useTranslation();
  const now = useNow(pending.length > 0);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selected = pending.find((p) => p.id === selectedId) ?? pending[0];
  const list = useRef<HTMLUListElement>(null);

  // ↑/↓ or j/k selects; A approves and R rejects through confirmation dialogs.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (busy || !selected || event.ctrlKey || event.metaKey || event.altKey) return;
      if (document.querySelector('[role="alertdialog"]')) return;
      if ((event.target as HTMLElement).closest("input, textarea, select, [contenteditable]")) return;
      const index = pending.findIndex((p) => p.id === selected.id);
      const key = event.key.toLowerCase();
      if (key === "arrowdown" || key === "j") { event.preventDefault(); setSelectedId(pending[Math.min(pending.length - 1, index + 1)].id); }
      else if (key === "arrowup" || key === "k") { event.preventDefault(); setSelectedId(pending[Math.max(0, index - 1)].id); }
      else if (key === "a" && Date.parse(selected.expires_at) > Date.now()) { event.preventDefault(); decide(selected, true); }
      else if (key === "r") { event.preventDefault(); decide(selected, false); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pending, selected, busy, decide]);
  useEffect(() => {
    list.current?.querySelector(".approval.selected")?.scrollIntoView({ block: "nearest" });
  }, [selected?.id]);

  if (pending.length === 0) return <div className="empty">
    <span className="empty-icon tone-ok"><Icon name="check" /></span>
    <h2>{t("approvals.emptyTitle")}</h2>
    <p>{t("approvals.emptyHint")}</p>
  </div>;

  return <div className="page-scroll"><ul className="approvals" ref={list}>
    {pending.map((item) => {
      const remaining = Date.parse(item.expires_at) - now;
      const expired = remaining <= 0;
      const tone = methodTone(item.method);
      return <li key={item.id} className={`approval tone-${tone} ${item.id === selected?.id ? "selected" : ""}`} aria-current={item.id === selected?.id ? "true" : undefined}
        aria-label={t("approvals.requestLabel", { method: item.method, model: item.model, environment: item.environment_name })} onMouseDown={() => setSelectedId(item.id)}>
        <header className="approval-header">
          <code className={`method tone-${tone}`}>{item.method}</code>
          <strong className="mono truncate">{item.model}</strong>
          <span className="chip env-chip">{envs.find((e) => e.id === item.environment_id) ? <EnvIcon env={envs.find((e) => e.id === item.environment_id)!} /> : <Icon name="server" />}{item.environment_name}</span>
          <span className="chip" data-tip={item.client_label ? t("approvals.clientRequest") : t("approvals.localRequest")}><Icon name="clients" />{item.client_label ?? t("approvals.local")}</span>
          <span className={`countdown ${expired ? "tone-danger" : remaining < 60_000 ? "tone-warn" : ""}`} data-tip={t("approvals.validUntil", { time: formatDateTime(new Date(item.expires_at), { hour: "2-digit", minute: "2-digit" }) })}>
            <Icon name="clock" /><time dateTime={item.expires_at}>{expired ? t("approvals.expired") : formatRemaining(remaining, { hour: t("common.hourShort"), minute: t("common.minuteShort") })}</time>
          </span>
          <span className="row gap-2 ml-auto">
            <button className="btn danger-ghost sm" disabled={busy} onClick={() => decide(item, false)}><Icon name="x" />{t("approvals.reject")}</button>
            <button className="btn primary sm" disabled={busy || expired} onClick={() => decide(item, true)}><Icon name="check" />{t("approvals.approve")}</button>
          </span>
        </header>
        {item.method === "unlink" && <p className="banner danger compact"><Icon name="alert" />{t("approvals.irreversibleDelete", { count: deletedIds(item) })}</p>}
        <CallDetails item={item} />
      </li>;
    })}
  </ul>
  <p className="hint keys-hint"><kbd>↑</kbd><kbd>↓</kbd> {t("approvals.chooseKeyHint")} · <kbd>A</kbd> {t("approvals.approve")} · <kbd>R</kbd> {t("approvals.reject")}</p>
  </div>;
}

/** Records an unlink approval deletes: positional ids or kwargs.ids; 1 when unknown. */
function deletedIds(item: Pending) {
  const ids = Array.isArray(item.args) && Array.isArray(item.args[0]) ? item.args[0] : (item.kwargs as { ids?: unknown } | null)?.ids;
  return Array.isArray(ids) ? ids.length : 1;
}
