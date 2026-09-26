import { PrimeReactProvider } from "primereact/api";
import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import { initI18n } from "./i18n";
import { registerPrimeLocales } from "./i18n/primeLocale";
import { PrefsProvider, usePrefs } from "./prefs/PrefsContext";
import { loadLangMode, resolveLang } from "./prefs/storage";

import "primereact/resources/primereact.min.css";
import "primeicons/primeicons.css";
import "primeflex/primeflex.css";
import "./styles.css";

initI18n(resolveLang(loadLangMode()));
registerPrimeLocales();

function Shell() {
  const { resolvedLang } = usePrefs();
  return (
    <PrimeReactProvider value={{ locale: resolvedLang }}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </PrimeReactProvider>
  );
}

const rootEl = document.getElementById("root");
if (rootEl) {
  ReactDOM.createRoot(rootEl).render(
    <React.StrictMode>
      <PrefsProvider>
        <Shell />
      </PrefsProvider>
    </React.StrictMode>,
  );
}
