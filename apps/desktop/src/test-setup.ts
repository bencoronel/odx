// Use jsdom storage instead of Node 25+ localStorage, without Node-specific flags.
const { jsdom } = globalThis as typeof globalThis & { jsdom?: { window: Window } };
if (jsdom) for (const key of ["localStorage", "sessionStorage"] as const) {
  Object.defineProperty(globalThis, key, { configurable: true, value: jsdom.window[key] });
}
// Use English labels by default; localization tests select their own language.
try { localStorage.setItem("odx.language", "en"); } catch { /* jsdom storage can be unavailable. */ }
await import("./i18n").then(({ setLanguage }) => setLanguage("en"));
export {};
