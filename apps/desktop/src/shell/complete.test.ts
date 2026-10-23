import { describe, expect, it } from "vitest";
import { completions, type CompletionContext } from "./complete";

const ctx: CompletionContext = {
  models: ["res.partner", "res.users", "sale.order", "sale.order.line"],
  fields: (m) => (m === "res.partner" ? [{ name: "email", label: "Email", type: "char" }, { name: "name", label: "Name", type: "char" }, { name: "company_id", label: "Company", type: "many2one" }] : undefined),
  connected: true,
};
const values = (input: string, c = ctx) => completions(input, c)?.items.map((i) => i.value);

describe("completion", () => {
  it("lists commands and filters by prefix", () => {
    expect(values("")?.length).toBeGreaterThan(15);
    expect(values("s")?.[0]).toBe("search");
    expect(values("\\his")).toEqual(["\\history"]);
    expect(values("\\d")).toEqual(["\\d", "\\dm"]);
  });
  it("suggests models, options and fields by position", () => {
    expect(values("search sale")).toEqual(["sale.order", "sale.order.line"]);
    expect(values("search order")).toEqual(["sale.order", "sale.order.line"]);
    expect(values("search res.partner ")).toEqual(["-d", "-f", "-l", "-o", "--order"]);
    expect(values("search res.partner -f name ")).toEqual(["-d", "-l", "-o", "--order"]);
    expect(values("search res.partner -f ")).toEqual(["email", "name", "company_id"]);
    const fields = completions("search res.partner -f name,e", ctx);
    expect(fields?.items.map((i) => i.value)).toEqual(["email"]);
    expect(fields?.from).toBe("search res.partner -f name,".length);
  });
  it("loads missing metadata and requires a connection for model suggestions", () => {
    expect(completions("search ", { ...ctx, models: undefined })?.needs).toEqual({ models: true });
    expect(completions("search account.move -f ", ctx)?.needs).toEqual({ fields: "account.move" });
    expect(completions("search ", { ...ctx, connected: false })).toBeNull();
  });
});

it("completes read_group groupby fields after commas", () => {
  const ctx = { envs: [], models: ["res.partner"], connected: true, fields: () => [{ name: "country_id", label: "Country", type: "many2one" }, { name: "state_id", label: "State", type: "many2one" }] };
  expect(completions("read_group res.partner country_id,st", ctx)).toMatchObject({ from: "read_group res.partner country_id,".length, items: [{ value: "state_id" }] });
});

describe("completion inside domains and values", () => {
  const fields = [
    { name: "name", label: "Name", type: "char" },
    { name: "active", label: "Active", type: "boolean" },
    { name: "type", label: "Address type", type: "selection", selection: [["contact", "Contact"], ["invoice", "Invoice"]] as [string, string][] },
  ];
  const c: CompletionContext = { models: ["res.partner"], connected: true, fields: (m) => (m === "res.partner" ? fields : undefined) };
  const at = (input: string) => completions(input, c);
  it("completes field, operator and selection value in a domain tuple", () => {
    expect(at("search res.partner -d [('na")).toMatchObject({ from: "search res.partner -d [('".length, suffix: "', '", items: [{ value: "name" }] });
    expect(at("search res.partner -d [('name', 'il")?.items.map((i) => i.value)).toEqual(["ilike", "not ilike", "=ilike"]);
    expect(at("search res.partner -d [('active', '")?.items.map((i) => i.value)).toEqual(["=", "!="]);
    expect(at("search res.partner -d [('type', '=', 'inv")).toMatchObject({ suffix: "')", items: [{ value: "invoice", detail: "Invoice" }] });
    expect(at("count res.partner -d [('active','=',")?.items.map((i) => i.value)).toEqual(["True", "False"]);
    expect(at("search res.partner -d [(")).toMatchObject({ items: expect.arrayContaining([{ value: "'name'", detail: "Name · char", kind: "field" }]) });
  });
  it("completes keys and values in create and write dicts", () => {
    expect(at("write res.partner 1 {'act")).toMatchObject({ suffix: "': ", items: [{ value: "active" }] });
    expect(at("create res.partner {'name': 'Ada', 'type': 'co")).toMatchObject({ suffix: "'", items: [{ value: "contact" }] });
    expect(at("write res.partner 1 {'active': ")?.items.map((i) => i.value)).toEqual(["True", "False"]);
  });
  it("asks for fields when they are not loaded and ignores other brackets", () => {
    expect(at("search sale.order -d [('na")).toMatchObject({ needs: { fields: "sale.order" } });
    expect(at("call res.partner action [['na")).toBeNull();
  });
});
