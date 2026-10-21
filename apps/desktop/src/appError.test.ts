import { afterEach, expect, it } from "vitest";
import { formatAppError, normalizeAppError } from "./appError";
import { i18n, setLanguage } from "./i18n";

afterEach(async () => { await setLanguage("en"); });

it("resolves stable codes and parameters in both languages", async () => {
  await setLanguage("en");
  expect(formatAppError({ code: "upstream.http_status", params: { status: 503 } })).toBe("Odoo returned HTTP status 503.");
  await setLanguage("es");
  expect(formatAppError({ code: "upstream.http_status", params: { status: 503 } })).toBe("Odoo respondió con el estado HTTP 503.");
});

it("localizes migrated validation codes and safe interpolation parameters on each render", async () => {
  const error = normalizeAppError({
    code: "validation.settings.out_of_range",
    params: { setting: "global_concurrency", min: 1, max: 64, api_key: "private" },
  });
  await setLanguage("en");
  expect(formatAppError(error)).toBe("global_concurrency must be between 1 and 64.");
  await setLanguage("es");
  expect(formatAppError(error)).toBe("global_concurrency debe estar entre 1 y 64.");
  expect(JSON.stringify(error)).not.toContain("private");
});

it("asks for the API key again when the connection target changes", async () => {
  await setLanguage("en");
  expect(formatAppError({ code: "validation.environment.secret_required" })).toBe("Enter the API key again: a new URL, database or user needs its own key.");
});

it("keeps uncertain writes distinct and explicitly says they were not retried", async () => {
  await setLanguage("en");
  expect(formatAppError({ code: "uncertain_result", outcome: "uncertain" })).toContain("was not retried");
  await setLanguage("es");
  expect(formatAppError({ code: "uncertain_result", outcome: "uncertain" })).toContain("no se reintentó");
});

it("localizes unknown errors and only keeps supplied safe detail", async () => {
  await setLanguage("en");
  const unknown = formatAppError({ code: "future.backend.code" });
  expect(unknown).toBe("An unknown error occurred.");
  expect(formatAppError({ code: "future.backend.code", detail: "redacted upstream diagnostic" }))
    .toBe("An unknown error occurred.\nTechnical detail: redacted upstream diagnostic");
  await setLanguage("es");
  expect(formatAppError({ code: "future.backend.code" })).toBe("Se produjo un error desconocido.");
});

it("does not expose arbitrary exception text or secret-shaped fields", () => {
  const normalized = normalizeAppError({ code: "upstream.remote", params: { status: 502, api_key: "never-show-this" }, detail: "redacted", secret: "private" });
  expect(normalized).toEqual({ code: "upstream.remote", params: { status: 502 }, detail: "redacted" });
  expect(normalizeAppError("password=private-token")).toEqual({ code: "unknown" });
  expect(i18n.language).toBe("en");
});
