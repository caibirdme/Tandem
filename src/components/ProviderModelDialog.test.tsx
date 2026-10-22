import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { ModelFormDraft } from '../lib/settingsForm';
import type { ReasoningEffortValue } from '../lib/types';
import { ProviderModelForm } from './ProviderModelDialog';

const model: ModelFormDraft = {
  id: 'opus',
  name: 'Opus',
  contextWindowK: '131.072',
  maxOutputTokens: 32_000,
  reasoningEffort: 'xhigh',
  supportedReasoningEfforts: ['low', 'xhigh'],
};

function markup(value: ModelFormDraft | null) {
  return renderToStaticMarkup(
    <ProviderModelForm
      model={value}
      existingIds={[]}
      onSave={() => undefined}
      onClose={() => undefined}
    />,
  );
}

afterEach(() => setLocale('en'));

describe('provider model form', () => {
  it('prepopulates the shared form with exact saved limits and effort', () => {
    const rendered = markup(model);
    expect(rendered).toMatch(/<input[^>]*value="opus"/);
    expect(rendered).toMatch(/<input[^>]*value="Opus"/);
    expect(rendered).toMatch(/<input[^>]*value="131\.072"/);
    expect(rendered).toMatch(/<input[^>]*value="32000"/);
    expect(rendered).toContain('<option value="xhigh" selected="">xhigh</option>');
    expect(rendered).not.toContain('<details');
  });

  it.each([
    { supported: undefined, expected: ['low', 'medium', 'high', 'xhigh', 'max'] },
    { supported: null, expected: ['low', 'medium', 'high', 'xhigh', 'max'] },
    { supported: [], expected: [] },
    { supported: ['low', 'xhigh'], expected: ['low', 'xhigh'] },
  ] satisfies {
    supported: ReasoningEffortValue[] | null | undefined;
    expected: string[];
  }[])('offers only the advertised reasoning choices: $supported', ({ supported, expected }) => {
    const value = { ...model, reasoningEffort: null };
    if (supported === undefined) delete value.supportedReasoningEfforts;
    else value.supportedReasoningEfforts = supported;
    const rendered = markup(value);
    expect([...rendered.matchAll(/<option value="([^"]+)"/g)].map((match) => match[1])).toEqual(
      expected,
    );
    expect(rendered).toContain('<option value="" selected="">Model default</option>');
  });

  it('keeps all manual presets in raw English in the Chinese UI', () => {
    setLocale('zh-CN');
    const rendered = markup(null);
    for (const effort of ['low', 'medium', 'high', 'xhigh', 'max'])
      expect(rendered).toContain(`<option value="${effort}">${effort}</option>`);
    expect(rendered).not.toContain('>极高</option>');
    expect(rendered).toContain('<option value="" selected="">模型默认</option>');
  });

  it('requires ID and precise K context entry while keeping output optional', () => {
    const rendered = markup(null);
    const inputs = [...rendered.matchAll(/<input[^>]*>/g)].map(([input]) => input);
    expect(inputs[0]).toContain('required=""');
    expect(inputs[0]).toContain('maxLength="256"');
    expect(inputs[1]).not.toContain('required');
    expect(inputs[2]).toContain('required=""');
    expect(inputs[2]).toContain('min="0.001"');
    expect(inputs[2]).toContain('max="10000"');
    expect(inputs[2]).toContain('step="0.001"');
    expect(inputs[2]).toContain('aria-describedby=');
    expect(inputs[3]).not.toContain('required');
    expect(inputs[3]).toContain('step="1"');
    expect(rendered).toContain('1 K = 1,000 tokens');
    expect(rendered).toContain('<button type="button" class="button">Cancel</button>');
    expect(rendered).toContain('<button type="submit" class="button primary">Save</button>');
    expect(rendered).not.toContain('novalidate');
  });
});
