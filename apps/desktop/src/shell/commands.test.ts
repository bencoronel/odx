import { describe, expect, it } from "vitest";
import { CommandError, parse, tokenize } from "./commands";
import { parseIds, parseLiteral } from "./literal";

describe("literales", () => {
  it("accepts JSON and Python literals without executing code", () => {
    expect(parseLiteral(`[('name', 'ilike', "ada"), '|', ('active','=',True)]`)).toEqual([["name", "ilike", "ada"], "|", ["active", "=", true]]);
    expect(parseLiteral(`{'name': 'O\\'Brien', 'n': -1.5, 'x': None,}`)).toEqual({ name: "O'Brien", n: -1.5, x: null });
    expect(() => parseLiteral("__import__('os')")).toThrow();
    expect(() => parseLiteral("{'a': 1, 'a': 2}")).toThrow();
    expect(() => parseLiteral("9007199254740993")).toThrow();
    expect(parseIds("1, 2,3")).toEqual([1, 2, 3]);
    expect(() => parseIds("0")).toThrow();
  });
});

describe("commands", () => {
  it("separa respetando comillas y anidamiento", () => {
    expect(tokenize(`write res.partner 1,2 {'name': 'Ada Lovelace'}`)).toEqual(["write", "res.partner", "1,2", "{'name': 'Ada Lovelace'}"]);
    expect(() => tokenize(`name res.partner 'abc`)).toThrow(CommandError);
  });
  it("parses searches, writes and metacommands", () => {
    const search = parse(`search res.partner -d [('name','ilike','ada')] -f name,email -l 5 -o 10 --order "name desc"`);
    expect(search).toMatchObject({ kind: "call", page: { limit: 5, offset: 10 }, call: { method: "search_read", kwargs: { domain: [["name", "ilike", "ada"]], fields: ["id", "name", "email"], limit: 6, offset: 10, order: "name desc" } } });
    expect(parse(`write res.partner [42] {'name': 'Ada'}`)).toMatchObject({ call: { method: "write", args: [[42], { name: "Ada" }] } });
    expect(parse(`call sale.order action_confirm [[7]]`)).toMatchObject({ call: { model: "sale.order", method: "action_confirm", args: [[7]], kwargs: {} } });
    expect(parse(`\\c Staging North`)).toEqual({ kind: "connect", env: "Staging North" });
    expect(parse(`\\context {'lang': 'es_ES'}`)).toEqual({ kind: "context", value: { lang: "es_ES" } });
    expect(parse(`\\d res.partner mail`)).toMatchObject({ display: "fields", filter: "mail" });
    expect(parse(`\\d res.partner mail --names name,email`)).toMatchObject({ display: "fields", filter: "mail", call: { kwargs: { allfields: ["name", "email"] } } });
    expect(() => parse(`\\d res.partner --names name,name`)).toThrow(/unique/);
    expect(parse("   ")).toBeNull();
  });
  it("rejects ambiguous input with useful errors", () => {
    expect(() => parse("search res.partner name")).toThrow(/did you mean -d or -f\?/);
    expect(() => parse("search Res.Partner")).toThrow(/invalid model/i);
    expect(() => parse("search res.partner -z 1")).toThrow(/unknown option/i);
    expect(() => parse("frobnicate")).toThrow(/unknown command/i);
    expect(() => parse("call res.partner _private")).toThrow(/method/i);
    // Extra ids are an error, not silently dropped ("5,6" is the list form).
    expect(() => parse("unlink res.partner 5 6")).toThrow(CommandError);
    expect(() => parse("read res.partner 1 2")).toThrow(CommandError);
    expect(parse("unlink res.partner 5,6")).toMatchObject({ call: { method: "unlink", args: [[5, 6]] } });
    // Invalid \u escapes fail instead of becoming NUL or eating the closing quote.
    expect(() => parseLiteral(`'\\uZZZZ'`)).toThrow();
    expect(() => parseLiteral(`'\\u41'`)).toThrow();
    expect(parseLiteral(`'\\u0041'`)).toBe("A");
  });
  it("asks for one extra row so truncated model and name lists say so", () => {
    expect(parse("\\dm sale")).toMatchObject({ display: "models", page: { limit: 500, offset: 0 }, call: { kwargs: { limit: 501 } } });
    expect(parse("name res.partner Ada -l 5")).toMatchObject({ display: "names", page: { limit: 5, offset: 0 }, call: { method: "name_search", kwargs: { limit: 6 } } });
  });
});
