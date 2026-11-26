import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import Select from "./Select";
import HeaderInput from "./HeaderInput";
import { Button, IconButton, Segment, Segmented, cx } from "./ui";

export const FILTER_OPS = ["=", "!=", ">", ">=", "<", "<=", "ilike", "in"] as const;
export type FilterOp = (typeof FILTER_OPS)[number];
export type FilterCombinator = "and" | "or";

export type FilterCondition = {
  kind: "cond";
  id: number;
  field: string;
  op: FilterOp;
  value: string;
};

export type FilterGroup = {
  kind: "group";
  id: number;
  combinator: FilterCombinator;
  items: FilterNode[];
};

export type FilterNode = FilterCondition | FilterGroup;

function parseScalar(trimmed: string): unknown {
  if (trimmed === "true") return true;
  if (trimmed === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(trimmed)) return Number(trimmed);
  return trimmed;
}

function parseFilterValue(op: FilterOp, raw: string): unknown {
  const trimmed = raw.trim();
  if (op === "in") {
    return trimmed ? trimmed.split(",").map((part) => parseScalar(part.trim())) : [];
  }
  return parseScalar(trimmed);
}

function formatFilterValue(val: unknown): string {
  if (Array.isArray(val)) return val.map((v) => String(v)).join(", ");
  if (val === null || val === undefined) return "";
  return String(val);
}

export function createEmptyRoot(nextId: () => number): FilterGroup {
  return { kind: "group", id: nextId(), combinator: "and", items: [] };
}

/** Parse an Odoo prefix-notation domain into a nested AND/OR FilterGroup tree. Returns null for unsupported constructs (e.g. "!" or unknown ops). */
export function domainToTree(domain: unknown[], nextId: () => number): FilterGroup | null {
  let idx = 0;

  function parseExpr(): FilterNode | null {
    if (idx >= domain.length) return null;
    const token = domain[idx++];
    if (token === "&" || token === "|") {
      const comb: FilterCombinator = token === "|" ? "or" : "and";
      const left = parseExpr();
      const right = parseExpr();
      if (!left || !right) return null;
      const items: FilterNode[] = [];
      if (left.kind === "group" && left.combinator === comb) items.push(...left.items);
      else items.push(left);
      if (right.kind === "group" && right.combinator === comb) items.push(...right.items);
      else items.push(right);
      return { kind: "group", id: nextId(), combinator: comb, items };
    }
    if (Array.isArray(token) && token.length === 3 && typeof token[0] === "string" && typeof token[1] === "string") {
      if (!(FILTER_OPS as readonly string[]).includes(token[1])) return null;
      return {
        kind: "cond",
        id: nextId(),
        field: token[0],
        op: token[1] as FilterOp,
        value: formatFilterValue(token[2]),
      };
    }
    return null;
  }

  const top: FilterNode[] = [];
  while (idx < domain.length) {
    const node = parseExpr();
    if (!node) return null;
    top.push(node);
  }

  if (top.length === 0) return { kind: "group", id: nextId(), combinator: "and", items: [] };
  if (top.length === 1 && top[0].kind === "group") return top[0];

  // Implicit top-level AND: flatten any direct AND groups.
  const flattened: FilterNode[] = [];
  for (const node of top) {
    if (node.kind === "group" && node.combinator === "and") flattened.push(...node.items);
    else flattened.push(node);
  }
  return { kind: "group", id: nextId(), combinator: "and", items: flattened };
}

/** Serialize a FilterGroup tree back into an Odoo Polish prefix-notation domain array. */
export function treeToDomain(root: FilterGroup): unknown[] {
  function emit(node: FilterNode, isRoot: boolean): unknown[] {
    if (node.kind === "cond") {
      const field = node.field.trim();
      if (!field) return [];
      return [[field, node.op, parseFilterValue(node.op, node.value)]];
    }
    const parts = node.items.map((child) => emit(child, false)).filter((p) => p.length > 0);
    if (parts.length === 0) return [];
    if (parts.length === 1) return parts[0];
    if (node.combinator === "or") {
      return [...Array<string>(parts.length - 1).fill("|"), ...parts.flat()];
    }
    // At root level Odoo AND is implicit; nested AND inside an OR needs explicit "&" operators.
    if (isRoot) return parts.flat();
    return [...Array<string>(parts.length - 1).fill("&"), ...parts.flat()];
  }
  return emit(root, true);
}

/** Format an Odoo domain into a human-readable string with AND / OR and parentheses for nested groups. */
export function formatDomainReadable(domain: unknown[]): string {
  if (!domain.length) return "";
  let seq = 1;
  const tree = domainToTree(domain, () => seq++);
  if (!tree) return JSON.stringify(domain);

  function fmt(node: FilterNode, parentComb?: FilterCombinator): string {
    if (node.kind === "cond") return `${node.field} ${node.op} ${node.value}`;
    const sep = node.combinator === "or" ? "  OR  " : "  AND  ";
    const inner = node.items.map((child) => fmt(child, node.combinator)).join(sep);
    if (parentComb && node.items.length >= 2) return `(${inner})`;
    return inner;
  }

  return fmt(tree);
}

/** Set the value of the last condition in the tree, or append a new create_date >= placeholder condition at root. */
export function applyPlaceholderToTree(root: FilterGroup, placeholder: string, nextId: () => number): FilterGroup {
  let lastId: number | null = null;
  function findLast(node: FilterNode) {
    if (node.kind === "cond") lastId = node.id;
    else node.items.forEach(findLast);
  }
  findLast(root);

  if (lastId === null) {
    return {
      ...root,
      items: [...root.items, { kind: "cond", id: nextId(), field: "create_date", op: ">=", value: placeholder }],
    };
  }

  function replaceLast(node: FilterNode): FilterNode {
    if (node.kind === "cond") return node.id === lastId ? { ...node, value: placeholder } : node;
    return { ...node, items: node.items.map(replaceLast) };
  }
  return replaceLast(root) as FilterGroup;
}

