import React from "react";
import "./i18n";
import { createRoot } from "react-dom/client";
import App from "./App";
import ContextMenu from "./ContextMenu";
import "./style.css";
createRoot(document.getElementById("root")!).render(<React.StrictMode><App /><ContextMenu dev={import.meta.env.DEV} /></React.StrictMode>);
