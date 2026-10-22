import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import {
  initialCredentialEditor,
  reduceCredentialEditor,
  type CredentialEditorState,
} from '../lib/credentialEditor';
import type { DiscoveredModel } from '../lib/types';
import {
  DiscoveredModelRow,
  discoveryImportIssue,
  ProviderModelDiscovery,
} from './ProviderModelDiscovery';

function markup(
  saved = false,
  blank = false,
  disabled = false,
  credential: CredentialEditorState = {
    ...initialCredentialEditor(saved && !blank),
    modified: blank,
  },
) {
  return renderToStaticMarkup(
    <ProviderModelDiscovery
      baseUrl="https://example.com"
      listModelsPath="/v1/models"
      onListModelsPathChange={() => undefined}
      protocol="anthropic"
      credential={credential}
      hasSavedKey={saved}
      disabled={disabled}
      existingIds={[]}
      onAdd={() => undefined}
    />,
  );
}

const model: DiscoveredModel = {
  id: 'claude-opus-5.5',
  displayName: 'Claude Opus 5.5',
  contextWindow: 1_000_000,
  maxOutputTokens: 128_000,
  supportedReasoningEfforts: ['low', 'xhigh'],
};

function row(value = model, selected = false, existing = false) {
  return renderToStaticMarkup(
    <DiscoveredModelRow
      model={value}
      selected={selected}
      existing={existing}
      disabled={false}
      onToggle={() => undefined}
    />,
  );
}

afterEach(() => setLocale('en'));

describe('discovery dialog trigger', () => {
  it('keeps catalog inputs and selections out of the provider form until opened', () => {
    const rendered = markup();
    expect(rendered).toContain('Fetch models');
    expect(rendered).toContain('type="button"');
    expect(rendered).not.toContain('type="checkbox"');
    expect(rendered).not.toContain('<input');
    expect(rendered).not.toContain('Add selected');
    expect(rendered).not.toContain('disabled');
  });

  it('disables the trigger while the provider is busy', () => {
    expect(markup(false, false, true)).toMatch(/<button[^>]*disabled=""/);
  });

  it('requires explicit key intent after blanking a saved credential', () => {
    const rendered = markup(true, true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Enter an API key or choose Clear saved key before fetching');
    expect(rendered).toContain('aria-describedby=');
  });

  it('disables fetching while a saved credential is loading', () => {
    const rendered = markup(true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Wait for the saved API key to load');
  });
});

describe('discovery after canceling credential removal', () => {
  it.each(['pending', 'failed'] as const)(
    'allows explicit keyless discovery but blocks canceled removal with a %s read',
    (status) => {
      const initial = initialCredentialEditor(true);
      const unread =
        status === 'pending'
          ? initial
          : reduceCredentialEditor(initial, {
              type: 'loadFailed',
              request: 0,
              error: 'Fixture unavailable',
            });
      const removed = reduceCredentialEditor(unread, { type: 'removalToggled' });
      expect(markup(true, false, false, removed)).not.toContain('disabled=""');
      const canceled = reduceCredentialEditor(removed, { type: 'removalToggled' });
      expect(markup(true, false, false, canceled)).toContain('disabled=""');
    },
  );
});

describe('catalog rows', () => {
  it('starts unchecked and collapsed while showing the returned context on the summary row', () => {
    const rendered = row();
    expect(rendered).toContain('type="checkbox"');
    expect(rendered).not.toContain('checked=""');
    expect(rendered).toContain('1000 K');
    expect(rendered).toContain('aria-expanded="false"');
    expect(rendered).toMatch(/class="catalog-model-details" hidden=""/);
    expect(rendered).toContain('Show parameters for Claude Opus 5.5');
    expect(rendered).toContain('1,000,000');
    expect(rendered).toContain('128,000');
    expect(rendered).toContain('<code>claude-opus-5.5</code>');
  });

  it('does not select an already imported model even with a stale selection', () => {
    const rendered = row(model, true, true);
    expect(rendered).toMatch(/<input[^>]*disabled=""/);
    expect(rendered).not.toContain('checked=""');
    expect(rendered).toContain('Already added');
  });

  it('shows only advertised effort values in English in the Chinese interface', () => {
    setLocale('zh-CN');
    const rendered = row();
    expect(rendered).toContain('low, xhigh');
    expect(rendered).not.toContain('medium');
    expect(rendered).not.toContain('极高');
  });

  it.each([
    { efforts: undefined, text: 'Not provided' },
    { efforts: [], text: 'Not supported' },
  ] satisfies { efforts: DiscoveredModel['supportedReasoningEfforts']; text: string }[])(
    'distinguishes unknown efforts from explicitly unsupported efforts: $text',
    ({ efforts, text }) => {
      const value = { ...model };
      if (efforts === undefined) delete value.supportedReasoningEfforts;
      else value.supportedReasoningEfforts = efforts;
      expect(row(value)).toContain(text);
    },
  );

  it.each([
    {
      contextWindow: undefined,
      maxOutputTokens: 128_000,
      issue: 'settings.discoveryMissingContext',
    },
    {
      contextWindow: 1_000_000,
      maxOutputTokens: 1_000_000,
      issue: 'settings.discoveryConflictingLimits',
    },
    {
      contextWindow: 1_000_000,
      maxOutputTokens: 2_000_000,
      issue: 'settings.discoveryConflictingLimits',
    },
  ])('disables import without silently replacing unsavable limits: $issue', (limits) => {
    const value = { ...model, maxOutputTokens: limits.maxOutputTokens };
    if (limits.contextWindow === undefined) delete value.contextWindow;
    else value.contextWindow = limits.contextWindow;
    expect(discoveryImportIssue(value)).toBe(limits.issue);
    const rendered = row(value, true);
    expect(rendered).toMatch(/<input[^>]*disabled=""/);
    expect(rendered).not.toContain('checked=""');
    expect(rendered).toContain('Add this model manually');
  });

  it.each([undefined, 128_000])('can import a valid context with output %s', (maxOutputTokens) => {
    const value = { ...model };
    if (maxOutputTokens === undefined) delete value.maxOutputTokens;
    else value.maxOutputTokens = maxOutputTokens;
    expect(discoveryImportIssue(value)).toBeNull();
    const rendered = row(value, true);
    expect(rendered).not.toContain('disabled=""');
    expect(rendered).toContain('checked=""');
  });
});
