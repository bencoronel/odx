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
    expect(() => parseLiteral("{'x': 1e999}")).toThrow(/invalid number/);
    expect(() => parseLiteral("-1e999")).toThrow(/invalid number/);
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
    expect(() => parse(`name res.partner 'a' 'b'`)).toThrow(CommandError);
    expect(() => parse(`name res.partner 'a' 'b'`)).toThrow(/extra text/);
    // Each session keeps its environment, so `$_` and saved ids can never reach another database.
    expect(() => parse(`\\c Staging`)).toThrow(/unknown command/i);
    expect(() => parse(`\\q`)).toThrow(/unknown command/i);
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

describe("OdooTerminal-style commands", () => {
  it("parses read_group with every groupby at once", () => {
    expect(parse(`read_group sale.order partner_id,state -d [('amount_total','>',0)] -f amount_total:sum -l 10 --order "partner_id desc"`)).toMatchObject({
      display: "groups", page: { limit: 10, offset: 0 },
      call: { model: "sale.order", method: "read_group", kwargs: { domain: [["amount_total", ">", 0]], fields: ["amount_total:sum"], groupby: ["partner_id", "state"], lazy: false, limit: 11, orderby: "partner_id desc" } },
    });
    expect(parse("read_group res.partner country_id")).toMatchObject({ call: { kwargs: { fields: [], groupby: ["country_id"], limit: 81 } } });
    expect(() => parse("read_group res.partner")).toThrow(/groupby/);
  });
  it("resolves XML IDs and validates their format", () => {
    expect(parse("ref base.main_company")).toMatchObject({ display: "rows", call: { model: "ir.model.data", method: "search_read", kwargs: { domain: [["module", "=", "base"], ["name", "=", "main_company"]] } } });
    expect(() => parse("ref main_company")).toThrow(/XML ID/);
    expect(parse("uhg base.group_system")).toEqual({ kind: "has_group", group: "base.group_system" });
    expect(() => parse("uhg")).toThrow(/XML ID/);
  });
  it("parses user and metadata commands", () => {
    expect(parse("whoami")).toEqual({ kind: "whoami" });
    expect(() => parse("whoami admin")).toThrow(CommandError);
    expect(parse("metadata res.partner 1,2")).toEqual({ kind: "metadata", model: "res.partner", ids: [1, 2] });
    expect(parse("cam sale.order")).toEqual({ kind: "access", model: "sale.order" });
    expect(() => parse("cam")).toThrow(/model/i);
  });
});

it("reads every field with -f *", () => {
  expect(parse("read res.partner 7 -f *")).toMatchObject({ call: { method: "read", args: [[7]], kwargs: {} } });
});

it("substitutes $_ outside strings only", async () => {
  const { substitute } = await import("./commands");
  expect(substitute("unlink res.partner $_", [1, 2])).toBe("unlink res.partner [1,2]");
  expect(substitute("search res.partner -d [('id','in',$_)]", [4])).toBe("search res.partner -d [('id','in',[4])]");
  expect(substitute("write res.partner 1 {'name': '$_'}", undefined)).toBe("write res.partner 1 {'name': '$_'}");
  expect(() => substitute("unlink res.partner $_", [])).toThrow(/no records/);
  expect(parse("search res.partner -f *")).toMatchObject({ call: { kwargs: { domain: [], limit: 21 } } });
});

it("detects input that is still being typed", async () => {
  const { unfinished } = await import("./commands");
  expect(unfinished("search res.partner -d [('name','=','")).toBe(true);
  expect(unfinished("search res.partner -d [('name'")).toBe(true);
  expect(unfinished("write res.partner 1 {'name': 'a)b'}")).toBe(false);
  expect(unfinished("search res.partner -d [('name','=','x')]")).toBe(false);
});

it("parses \\prev, \\set, \\alias, \\grep and \\cols, and expands aliases and $name", async () => {
  const { expandAlias, substitute } = await import("./commands");
  expect(parse("\\prev")).toEqual({ kind: "prev" });
  expect(parse("\\set vip")).toEqual({ kind: "set", name: "vip", remove: false });
  expect(parse("\\set vip -")).toEqual({ kind: "set", name: "vip", remove: true });
  expect(() => parse("\\set 1x")).toThrow(CommandError);
  expect(parse("\\alias pc search res.partner  -d [('customer_rank','>',0)]")).toEqual({ kind: "alias", name: "pc", command: "search res.partner  -d [('customer_rank','>',0)]" });
  expect(parse("\\alias pc -")).toEqual({ kind: "alias", name: "pc", command: null });
  expect(() => parse("\\alias search count res.partner")).toThrow(CommandError);
  expect(() => parse("\\alias pc frobnicate")).toThrow(CommandError);
  expect(parse("\\grep 'Ada L'")).toEqual({ kind: "grep", q: "Ada L" });
  expect(parse("\\cols id, name")).toEqual({ kind: "cols", columns: ["id", "name"], hide: false });
  expect(() => parse("\\cols id,id")).toThrow(CommandError);
  expect(parse("\\cols -email")).toEqual({ kind: "cols", columns: ["email"], hide: true });
  expect(expandAlias("pc -l 5", { pc: "search res.partner" })).toBe("search res.partner -l 5");
  expect(expandAlias("constructor", {})).toBe("constructor");
  expect(substitute("unlink res.partner $vip", undefined, { vip: { ids: [7, 8] } })).toBe("unlink res.partner [7,8]");
  expect(substitute("write res.partner 1 {'name': '$vip'}", undefined, {})).toBe("write res.partner 1 {'name': '$vip'}");
  expect(() => substitute("unlink res.partner $vip", undefined, {})).toThrow(/vip/);
});
