import React, { Suspense, lazy } from "react";
import "./i18n";
import { createRoot } from "react-dom/client";
import App from "./App";
import ContextMenu from "./ContextMenu";
import { restoreWindowState } from "./windowState";
// Bundled so every machine renders the same fonts, installed or not.
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./style.css";

// Browser behaviour that makes a desktop window feel like a web page: dragging text or images out, reload, print and page zoom.
// Dev builds keep reload so Ctrl+R still works while iterating.
window.addEventListener("dragstart", (event) => event.preventDefault());
window.addEventListener("wheel", (event) => { if (event.ctrlKey) event.preventDefault(); }, { passive: false });
window.addEventListener("keydown", (event) => {
  const key = event.key.toLowerCase();
  const command = event.ctrlKey || event.metaKey;
  const reload = key === "f5" || (command && key === "r");
  // Ctrl+A selects text only where text is meant to be selected; lists handle it themselves.
  const selectAll = command && key === "a" && !(event.target as Element).closest?.("input, textarea, [contenteditable], .term-out");
  if ((reload && !import.meta.env.DEV) || selectAll || (command && ["p", "+", "=", "-", "0"].includes(key))) event.preventDefault();
});

// A table window ("?window=table-n") shows one Terminal table; the main window restores its state first.
if (new URLSearchParams(location.search).get("window")?.startsWith("table-")) {
  const TableWindow = lazy(() => import("./TableWindow"));
  createRoot(document.getElementById("root")!).render(<React.StrictMode><Suspense fallback={null}><TableWindow /></Suspense><ContextMenu dev={import.meta.env.DEV} /></React.StrictMode>);
} else {
  const render = () => createRoot(document.getElementById("root")!).render(<React.StrictMode><App /><ContextMenu dev={import.meta.env.DEV} /></React.StrictMode>);
  void restoreWindowState().catch((error) => console.error("Could not restore window state", error)).then(render);
}
