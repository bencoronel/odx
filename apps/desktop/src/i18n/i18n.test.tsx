import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { createInstance } from "i18next";
import LanguageSelector from "../LanguageSelector";
import { resolveLanguage, i18n, languageStorageKey, resources, setLanguage } from "./index";
import { formatDateTime, formatNumber, formatRelativeTime } from "./format";

afterEach(async () => { cleanup(); await setLanguage("en"); localStorage.clear(); });

it("uses English by default and preserves a saved language", () => {
  expect(resolveLanguage("en")).toBe("en");
  expect(resolveLanguage("es")).toBe("es");
  expect(resolveLanguage("invalid")).toBe("en");
  expect(resolveLanguage(null)).toBe("en");
});

it("keeps catalog keys and interpolation variables aligned", () => {
  const entries = (value: object, prefix = ""): [string, string[]][] => Object.entries(value).flatMap(([key, child]) =>
    typeof child === "string" ? [[prefix + key, (child.match(/{{[^}]+}}/g) ?? []).sort()]] : entries(child, `${prefix}${key}.`));
  expect(entries(resources.es.translation)).toEqual(entries(resources.en.translation));
});

it("changes language immediately, persists the selection and updates document language", async () => {
  await setLanguage("en");
  render(<LanguageSelector />);
  fireEvent.click(screen.getByRole("combobox", { name: "Interface language" }));
  fireEvent.mouseDown(screen.getByRole("option", { name: "Spanish" }));
  await screen.findByRole("combobox", { name: "Idioma de la interfaz" });
  expect(localStorage.getItem(languageStorageKey)).toBe("es");
  expect(document.documentElement.lang).toBe("es");
  expect(i18n.t("common.save")).toBe("Guardar");
});

it("uses English when a Spanish translation is missing", async () => {
  const instance = createInstance();
  await instance.init({ lng: "es", fallbackLng: "en", resources: {
    en: resources.en, es: { translation: {} },
  } });
  expect(instance.t("common.save")).toBe("Save");
});

it("formats values using the selected locale", async () => {
  await setLanguage("en");
  expect(formatNumber(1234.5)).toBe("1,234.5");
  expect(formatRelativeTime(-1, "day")).toBe("yesterday");
  const date = new Date("2026-10-17T12:00:00Z");
  expect(formatDateTime(date, { month: "long", timeZone: "UTC" })).toBe("October");
  await setLanguage("es");
  expect(formatNumber(1234.5)).toBe("1234,5");
  expect(formatRelativeTime(-1, "day")).toBe("ayer");
  expect(formatDateTime(date, { month: "long", timeZone: "UTC" })).toBe("octubre");
});

it("renders translated phrases and plural forms in both languages", async () => {
  await setLanguage("en");
  expect(i18n.t("navigation.environments.title")).toBe("Environments");
  expect(i18n.t("common.connections", { count: 1 })).toBe("1 connection");
  expect(i18n.t("common.connections", { count: 3 })).toBe("3 connections");
  await setLanguage("es");
  expect(i18n.t("navigation.environments.title")).toBe("Entornos");
  expect(i18n.t("common.connections", { count: 1 })).toBe("1 conexión");
  expect(i18n.t("common.connections", { count: 3 })).toBe("3 conexiones");
});
