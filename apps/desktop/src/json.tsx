import { Fragment, type ReactNode } from "react";

/** JSON with the Terminal's syntax colors. Indented unless `compact`; strings with line breaks open as a block under their key. */
export function jsonNodes(value: unknown, indent = "", compact = false): ReactNode {
  if (typeof value === "string") return value.includes("\n")
    ? <span className="json-block">{value.replace(/\n$/, "")}</span>
    : <span className="hl-str">{JSON.stringify(value)}</span>;
  if (typeof value === "number") return <span className="hl-num">{String(value)}</span>;
  if (value === null || typeof value === "boolean") return <span className="hl-kw">{String(value)}</span>;
  const inner = compact ? "" : `${indent}  `;
  const array = Array.isArray(value);
  const entries = array ? value.map((item) => [null, item] as const) : Object.entries(value as object);
  const [open, close] = array ? ["[", "]"] : ["{", "}"];
  if (!entries.length) return <span className="hl-punct">{open + close}</span>;
  const br = compact ? "" : "\n";
  return <><span className="hl-punct">{open}</span>{br}
    {entries.map(([key, item], i) => <Fragment key={i}>{inner}
      {key !== null && <><span className="hl-model">{JSON.stringify(key)}</span><span className="hl-punct">: </span></>}
      {jsonNodes(item, inner, compact)}{i < entries.length - 1 && <span className="hl-punct">{compact ? ", " : ","}</span>}{br}</Fragment>)}
    {compact ? "" : indent}<span className="hl-punct">{close}</span></>;
}

/** One line when short; indented when it would wrap. */
export function Json({ value }: { value: unknown }) {
  const line = JSON.stringify(value) ?? "";
  return <>{jsonNodes(value, "", line.length <= 80 && !line.includes("\\n"))}</>;
}

const TOKEN = /("(?:[^"\\\n]|\\.)*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false|null)\b|([{}[\],])/g;

/** Same colors for JSON that cannot be parsed, such as a payload cut at its size limit. */
export function jsonTextNodes(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(TOKEN)) {
    const [token, string, colon, number, keyword] = match;
    if (match.index > last) nodes.push(text.slice(last, match.index));
    const className = string ? (colon ? "hl-model" : "hl-str") : number ? "hl-num" : keyword ? "hl-kw" : "hl-punct";
    nodes.push(<span key={match.index} className={className}>{string ?? token}</span>);
    if (colon) nodes.push(<span key={`${match.index}:`} className="hl-punct">{colon}</span>);
    last = match.index + token.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}
