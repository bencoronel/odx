import { useEffect, useRef, useState, type CSSProperties } from "react";
import type { Environment, Pending } from "./ipc";
import { EnvIcon } from "./EnvIcon";
import { ListResizer, useListWidth } from "./ListResizer";
import { Icon } from "./Icon";
import SearchField from "./SearchField";
import { formatRemaining, relativeTime, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { CallDetails, ValuesTable, methodTone, recordIds, shellCommand, writtenValues } from "./ApprovalDetails";
import CopyButton from "./CopyButton";

export default function ApprovalsPage({ pending, envs, deciding, decide, decideMany, openInShell, openEnvironment, filter, setFilter, focusId }: {
  pending: Pending[];
  envs: Environment[];
  /** Ids whose decision is in flight; only those are locked. */
  deciding: ReadonlySet<string>;
  decide: (approval: Pending, approve: boolean, reason?: string) => void;
  /** Same decision for several selected requests, after confirmation. */
  decideMany: (approvals: Pending[], approve: boolean) => void;
  /** Run a read-only Terminal command on the request's environment. */
  openInShell: (approval: Pending, command: string) => void;
  openEnvironment: (id: string) => void;
  /** Kept by the app, so it survives switching pages. */
  filter: string;
  setFilter: (filter: string) => void;
  /** Request to select on arrival, such as a clicked notification. */
  focusId?: string | null;
}) {
  const { t } = useTranslation();
  const now = useNow(pending.length > 0);
  const [selectedId, setSelectedId] = useState<string | null>(focusId ?? null);
  useEffect(() => { if (focusId) setSelectedId(focusId); }, [focusId]);
  const [nudge, setNudge] = useState(0);
  const [reason, setReason] = useState("");
  const [rejecting, setRejecting] = useState(false);
  const [showKeys, setShowKeys] = useState(false);
  const query = filter.trim().toLowerCase();
  const visible = query ? pending.filter((p) => [p.method, p.model, p.environment_name, p.client_label ?? t("approvals.local")]
    .some((text) => text.toLowerCase().includes(query))) : pending;
  // After a decision, select the request that took its place.
  const lastIndex = useRef(0);
  const found = visible.findIndex((p) => p.id === selectedId);
  if (found >= 0) lastIndex.current = found;
  const selected: Pending | undefined = found >= 0 ? visible[found] : visible[Math.min(lastIndex.current, visible.length - 1)];
  // Multiple selection: Ctrl toggles, Shift extends a range from the anchor. Below two, the cursor rules.
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const anchor = useRef<string | null>(null);
  const batch = visible.filter((p) => picked.has(p.id));
  const marking = batch.length > 0;
  // Marked requests are what A, R and the panel act on, unless the only one marked is under the cursor.
  const multi = marking && !(batch.length === 1 && batch[0].id === selected?.id);
  /** Move the cursor; `mode` keeps ("move"), marks (Space), toggles (Ctrl+click) or extends the selection instead of resetting it. */
  const choose = (id: string, mode: "single" | "move" | "mark" | "toggle" | "range" = "single") => {
    setSelectedId(id);
    if (mode === "range") {
      const to = visible.findIndex((p) => p.id === id);
      // The anchor may be gone (decided or filtered out); the range then starts at the target.
      const start = visible.findIndex((p) => p.id === (anchor.current ?? selected?.id));
      if (start < 0) anchor.current = id;
      const from = start < 0 ? to : start;
      setPicked(new Set(visible.slice(Math.min(from, to), Math.max(from, to) + 1).map((p) => p.id)));
      return;
    }
    anchor.current = id;
    if (mode === "move") return;
    if (mode === "single") return setPicked(new Set());
    // Ctrl+click starts from the request already shown, as in file managers; Space starts empty.
    const next = new Set(mode === "toggle" && !picked.size && selected && selected.id !== id ? [selected.id] : picked);
    if (!next.delete(id)) next.add(id);
    setPicked(next);
  };
  const list = useRef<HTMLUListElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const [listWidth, setListWidth] = useListWidth("odx.approvals.listWidth");
  // Animate only requests that arrive after the page opened.
  const fresh = useRef(new Map<string, boolean>());
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; }, []);
  useEffect(() => { setReason(""); setRejecting(false); }, [selected?.id]);

  // Ctrl+F or / filters; ↑/↓ or j/k selects (Shift extends), Space toggles, Ctrl+A selects all, Esc clears;
  // A approves, R rejects and Shift+R asks for a reason first. With several selected, A and R act on all of them.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.altKey || document.querySelector('[role="dialog"],[role="alertdialog"]')) return;
      const command = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();
      const typing = (event.target as Element).closest?.("input, textarea, select, [contenteditable]");
      if ((command && key === "f") || (!command && key === "/" && !typing)) {
        event.preventDefault();
        search.current?.focus();
        search.current?.select();
        return;
      }
      if (key === "?" && !typing) { event.preventDefault(); setShowKeys((open) => !open); return; }
      if (!selected || typing) return;
      if (command) {
        if (key === "a") { event.preventDefault(); setPicked(new Set(visible.map((p) => p.id))); }
        return;
      }
      const index = visible.findIndex((p) => p.id === selected.id);
      const select = (next: number) => { event.preventDefault(); choose(visible[next].id, event.shiftKey ? "range" : picked.size ? "move" : "single"); };
      if (key === "arrowdown" || key === "j") select(Math.min(visible.length - 1, index + 1));
      else if (key === "arrowup" || key === "k") select(Math.max(0, index - 1));
      // Space on a focused button presses it.
      else if (key === " " && !(event.target as Element).closest?.("button, summary, a")) { event.preventDefault(); choose(selected.id, "mark"); }
      else if (key === "escape" && picked.size) { event.preventDefault(); setPicked(new Set()); }
      else if ((key === "a" || key === "r") && multi) {
        event.preventDefault();
        const ready = batch.filter((p) => !deciding.has(p.id) && (key === "r" || Date.parse(p.expires_at) > Date.now()));
        if (ready.length) decideMany(ready, key === "a");
      }
      else if (key === "v" && !multi) {
        const targets = recordIds(selected);
        if (targets) { event.preventDefault(); openInShell(selected, readCommand(selected, targets, writtenValues(selected))); }
      }
      else if ((key === "a" || key === "r") && !deciding.has(selected.id)) {
        event.preventDefault();
        if (key === "r" && event.shiftKey) setRejecting(true);
        else if (key === "r") decide(selected, false);
        else if (Date.parse(selected.expires_at) > Date.now()) decide(selected, true);
        else setNudge((n) => n + 1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, selected, deciding, decide, decideMany, openInShell, picked, batch, multi]);
  useEffect(() => {
    list.current?.querySelector("[aria-selected=true]")?.scrollIntoView({ block: "nearest" });
  }, [selected?.id]);

  const remainingOf = (item: Pending) => Date.parse(item.expires_at) - now;
  const countdown = (item: Pending) => {
    const remaining = remainingOf(item);
    const expired = remaining <= 0;
    return <span className={`countdown ${expired ? "tone-danger" : remaining < 60_000 ? "tone-warn" : ""}`} data-tip={t("approvals.validUntil", { time: formatDateTime(new Date(item.expires_at), { hour: "2-digit", minute: "2-digit" }) })}>
      <Icon name="clock" /><time dateTime={item.expires_at}>{expired ? t("approvals.expired") : formatRemaining(remaining, { hour: t("common.hourShort"), minute: t("common.minuteShort") })}</time>
    </span>;
  };

  const toolbar = <div className="list-toolbar">
    <SearchField inputRef={search} value={filter} onChange={setFilter} label={t("approvals.filterLabel")} placeholder={t("approvals.filterPlaceholder")} shortcut
      filtered={!!query} count={{ shown: visible.length, total: pending.length, text: t("approvals.filterCount", { shown: visible.length, total: pending.length }) }}
      inputProps={{ "aria-keyshortcuts": "Control+F /" }} />
  </div>;
  const item = selected;
  const expired = !!item && remainingOf(item) <= 0;
  const busy = !!item && deciding.has(item.id);
  const env = item && envs.find((e) => e.id === item.environment_id);
  const ids = item && recordIds(item);
  const values = item && writtenValues(item);
  return <div className="split" style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className="list-pane" aria-label={t("approvals.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("approvals.resizeList")} />
      {toolbar}
      {pending.length === 0 ? <div className="empty">
        <span className="empty-icon tone-ok"><Icon name="check" /></span>
        <h2>{t("approvals.emptyTitle")}</h2>
      </div>
      : !item ? <ul className="env-list"><li className="list-empty"><span className="list-empty-icon"><Icon name="search" /></span><strong>{t("approvals.noMatches")}</strong><button type="button" className="btn sm" onClick={() => setFilter("")}><Icon name="x" />{t("approvals.clearFilter")}</button></li></ul>
      : <ul className="env-list approval-list" role="listbox" aria-multiselectable="true" aria-label={t("approvals.listLabel")} ref={list}>
        {visible.map((p) => {
          if (!fresh.current.has(p.id)) fresh.current.set(p.id, mounted.current);
          const remaining = remainingOf(p);
          const total = Date.parse(p.expires_at) - Date.parse(p.created_at);
          const left = Math.min(1, Math.max(0, remaining / total));
          const tone = remaining <= 0 ? "danger" : remaining < 60_000 ? "warn" : methodTone(p.method);
          return <li key={p.id} role="option" aria-selected={marking ? picked.has(p.id) : p.id === item.id} tabIndex={p.id === item.id ? 0 : -1}
            aria-label={t("approvals.requestLabel", { method: p.method, model: p.model, environment: p.environment_name })}
            className={`approval-item tone-${methodTone(p.method)}${fresh.current.get(p.id) ? " fresh" : ""}${marking && p.id === item.id ? " cursor" : ""}`}
            // WebKit also clicks a focused option on Space (detail 0); the key handler already marked it.
            onClick={(event) => { if (event.detail) choose(p.id, event.shiftKey ? "range" : event.ctrlKey || event.metaKey ? "toggle" : "single"); }}>
            <code className={`method tone-${methodTone(p.method)}`}>{p.method}</code>
            <span className="env-list-text">
              <strong className="mono">{p.model}</strong>
              <small>{p.environment_name} · {p.client_label ?? t("approvals.local")} · {relativeTime(p.created_at, now)}</small>
            </span>
            {deciding.has(p.id) ? <Icon name="refresh" className="spin ml-auto" /> : <span className="ml-auto">{countdown(p)}</span>}
            {/* Remaining validity; color warns as expiry approaches. */}
            <span className={`expiry-bar tone-${tone}`} style={{ "--left": Number.isFinite(left) ? left : 1 } as CSSProperties} aria-hidden="true" />
          </li>;
        })}
      </ul>}
      <div className="hint keys-hint">
        {showKeys && <div id="approval-keys" className="keys-grid">
          {KEY_GROUPS.map(([group, keys]) => <section key={group}>
            <p className="keys-group">{t(`approvals.keys.${group}`)}</p>
            <dl>{keys.map(([combo, action]) => <div key={action}><dt>{combo.map((k) => <kbd key={k}>{k === "Space" ? t("approvals.keys.space") : k}</kbd>)}</dt><dd>{t(`approvals.keys.${action}`)}</dd></div>)}</dl>
          </section>)}
        </div>}
        <div className="row gap-2">
          <span><kbd>↑</kbd><kbd>↓</kbd> {t("approvals.keys.move")}</span>
          <span><kbd>A</kbd> {t("approvals.keys.approve")}</span>
          <span><kbd>R</kbd> {t("approvals.keys.reject")}</span>
          <button type="button" className="keys-toggle ml-auto" aria-expanded={showKeys} aria-controls="approval-keys" aria-keyshortcuts="?" onClick={() => setShowKeys((open) => !open)}>
            <kbd>?</kbd> {t(showKeys ? "approvals.keys.hide" : "approvals.keys.all")}
          </button>
        </div>
      </div>
    </aside>
    <section className="detail-pane">
      {multi ? <BatchDetail batch={batch} deciding={deciding} now={now} decideMany={decideMany} clear={() => setPicked(new Set())} />
      : item && <div className={`detail approval-detail tone-${methodTone(item.method)}`} aria-busy={busy}>
        <header className="detail-header">
          <code className={`method tone-${methodTone(item.method)}`}>{item.method}</code>
          <strong className="mono truncate">{item.model}</strong>
          <span className="row gap-2 ml-auto">
            <button className="btn danger-ghost sm icon-only" disabled={busy} onClick={() => setRejecting(true)} aria-label={t("approvals.rejectWithReason")} aria-keyshortcuts="Shift+R" data-tip={`${t("approvals.rejectWithReason")} · ⇧R`}><Icon name="pencil" /></button>
            <button className="btn danger-ghost sm" disabled={busy} onClick={() => decide(item, false)} aria-keyshortcuts="R"><Icon name="x" />{t("approvals.reject")}</button>
            <button className="btn primary sm" disabled={busy || expired} onClick={() => decide(item, true)}><Icon name="check" />{busy ? t("common.processing") : t("approvals.approve")}</button>
          </span>
        </header>
        {rejecting && <div className="reject-reason row gap-2">
          <label className="search grow"><Icon name="pencil" />
            <input autoFocus value={reason} maxLength={500} disabled={busy} placeholder={t("approvals.reasonPlaceholder")} aria-label={t("approvals.reasonLabel")}
              onChange={(event) => setReason(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !busy) { event.preventDefault(); decide(item, false, reason); }
                else if (event.key === "Escape") { event.preventDefault(); setRejecting(false); setReason(""); }
              }} />
          </label>
          <button className="btn danger sm" disabled={busy} onClick={() => decide(item, false, reason)}>{t("approvals.reject")}</button>
          <button className="btn ghost sm" onClick={() => { setRejecting(false); setReason(""); }}>{t("common.cancel")}</button>
        </div>}
        {expired && <p key={nudge} className={`banner warn compact${nudge ? " nudge" : ""}`} role="status"><Icon name="clock" />{t("approvals.expiredHint")}</p>}
        <div className="panel">
          <dl className="props compact">
            <div><dt>{t("common.environment")}</dt><dd className="row gap-2">{env
              ? <button type="button" className="link-btn row gap-2" onClick={() => openEnvironment(env.id)} data-tip={t("approvals.openEnvironment")}><EnvIcon env={env} />{item.environment_name}</button>
              : <><Icon name="server" />{item.environment_name}</>}</dd></div>
            <div><dt>{t("approvals.requestedBy")}</dt><dd className="row gap-2" data-tip={item.client_label ? t("approvals.clientRequest") : t("approvals.localRequest")}><Icon name="clients" />{item.client_label ?? t("approvals.local")}</dd></div>
            <div><dt>{t("approvals.requestedAt")}</dt><dd><time dateTime={item.created_at} data-tip={formatDateTime(new Date(item.created_at), { hour: "2-digit", minute: "2-digit", second: "2-digit" })}>{relativeTime(item.created_at, now)}</time></dd></div>
            <div><dt>{t("approvals.expires")}</dt><dd>{countdown(item)}</dd></div>
            {ids && <div><dt>{t("approvals.records", { count: ids.length })}</dt><dd className="record-ids">
              <strong className={ids.length > 1 ? `count tone-${methodTone(item.method)}` : "count"}>{ids.length}</strong>
              <button type="button" className="btn sm" onClick={() => openInShell(item, readCommand(item, ids, values))} aria-keyshortcuts="V" data-tip={`${t("approvals.viewRecordsHint")} · V`}><Icon name="terminal" />{t("approvals.viewRecords", { count: ids.length })}</button>
            </dd></div>}
          </dl>
          {values && <ValuesTable values={values} />}
          {values ? <details className="raw-call"><summary>{t("approvals.rawCall")}</summary><CallDetails item={item} /></details> : <CallDetails item={item} />}
          <div className="copy-command"><CopyButton text={() => shellCommand(item)} label={t("approvals.copyCommand")} className="btn ghost sm" /></div>
        </div>
      </div>}
    </section>
  </div>;
}

