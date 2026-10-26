import { i18n } from "../i18n";

/** Parse JSON and Python literal subsets without executing code: lists, tuples, dictionaries, quoted strings, numbers and boolean/null literals. */
export class LiteralError extends Error {}

export function parseLiteral(text: string): unknown {
  let pos = 0;
  const fail = (msg: string): never => { throw new LiteralError(i18n.t("shell.literal.position", { message: msg, position: pos + 1 })); };
  const ws = () => { while (pos < text.length && /\s/.test(text[pos])) pos++; };
  const peek = () => text[pos];

  function value(): unknown {
    ws();
    const c = peek();
    if (c === undefined) return fail(i18n.t("shell.literal.expectedValue"));
    if (c === "[" || c === "(") return sequence(c === "[" ? "]" : ")");
    if (c === "{") return dict();
    if (c === "'" || c === '"') return string();
    if (c === "-" || c === "+" || /[0-9.]/.test(c)) return number();
    const word = /^[A-Za-z_]+/.exec(text.slice(pos))?.[0];
    const words: Record<string, unknown> = { True: true, False: false, None: null, true: true, false: false, null: null };
    if (word && word in words) { pos += word.length; return words[word]; }
    return fail(i18n.t("shell.literal.unrecognized", { value: text.slice(pos, pos + 12) }));
  }
  function sequence(close: string): unknown[] {
    pos++;
    const out: unknown[] = [];
    ws();
    if (peek() === close) { pos++; return out; }
    for (;;) {
      out.push(value());
      ws();
      if (peek() === ",") { pos++; ws(); if (peek() === close) { pos++; return out; } continue; }
      if (peek() === close) { pos++; return out; }
      fail(i18n.t("shell.literal.expectedSeparator", { close }));
    }
  }
  function dict(): Record<string, unknown> {
    pos++;
    const out: Record<string, unknown> = {};
    ws();
    if (peek() === "}") { pos++; return out; }
    for (;;) {
      ws();
      const key = value();
      if (typeof key !== "string" && typeof key !== "number") fail(i18n.t("shell.literal.keyType"));
      ws();
      if (peek() !== ":") fail(i18n.t("shell.literal.expectedColon"));
      pos++;
      const k = String(key);
      if (Object.prototype.hasOwnProperty.call(out, k)) fail(i18n.t("shell.literal.duplicateKey", { key: k }));
      // An own property even for "__proto__": plain assignment would set the prototype and drop the key.
      Object.defineProperty(out, k, { value: value(), enumerable: true, writable: true, configurable: true });
      ws();
      if (peek() === ",") { pos++; ws(); if (peek() === "}") { pos++; return out; } continue; }
      if (peek() === "}") { pos++; return out; }
      fail(i18n.t("shell.literal.expectedSeparator", { close: "}" }));
    }
  }
  function string(): string {
    const quote = text[pos++];
    let out = "";
    while (pos < text.length && text[pos] !== quote) {
      if (text[pos] === "\\") {
        const n = text[pos + 1];
        const map: Record<string, string> = { n: "\n", t: "\t", r: "\r", "\\": "\\", "'": "'", '"': '"', "/": "/" };
        if (n === "u") {
          const hex = text.slice(pos + 2, pos + 6);
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) fail(i18n.t("shell.literal.invalidEscape"));
          out += String.fromCharCode(parseInt(hex, 16)); pos += 6; continue;
        }
        if (n in map) { out += map[n]; pos += 2; continue; }
        fail(i18n.t("shell.literal.invalidEscape"));
      }
      out += text[pos++];
    }
    if (text[pos] !== quote) fail(i18n.t("shell.literal.unclosedString"));
    pos++;
    return out;
  }
  function number(): number {
    const m = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?/.exec(text.slice(pos));
    if (!m) return fail(i18n.t("shell.literal.invalidNumber"));
    const n = Number(m[0]);
    // Python 3 rejects 010 (it is not octal 8 nor decimal 10); 0, 00, 0.5 and 010.5 stay valid.
    if (!Number.isFinite(n) || /^[+-]?0+[1-9]\d*$/.test(m[0])) fail(i18n.t("shell.literal.invalidNumber"));
    pos += m[0].length;
    if (Number.isInteger(n) && !Number.isSafeInteger(n)) fail(i18n.t("shell.literal.integerRange"));
    return n;
  }

  const result = value();
  ws();
  if (pos < text.length) fail(i18n.t("shell.literal.trailingText"));
  return result;
}

/** IDs as `1,2,3`, `[1, 2]` or `(1,)`. */
export function parseIds(text: string): number[] {
  const raw = /^[\d\s,]+$/.test(text) ? text.split(",").filter((s) => s.trim()).map(Number) : parseLiteral(text);
  const list = Array.isArray(raw) ? raw : [raw];
  if (!list.length || !list.every((n) => Number.isSafeInteger(n) && (n as number) > 0)) {
    throw new LiteralError(i18n.t("shell.literal.ids"));
  }
  return list as number[];
}
