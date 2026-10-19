import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import type { Settings } from '../lib/types';
import { TitlePreferences } from './TitlePreferences';

const settings: Settings = {
  titleModel: null,
  cliPath: null,
  telemetry: { enabled: false, endpoint: '', protocol: 'http/json', captureContent: false },
  providers: [
    {
      id: 'provider',
      name: 'Fixture provider',
      protocol: 'anthropic',
      baseUrl: 'https://example.com',
      hasApiKey: false,
      models: [
        {
          id: 'model',
          name: 'Fixture model',
          reasoningEffort: null,
          contextWindow: null,
          maxOutputTokens: null,
        },
      ],
    },
  ],
};

describe('title model settings', () => {
  it('defaults to the conversation model and groups configured models by provider', () => {
    const markup = renderToStaticMarkup(
      <TitlePreferences settings={settings} onSaved={async () => undefined} />,
    );
    expect(markup).toContain('<option value="" selected="">跟随对话模型</option>');
    expect(markup).toContain('<optgroup label="Fixture provider">');
    expect(markup).toContain('Fixture model');
    expect(markup).toContain('保存标题设置');
    expect(markup).not.toContain('Copilot CLI 路径');
  });

  it('shows an unavailable saved model explicitly instead of silently selecting another', () => {
    const markup = renderToStaticMarkup(
      <TitlePreferences
        settings={{ ...settings, titleModel: { providerId: 'removed', modelId: 'missing' } }}
        onSaved={async () => undefined}
      />,
    );
    expect(markup).toContain('已保存的模型不可用，请重新选择');
    expect(markup).toMatch(/<button[^>]*type="submit"[^>]*disabled=""/);
  });
});