/** Terminal read of the target records; for writes, also the fields about to change, to compare with the new values. */
type KeyText = "navigate" | "decide" | "select" | "move" | "viewInShell" | "filter" | "toggleKeys" | "approve" | "reject" | "rejectWithReason" | "mark" | "extend" | "selectAll" | "clear";
/** Shortcut legend: [group, [keys, action][]]. */
const KEY_GROUPS: [KeyText, [string[], KeyText][]][] = [
  ["navigate", [[["↑", "↓"], "move"], [["V"], "viewInShell"], [["/"], "filter"], [["?"], "toggleKeys"]]],
  ["decide", [[["A"], "approve"], [["R"], "reject"], [["⇧", "R"], "rejectWithReason"]]],
  ["select", [[["Space"], "mark"], [["⇧", "↑", "↓"], "extend"], [["Ctrl", "A"], "selectAll"], [["Esc"], "clear"]]],
];

function readCommand(item: Pick<Pending, "model">, ids: number[], values: Record<string, unknown>[] | null) {
  const fields = Object.keys(values?.[0] ?? {}).filter((field) => /^[a-z_][a-z0-9_]*$/i.test(field));
  return `read ${item.model} ${ids.join(",")}${fields.length ? ` -f display_name,${fields.join(",")}` : ""}`;
}

