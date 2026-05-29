import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";
import { App } from "./App";
import { I18nProvider } from "./i18n";
import "./styles.css";

const root = document.getElementById("root");
if (!root) {
    throw new Error("Root element not found");
}

ReactDOM.createRoot(root).render(
    <React.StrictMode>
        <I18nProvider>
            <App />
        </I18nProvider>
    </React.StrictMode>,
);
