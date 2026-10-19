import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { initialCredentialEditor, reduceCredentialEditor } from '../lib/credentialEditor';
import type { CredentialEditorState } from '../lib/credentialEditor';
import { ProviderKeyInput } from './ProviderKeyInput';

function markup(state: CredentialEditorState) {
  return renderToStaticMarkup(
    <form>
      <ProviderKeyInput
        state={state}
        hasSavedKey
        update={() => undefined}
        onRetry={() => undefined}
      />
    </form>,
  );
}

describe('provider credential input', () => {
  it.each([false, true])(
    'renders an accessible non-submit visibility control when visible=%s',
    (visible) => {
      const state = { ...initialCredentialEditor(false), value: 'fixture-ui-key', visible };
      const rendered = markup(state);
      expect(rendered).toContain(`type="${visible ? 'text' : 'password'}"`);
      expect(rendered).toContain('value="fixture-ui-key"');
      const eye = rendered.match(/<button[^>]*aria-label="(?:Show|Hide) key"[^>]*>/)?.[0];
      expect(eye).toContain('type="button"');
      expect(eye).toContain(`aria-label="${visible ? 'Hide key' : 'Show key'}"`);
      expect(eye).toContain(`aria-pressed="${String(visible)}"`);
    },
  );

  it('allows manual replacement after read failure and offers a non-submit retry', () => {
    const failed = reduceCredentialEditor(initialCredentialEditor(true), {
      type: 'loadFailed',
      request: 0,
      error: 'Fixture unavailable',
    });
    const rendered = markup(failed);
    expect(rendered).toContain('Reload key');
    expect(rendered).toContain('saving a blank value keeps the existing key');
    expect(rendered).not.toContain('disabled');
    for (const button of rendered.match(/<button[^>]*>/g) ?? [])
      expect(button).toContain('type="button"');
  });

  it('keeps removal explicit and masks the field again', () => {
    const removed = reduceCredentialEditor(
      { ...initialCredentialEditor(false), visible: true, value: 'fixture-old-key' },
      { type: 'removalToggled' },
    );
    const rendered = markup(removed);
    expect(rendered).toContain('type="password"');
    expect(rendered).toContain('value=""');
    expect(rendered).toContain('Saving will clear the existing key');
    expect(rendered).toContain('Cancel key removal');
  });
});
