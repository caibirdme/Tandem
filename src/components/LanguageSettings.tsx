import { locales, resolveLocale, useI18n } from '../i18n';

export function LanguageSettings() {
  const { locale, setLocale, t } = useI18n();
  return (
    <label>
      {t('settings.language')}
      <select value={locale} onChange={(event) => setLocale(resolveLocale(event.target.value))}>
        {Object.entries(locales).map(([value, language]) => (
          <option key={value} value={value} lang={value}>
            {language.name}
          </option>
        ))}
      </select>
      <small>{t('settings.languageHint')}</small>
    </label>
  );
}