const MAX_NESTING_DEPTH = 3;

export function FilterGroupEditor({
  group,
  depth = 0,
  fieldSuggestions,
  placeholders,
  nextId,
  onChange,
  onRemove,
  onInsertPlaceholder,
}: {
  group: FilterGroup;
  depth?: number;
  fieldSuggestions: string[];
  placeholders?: readonly string[];
  nextId: () => number;
  onChange: (next: FilterGroup) => void;
  onRemove?: () => void;
  onInsertPlaceholder?: (ph: string) => void;
}) {
  const { t } = useTranslation();
  const isRoot = depth === 0;
  const showCombinator = !isRoot || group.items.length >= 2;

  function updateItem(id: number, next: FilterNode) {
    onChange({ ...group, items: group.items.map((item) => (item.id === id ? next : item)) });
  }

  function removeItem(id: number) {
    onChange({ ...group, items: group.items.filter((item) => item.id !== id) });
  }

  function addCondition() {
    onChange({
      ...group,
      items: [...group.items, { kind: "cond", id: nextId(), field: "", op: "=", value: "" }],
    });
  }

  function addSubgroup() {
    const childComb: FilterCombinator = group.combinator === "and" ? "or" : "and";
    onChange({
      ...group,
      items: [
        ...group.items,
        {
          kind: "group",
          id: nextId(),
          combinator: childComb,
          items: [
            { kind: "cond", id: nextId(), field: "", op: "=", value: "" },
            { kind: "cond", id: nextId(), field: "", op: "=", value: "" },
          ],
        },
      ],
    });
  }

  const content = <>
    <div className="flex items-center justify-between gap-2">
      {isRoot ? (
        <span className="text-xs font-medium text-fg-2">{t("views.filters")}</span>
      ) : (
        <span className="text-2xs font-medium uppercase tracking-wide text-fg-3">{t("views.filterGroup")}</span>
      )}
      <div className="flex items-center gap-1.5">
        {showCombinator && (
          <Segmented role="radiogroup" aria-label={t("views.filterCombinator")}>
            <Segment role="radio" aria-checked={group.combinator === "and"} aria-pressed={group.combinator === "and"} onClick={() => onChange({ ...group, combinator: "and" })}>
              {t("views.matchAll")}
            </Segment>
            <Segment role="radio" aria-checked={group.combinator === "or"} aria-pressed={group.combinator === "or"} onClick={() => onChange({ ...group, combinator: "or" })}>
              {t("views.matchAny")}
            </Segment>
          </Segmented>
        )}
        {onRemove && (
          <IconButton size="sm" aria-label={t("views.removeGroup")} onClick={onRemove}>
            <Icon name="x" />
          </IconButton>
        )}
      </div>
    </div>

    {group.items.map((item) =>
      item.kind === "cond" ? (
        <div key={item.id} className="grid grid-cols-[1fr_92px_1fr_auto] gap-2 items-center max-sm:grid-cols-1">
          <HeaderInput
            className="mono"
            aria-label={t("views.filterField")}
            value={item.field}
            placeholder="state"
            suggestions={fieldSuggestions}
            showAll
            onChange={(val) => updateItem(item.id, { ...item, field: val })}
            onBlur={() => undefined}
          />
          <Select
            label={t("views.filterOp")}
            value={item.op}
            options={FILTER_OPS.map((op) => ({ value: op, label: op }))}
            onChange={(op) => updateItem(item.id, { ...item, op: op as FilterOp })}
          />
          <input
            className="mono"
            aria-label={t("views.filterValue")}
            value={item.value}
            placeholder="exception"
            onChange={(e) => updateItem(item.id, { ...item, value: e.target.value })}
          />
          <IconButton size="sm" aria-label={t("common.delete")} onClick={() => removeItem(item.id)}>
            <Icon name="x" />
          </IconButton>
        </div>
      ) : (
        <FilterGroupEditor
          key={item.id}
          group={item}
          depth={depth + 1}
          fieldSuggestions={fieldSuggestions}
          nextId={nextId}
          onChange={(nextChild) => updateItem(item.id, nextChild)}
          onRemove={() => removeItem(item.id)}
        />
      ),
    )}

    <div className="flex flex-wrap items-center gap-1.5">
      <Button size="sm" onClick={addCondition}>
        <Icon name="plus" />{t("views.addFilter")}
      </Button>
      {depth < MAX_NESTING_DEPTH && (
        <Button size="sm" variant="ghost" onClick={addSubgroup}>
          <Icon name="plus" />{t("views.addGroup")}
        </Button>
      )}
      {isRoot && placeholders && onInsertPlaceholder && <>
        <span className="text-fg-3 text-xs ml-auto mr-1">{t("views.placeholders")}:</span>
        {placeholders.map((ph) => (
          <button
            key={ph}
            type="button"
            className="px-1.5 py-0.5 rounded-sm border border-line bg-surface-2 font-mono text-2xs text-fg-2 cursor-pointer hover:border-accent hover:text-fg"
            onClick={() => onInsertPlaceholder(ph)}
          >
            {ph}
          </button>
        ))}
      </>}
    </div>
  </>;

  if (isRoot) {
    return <div className="grid gap-2">{content}</div>;
  }

  return (
    <div
      role="group"
      aria-label={t("views.filterGroup")}
      className={cx(
        "grid gap-2 rounded-md border border-line border-l-2 border-l-accent/70 bg-surface-2/50 p-2.5",
      )}
    >
      {content}
    </div>
  );
}
