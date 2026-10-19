import type { ReactNode } from "react";
import { META, MODEL_VERBS, VERBS } from "./commands";

const KEYWORDS = new Set(["True", "False", "None", "true", "false", "null"]);
const OPERATORS = new Set(["=", "!=", "<", ">", "<=", ">=", "in", "not in", "like", "ilike", "=like", "=ilike", "child_of", "parent_of", "&", "|", "!"]);

/** Tokenize without losing spaces, strings, numbers, words or punctuation. */
const PIECE = /(\s+)|('(?:\\.|[^'\\])*'?|"(?:\\.|[^"\\])*"?)|(-?\d+(?:\.\d+)?)|(\$_(?!\w)|[A-Za-z_\\][\w.\\-]*|!(?:!|\d+)|-{1,2}[\w-]+)|([[\](){}:,])|(.)/g;

/** Presentation only; commands.ts parses the command. */
export function highlight(line: string): ReactNode[] {
  const out: ReactNode[] = [];
  let word = 0; // Word index outside brackets and strings.
  let depth = 0;
  let verb = "";
  let m: RegExpExecArray | null;
  PIECE.lastIndex = 0;
  let key = 0;
  while ((m = PIECE.exec(line))) {
    const [text, space, str, num, ident, punct] = m;
    let cls = "";
    if (space) { out.push(text); continue; }
    if (str) cls = OPERATORS.has(str.slice(1, -1)) && depth > 0 ? "hl-op" : "hl-str";
    else if (num) cls = "hl-num";
    else if (punct) {
      if ("[({".includes(text)) depth++;
      if ("])}".includes(text)) depth = Math.max(0, depth - 1);
      cls = "hl-punct";
    } else if (ident) {
      if (text === "$_") cls = "hl-kw";
      else if (depth === 0 && word === 0) {
        verb = text;
        cls = META.includes(text) || VERBS.includes(text) || /^!(!|\d+)$/.test(text) ? "hl-cmd" : "hl-err";
      } else if (KEYWORDS.has(text)) cls = "hl-kw";
      else if (depth === 0 && text.startsWith("-")) cls = "hl-opt";
      else if (depth === 0 && word === 1 && MODEL_VERBS.includes(verb)) cls = "hl-model";
      else if (depth === 0 && word === 2 && verb === "call") cls = "hl-method";
      else if (depth === 0 && verb === "\\c") cls = "hl-env";
    }
    if (depth === 0 && !punct) word++;
    else if (punct && depth === 0 && "])}".includes(text)) word++;
    out.push(cls ? <span key={key++} className={cls}>{text}</span> : text);
  }
  return out;
}
