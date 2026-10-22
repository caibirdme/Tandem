import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { Provider, ProviderProtocol, ReasoningEffortValue } from '../lib/types';
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

describe('provider editor fields', () => {
  it('shows a required K input before advanced parameters and preserves individual tokens', () => {
    const rendered = markup(provider);
    const context = rendered.match(/<input[^>]*value="131\.072"[^>]*>/)?.[0];
    expect(context).toBeDefined();
    expect(context).toContain('required=""');
    expect(context).toContain('class="number-input"');
    expect(context).toContain('type="number"');
    expect(context).toContain('min="0.001"');
    expect(context).toContain('step="0.001"');
    expect(context).toContain('aria-describedby="context-window-hint-');
    expect(rendered).toContain('<span aria-hidden="true">K</span>');
    expect(rendered).toContain('1 K = 1,000 tokens');
    expect(rendered.indexOf('value="131.072"')).toBeLessThan(rendered.indexOf('<details'));
  });

  it('places model discovery inside the form with one path input and an adjacent primary action', () => {
    const rendered = markup(provider);
    expect(rendered.match(/value="\/v1\/models"/g)).toHaveLength(1);
    expect(rendered).toMatch(
      /<form class="provider-form"[^]*<section class="model-discovery"[^]*<div class="model-discovery-controls"><input[^>]+><button type="button" class="button primary"/,
    );
  });

  it('offers max and preserves a saved max model default', () => {
    const rendered = markup({
      ...provider,
      models: provider.models.map((model) => ({ ...model, reasoningEffort: 'max' })),
    });
    expect(rendered).toContain('<option value="max" selected="">max</option>');
  });

  it('renders the discovered 1M context window as 1000 K with the returned maximum output', () => {
    const rendered = markup({
      ...provider,
      models: provider.models.map((model) => ({
        ...model,
        contextWindow: 1_000_000,
        maxOutputTokens: 128_000,
      })),
    });
    expect(rendered).toMatch(/<input[^>]*value="1000"[^>]*>/);
    expect(rendered).toMatch(/<input[^>]*value="128000"[^>]*>/);
  });

  it.each([
    { supported: null, expected: ['low', 'medium', 'high', 'xhigh', 'max'] },
    { supported: [], expected: [] },
    { supported: ['low', 'xhigh'], expected: ['low', 'xhigh'] },
  ] satisfies { supported: ReasoningEffortValue[] | null; expected: string[] }[])(
    'shows the advertised effort choices $supported in the model editor',
    ({ supported, expected }) => {
      const rendered = markup({
        ...provider,
        models: provider.models.map((model) => ({
          ...model,
          supportedReasoningEfforts: supported,
        })),
      });
      const select = rendered
        .match(/<select[^>]*>[^]*?<\/select>/g)
        ?.find((value) => value.includes('<option value=""'));
      expect(select).toBeDefined();
      expect(
        [...(select ?? '').matchAll(/<option value="([^"]+)"/g)].map((match) => match[1]),
      ).toEqual(expected);
    },
  );

  it('keeps manual effort presets as raw English values when the UI language is Chinese', () => {
    setLocale('zh-CN');
    const rendered = markup(null);
    for (const effort of ['low', 'medium', 'high', 'xhigh', 'max'])
      expect(rendered).toContain(`<option value="${effort}">${effort}</option>`);
    expect(rendered).not.toContain('>极高</option>');
  });

  it('marks required fields and preserves native validation for a new provider', () => {
    const rendered = markup(null);
    const labels = [...rendered.matchAll(/<label(?: [^>]*)?>(.*?)<\/label>/g)].map(
      ([, label]) => label ?? '',
    );
    for (const name of ['Name', 'Base URL', 'Model ID', 'Context window']) {
      const field = labels.find((label) => label.startsWith(`<span>${name}<`));
      expect(field).toBeDefined();
      expect(field).toContain('class="required-mark" aria-hidden="true">*</span>');
      expect(field).toContain('required=""');
    }
    const displayName = labels.find((label) => label.startsWith('Display name<'));
    expect(displayName).toBeDefined();
    expect(displayName).not.toContain('required');
    expect(rendered).toContain('type="submit"');
    expect(rendered).not.toContain('novalidate');
    expect(rendered).not.toContain('(required)');
    expect(rendered).not.toContain('provider-request-url');
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
