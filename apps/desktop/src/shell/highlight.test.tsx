import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { highlight } from "./highlight";

const text = (line: string) => render(<span>{highlight(line)}</span>).container.textContent;
const classOf = (line: string, piece: string) =>
  [...render(<span>{highlight(line)}</span>).container.querySelectorAll("span span")].find((s) => s.textContent === piece)?.className;

describe("highlighting", () => {
  it("preserves text, including incomplete input", () => {
    for (const line of [
      `search res.partner -d [('name','ilike','ada'), ('active','=',True)] -f name,email -l 5`,
      `write res.partner 1,2 {"email": "a@b.c"}`,
      `call sale.order action_confirm [[7]] {'context': {'lang': 'es_ES'}}`,
      `\\c Staging North`, `search res.partner -d [('name', 'ilike`, `  odd  `, `ñ é 😀 \\x`,
    ]) expect(text(line)).toBe(line);
  });
  it("identifies commands, models, options, strings, numbers and keywords", () => {
    const line = `search res.partner -d [('active','=',True)] -l 5`;
    expect(classOf(line, "search")).toBe("hl-cmd");
    expect(classOf(line, "res.partner")).toBe("hl-model");
    expect(classOf(line, "-d")).toBe("hl-opt");
    expect(classOf(line, "'active'")).toBe("hl-str");
    expect(classOf(line, "'='")).toBe("hl-op");
    expect(classOf(line, "True")).toBe("hl-kw");
    expect(classOf(line, "5")).toBe("hl-num");
    expect(classOf("frobnicate x", "frobnicate")).toBe("hl-err");
    expect(classOf("!!", "!!")).toBe("hl-cmd");
    expect(classOf("!12", "!12")).toBe("hl-cmd");
  });
});
