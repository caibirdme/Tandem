import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  getLocale,
  initializeLocale,
  locales,
  localeStorageKey,
  resolveLocale,
  setLocale,
  translate,
  useI18n,
} from './index';
import type { MessageKey } from './index';

function createStorage() {
  const values = new Map<string, string>();
  return {
    values,
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => {
      values.set(key, value);
    }),
  };
}

function placeholders(message: string) {
  return [...message.matchAll(/\{(\w+)\}/gu)].map((match) => match[1]).toSorted();
}

let storage: ReturnType<typeof createStorage>;
let html: { lang: string };

beforeEach(() => {
  storage = createStorage();
  html = { lang: '' };
  vi.stubGlobal('localStorage', storage);
  vi.stubGlobal('document', { documentElement: html });
});

afterEach(() => {
  setLocale('en');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function LanguagePreview() {
  const { locale, t } = useI18n();
  return <span lang={locale}>{t('chat.modelLabel', { name: 'Model' })}</span>;
}

describe('language preference', () => {
  it('starts in English and uses English when no preference was saved', () => {
    expect(getLocale()).toBe('en');
    initializeLocale();
    expect(getLocale()).toBe('en');
    expect(html.lang).toBe('en');
    expect(renderToStaticMarkup(<LanguagePreview />)).toBe('<span lang="en">Model: Model</span>');
    expect(storage.setItem).not.toHaveBeenCalled();
  });

  it('restores saved Chinese before the first render and updates the document language', () => {
    storage.values.set(localeStorageKey, 'zh-CN');
    initializeLocale();
    expect(getLocale()).toBe('zh-CN');
    expect(html.lang).toBe('zh-CN');
    expect(renderToStaticMarkup(<LanguagePreview />)).toBe('<span lang="zh-CN">模型：Model</span>');
  });

  it('applies a selection immediately and persists it for the next launch', () => {
    initializeLocale();
    setLocale('zh-CN');
    expect(html.lang).toBe('zh-CN');
    expect(storage.setItem).toHaveBeenCalledWith(localeStorageKey, 'zh-CN');
    expect(translate('chat.workMode')).toBe('工作');
    expect(renderToStaticMarkup(<LanguagePreview />)).toBe('<span lang="zh-CN">模型：Model</span>');
    initializeLocale();
    expect(getLocale()).toBe('zh-CN');
    setLocale('en');
    expect(html.lang).toBe('en');
    expect(translate('chat.workMode')).toBe('Work');
    expect(storage.values.get(localeStorageKey)).toBe('en');
  });

  it.each(['unsupported', 'zh', 'EN', '__proto__'])(
    'falls back to English for saved %s',
    (saved) => {
      storage.values.set(localeStorageKey, saved);
      initializeLocale();
      expect(getLocale()).toBe('en');
      expect(html.lang).toBe('en');
      expect(resolveLocale(saved)).toBe('en');
    },
  );

  it('remains usable when preference storage rejects reads and writes', () => {
    storage.getItem.mockImplementation(() => {
      throw new Error('Storage is unavailable');
    });
    storage.setItem.mockImplementation(() => {
      throw new Error('Storage is unavailable');
    });
    expect(() => initializeLocale()).not.toThrow();
    expect(getLocale()).toBe('en');
    expect(() => setLocale('zh-CN')).not.toThrow();
    expect(getLocale()).toBe('zh-CN');
    expect(html.lang).toBe('zh-CN');
    expect(translate('common.settings')).toBe('设置');
  });
});

describe('translation catalogs', () => {
  const keys = Object.keys(locales.en.messages).toSorted() as MessageKey[];

  it('has the same nonempty messages and interpolation parameters in every language', () => {
    for (const locale of Object.values(locales)) {
      expect(Object.keys(locale.messages).toSorted()).toEqual(keys);
      for (const key of keys) {
        expect(locale.messages[key].trim(), key).not.toBe('');
        expect(placeholders(locale.messages[key]), key).toEqual(
          placeholders(locales.en.messages[key]),
        );
      }
    }
  });

  it('interpolates strings and numeric zero without translating or reinterpreting user values', () => {
    expect(translate('settings.providerModels', { protocol: 'API', count: 0 }, 'en')).toBe(
      'API · Models: 0',
    );
    expect(translate('settings.providerModels', { protocol: 'API', count: 0 }, 'zh-CN')).toBe(
      'API · 0 个模型',
    );
    const name = '<Project> $& {name}';
    expect(translate('chat.modelLabel', { name }, 'en')).toBe(`Model: ${name}`);
    expect(translate('chat.modelLabel', { name }, 'zh-CN')).toBe(`模型：${name}`);
  });
});