/** Summary of the selected requests, with one decision for all of them. */
function BatchDetail({ batch, deciding, now, decideMany, clear }: {
  batch: Pending[]; deciding: ReadonlySet<string>; now: number;
  decideMany: (approvals: Pending[], approve: boolean) => void; clear: () => void;
}) {
  const { t } = useTranslation();
  const ready = batch.filter((p) => !deciding.has(p.id));
  const approvable = ready.filter((p) => Date.parse(p.expires_at) > now);
  return <div className="detail approval-detail">
    <header className="detail-header">
      <strong>{t("approvals.selectedCount", { count: batch.length })}</strong>
      <span className="row gap-2 ml-auto">
        <button className="btn ghost sm" onClick={clear}>{t("approvals.clearSelection")}</button>
        <button className="btn danger-ghost sm" disabled={!ready.length} onClick={() => decideMany(ready, false)} aria-keyshortcuts="R"><Icon name="x" />{t("approvals.rejectAll", { count: ready.length })}</button>
        <button className="btn primary sm" disabled={!approvable.length} onClick={() => decideMany(approvable, true)} aria-keyshortcuts="A"><Icon name="check" />{t("approvals.approveAll", { count: approvable.length })}</button>
      </span>
    </header>
    {approvable.length < ready.length && <p className="banner warn compact" role="status"><Icon name="clock" />{t("approvals.expiredSkipped", { count: ready.length - approvable.length })}</p>}
    <div className="panel">
      <table className="values-table batch-table">
        <thead><tr><th scope="col">{t("approvals.method")}</th><th scope="col">{t("approvals.model")}</th><th scope="col">{t("common.environment")}</th><th scope="col">{t("approvals.recordsColumn")}</th></tr></thead>
        <tbody>{batch.map((p) => <tr key={p.id}>
          <td><code className={`method tone-${methodTone(p.method)}`}>{p.method}</code></td>
          <td className="mono">{p.model}</td>
          <td>{p.environment_name}</td>
          <td>{deciding.has(p.id) ? <Icon name="refresh" className="spin" /> : recordIds(p)?.length ?? "—"}</td>
        </tr>)}</tbody>
      </table>
    </div>
  </div>;
}
