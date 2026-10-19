import { useSyncExternalStore } from 'react';

import { en } from './en';
import { zhCN } from './zh-CN';

// Register complete catalogs here; the English catalog defines the shared message keys.
export const locales = {
  en: { name: 'English', messages: en },
  'zh-CN': { name: '简体中文', messages: zhCN },
} as const;
export type Locale = keyof typeof locales;
export type MessageKey = keyof typeof en;
export type Messages = Record<MessageKey, string>;
export type Parameters = Record<string, string | number>;
export const localeStorageKey = 'tandem.locale';

let currentLocale: Locale = 'en';
const listeners = new Set<() => void>();

export function resolveLocale(value: string | null): Locale {
  return value !== null && Object.hasOwn(locales, value) ? (value as Locale) : 'en';
}

export function getLocale(): Locale {
  return currentLocale;
}

export function translate(
  key: MessageKey,
  parameters: Parameters = {},
  locale = currentLocale,
): string {
  const message: string = locales[locale].messages[key];
  return message.replace(/\{(\w+)\}/gu, (placeholder, name: string) =>
    Object.hasOwn(parameters, name) ? String(parameters[name]) : placeholder,
  );
}

function applyLocale(locale: Locale) {
  currentLocale = locale;
  if (typeof document !== 'undefined') document.documentElement.lang = locale;
  for (const listener of listeners) listener();
}

export function initializeLocale() {
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(localeStorageKey);
  } catch {
    // Storage may be disabled; the app still starts in English.
  }
  applyLocale(resolveLocale(saved));
}

export function setLocale(locale: Locale) {
  try {
    localStorage.setItem(localeStorageKey, locale);
  } catch {
    // Language changes still apply when the preference cannot be persisted.
  }
  applyLocale(locale);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// Keep translator identities stable for callbacks and effects until the language changes.
const translators = Object.fromEntries(
  (Object.keys(locales) as Locale[]).map((locale) => [
    locale,
    (key: MessageKey, parameters?: Parameters) => translate(key, parameters, locale),
  ]),
) as Record<Locale, (key: MessageKey, parameters?: Parameters) => string>;

export function useI18n() {
  const locale = useSyncExternalStore(subscribe, getLocale, getLocale);
  return { locale, setLocale, t: translators[locale] };
}
