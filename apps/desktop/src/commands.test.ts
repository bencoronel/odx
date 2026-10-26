import { expect, it } from "vitest";
import commandList from "../src-tauri/src/command_list.rs?raw";

// Every command the window invokes must be registered (and granted) in src-tauri/src/command_list.rs.
const sources = import.meta.glob(["./**/*.{ts,tsx}", "!./**/*.test.{ts,tsx}"], { query: "?raw", import: "default", eager: true }) as Record<string, string>;

it("only invokes commands the desktop shell registers", () => {
  const registered = new Set(commandList.match(/\$m! \{([\s\S]*?)\}/)![1].split(",").map((name) => name.trim()).filter(Boolean));
  const invoked = new Set(Object.values(sources).flatMap((source) => [...source.matchAll(/invoke(?:<[^>]*>)?\("([a-z_]+)"/g)].map((match) => match[1])));
  expect(invoked.size).toBeGreaterThan(50);
  expect([...invoked].filter((name) => !registered.has(name))).toEqual([]);
});
