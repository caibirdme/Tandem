import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import type { Provider, ProviderProtocol } from '../lib/types';
import { ProviderEditor } from './Settings';

const provider: Provider = {
  id: 'provider',
  name: 'Provider',
  protocol: 'anthropic',
  baseUrl: 'https://example.com/x',
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

describe('provider editor fields', () => {
  it('shows a required K input before advanced parameters and preserves individual tokens', () => {
    const rendered = markup(provider);
    const context = rendered.match(/<input[^>]*value="131\.072"[^>]*>/)?.[0];
    expect(context).toBeDefined();
    expect(context).toContain('required=""');
    expect(context).toContain('min="0.001"');
    expect(context).toContain('step="0.001"');
    expect(context).toContain('aria-describedby="context-window-hint-');
    expect(rendered).toContain('<span aria-hidden="true">K</span>');
    expect(rendered).toContain('1 K = 1,000 tokens');
    expect(rendered.indexOf('value="131.072"')).toBeLessThan(rendered.indexOf('<details'));
  });

  it('requires users to specify a context limit for a new provider', () => {
    const rendered = markup(null);
    expect(rendered).toMatch(/<input required="" type="number"[^>]*value=""/);
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
