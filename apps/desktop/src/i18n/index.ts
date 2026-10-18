import { createInstance } from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./locales/en.json";
import es from "./locales/es.json";

export const languages = ["en", "es"] as const;
export type Language = typeof languages[number];
export const languageStorageKey = "odx.language";
export const resources = { en: { translation: en }, es: { translation: es } };

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: "translation";
    resources: { translation: typeof en };
    returnNull: false;
  }
}

export function resolveLanguage(stored: string | null): Language {
  return languages.includes(stored as Language) ? stored as Language : "en";
}

function initialLanguage(): Language {
  let stored: string | null = null;
  try { stored = localStorage.getItem(languageStorageKey); } catch { /* Storage can be unavailable. */ }
  return resolveLanguage(stored);
}

export const i18n = createInstance();
// Bundled resources: initialization completes before the first render; no network or Suspense.
void i18n.use(initReactI18next).init({
  resources,
  lng: initialLanguage(),
  supportedLngs: [...languages],
  fallbackLng: "en",
  defaultNS: "translation",
  initAsync: false,
  returnNull: false,
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});

export function currentLanguage(): Language {
  return i18n.resolvedLanguage === "es" ? "es" : "en";
}

function updateDocumentLanguage() {
  if (typeof document !== "undefined") document.documentElement.lang = currentLanguage();
}
i18n.on("languageChanged", updateDocumentLanguage);
updateDocumentLanguage();

export async function setLanguage(language: Language): Promise<void> {
  await i18n.changeLanguage(language);
  try { localStorage.setItem(languageStorageKey, language); } catch { /* Keep working without persistence. */ }
}
