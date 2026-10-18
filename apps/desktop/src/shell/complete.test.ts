import { describe, expect, it } from "vitest";
import { completions, type CompletionContext } from "./complete";

const ctx: CompletionContext = {
  envs: [{ name: "Production", detail: "" }, { name: "Staging", detail: "" }, { name: "Staging North", detail: "" }],
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
  it("suggests environments, models, options and fields by position", () => {
    expect(values("\\c sta")).toEqual(["Staging", "Staging North"]);
    expect(completions("\\c sta", ctx)?.from).toBe(3);
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
