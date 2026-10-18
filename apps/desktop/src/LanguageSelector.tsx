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

export default function LanguageSelector() {
  const { t } = useTranslation();
  return <section className="panel">
    <header className="panel-header"><h3>{t("settings.language.title")}</h3></header>
    <div className="setting-row">
      <div><strong>{t("settings.language.label")}</strong><p className="hint">{t("settings.language.hint")}</p></div>
      <LanguageSelect />
    </div>
  </section>;
}
