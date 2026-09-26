import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import type { Lang } from "../prefs/storage";
import en from "./locales/en.json";
import ru from "./locales/ru.json";

export function initI18n(lang: Lang): typeof i18n {
  if (!i18n.isInitialized) {
    i18n.use(initReactI18next).init({
      resources: {
        ru: { translation: ru },
        en: { translation: en },
      },
      lng: lang,
      fallbackLng: "en",
      supportedLngs: ["ru", "en"],
      interpolation: { escapeValue: false },
      returnEmptyString: false,
      react: { useSuspense: false },
    });
  }
  return i18n;
}

export { i18n };
