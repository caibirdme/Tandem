import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { Provider, ProviderProtocol } from '../lib/types';
import { ProviderEditor } from './Settings';

const provider: Provider = {
  id: 'provider',
  name: 'Provider',
  protocol: 'anthropic',
  baseUrl: 'https://example.com/x',
  listModelsPath: '/v1/models',
  hasApiKey: false,
  models: [
    {
      id: 'model',
      name: 'Model',
      reasoningEffort: null,
      contextWindow: 131_072,
      maxOutputTokens: null,
    },
  ],
};

function markup(value: Provider | null) {
  return renderToStaticMarkup(
    <ProviderEditor provider={value} onBack={() => undefined} onSaved={async () => undefined} />,
  );
}

afterEach(() => setLocale('en'));

describe('provider editor', () => {
  it('shows saved models as compact edit buttons with their actual context limits', () => {
    const rendered = markup({
      ...provider,
      models: provider.models.map((model) => ({
        ...model,
        contextWindow: 1_000_000,
        maxOutputTokens: 128_000,
      })),
    });
    expect(rendered).toContain('class="provider-model-list"');
    expect(rendered).toContain('class="provider-model-open"');
    expect(rendered).toContain('<strong>Model</strong><code>model</code>');
    expect(rendered).toContain('1000 K');
    expect(rendered).toContain('Click a model to edit its parameters.');
    expect(rendered).not.toContain('<fieldset');
    expect(rendered).not.toContain('value="128000"');
    expect(rendered).not.toContain('<details');
  });

  it('opens discovery and manual addition from separate explicit actions', () => {
    const rendered = markup(provider);
    expect(rendered).toContain('Fetch models');
    expect(rendered).toContain('Add model');
    expect(rendered).not.toContain('value="/v1/models"');
    expect(rendered).not.toContain('type="checkbox"');
    expect(rendered).not.toContain('Model ID');
    expect(rendered).not.toContain('Default reasoning effort');
    expect(rendered.match(/<form /g)).toHaveLength(1);
  });

  it('starts a new provider with an empty model list instead of an unfinished inline form', () => {
    const rendered = markup(null);
    expect(rendered).toContain('Fetch models or add a model to get started.');
    expect(rendered).not.toContain('provider-model-list');
    expect(rendered).not.toContain('provider-model-open');
    expect(rendered).not.toContain('provider-request-url');
  });

  it('allows removing any model, including the last one, before adding its replacement', () => {
    const rendered = markup(provider);
    expect(rendered).toContain('aria-label="Remove model 1"');
    expect(rendered).toContain('Save provider');
  });

  it('marks required provider fields and preserves native validation', () => {
    const rendered = markup(null);
    const labels = [...rendered.matchAll(/<label(?: [^>]*)?>(.*?)<\/label>/g)].map(
      ([, label]) => label ?? '',
    );
    for (const name of ['Name', 'Base URL']) {
      const field = labels.find((label) => label.startsWith(`<span>${name}<`));
      expect(field).toBeDefined();
      expect(field).toContain('class="required-mark" aria-hidden="true">*</span>');
      expect(field).toContain('required=""');
    }
    expect(rendered).toContain('class="provider-fields"');
    expect(rendered).toContain('type="submit"');
    expect(rendered).not.toContain('novalidate');
  });

  it('uses the same compact list when the interface is Chinese', () => {
    setLocale('zh-CN');
    const rendered = markup(provider);
    expect(rendered).toContain('点击模型可编辑参数。');
    expect(rendered).toContain('131.072 K');
    expect(rendered).not.toContain('<fieldset');
  });

  it.each([
    { protocol: 'anthropic', path: '/v1/messages' },
    { protocol: 'openai-responses', path: '/responses' },
    { protocol: 'openai-completions', path: '/chat/completions' },
  ] satisfies { protocol: ProviderProtocol; path: string }[])(
    'shows the actual $protocol endpoint below the base URL',
    ({ protocol, path }) => {
      const rendered = markup({ ...provider, protocol });
      expect(rendered).toContain(`https://example.com/x${path}</code>`);
      expect(rendered).toContain('aria-live="polite"');
      expect(rendered.indexOf('value="https://example.com/x"')).toBeLessThan(
        rendered.indexOf('provider-request-url'),
      );
    },
  );
});
