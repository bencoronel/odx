import { useTranslation } from "react-i18next";
import Select from "./Select";
import { currentLanguage, setLanguage, type Language } from "./i18n";

/** Saved per device, outside the vault, so it also works before a vault exists. */
export function LanguageSelect({ className }: { className?: string }) {
  const { t } = useTranslation();
  return <Select<Language> className={className} label={t("settings.language.label")} value={currentLanguage()}
    options={[{ value: "en", label: t("settings.language.english") }, { value: "es", label: t("settings.language.spanish") }]}
    onChange={(language) => { void setLanguage(language); }} />;
}
