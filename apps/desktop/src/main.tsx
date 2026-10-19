import React from "react";
import "./i18n";
import { createRoot } from "react-dom/client";
import App from "./App";
import ContextMenu from "./ContextMenu";
import { restoreWindowState } from "./windowState";
import "./style.css";

const render = () => createRoot(document.getElementById("root")!).render(<React.StrictMode><App /><ContextMenu dev={import.meta.env.DEV} /></React.StrictMode>);
void restoreWindowState().catch((error) => console.error("Could not restore window state", error)).then(render);
